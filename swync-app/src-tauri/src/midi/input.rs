//! Live MIDI in: a controller, or a keyboard, as something a program can name.
//!
//! Two quite different things arrive down the same wire and leave here by
//! different roads, because what they are for is different.
//!
//! **Controllers** — a knob, a wheel, a pedal — are a *value that is always
//! there*. `cc("push", 1, 74)` reads the last thing that knob said, at audio
//! rate, from inside the graph. That makes it [`crate::audio_in`]'s neighbour:
//! a number the outside world writes and the audio callback reads, so it lives
//! in atomics and is never locked against.
//!
//! **Notes** are a *thing that happened at a moment*. They cannot be read;
//! they have to be delivered, once each, to the scheduler — the only thread
//! allowed to push a voice. So they queue behind a mutex, which is safe
//! precisely because the audio thread never touches them.
//!
//! ## Slots, and why a port is interned rather than looked up
//!
//! A graph node reads a controller on the audio callback, so it cannot hash a
//! port name to find it. What it holds instead is a **slot** — a small integer
//! fixed for the life of the process — and the table it indexes is allocated
//! once at startup.
//!
//! Slots are handed out by [`slot_for`], keyed on the selector **as it was
//! written**, and doing that touches no hardware at all. That is deliberate
//! and load-bearing: lowering happens in a thousand tests and on every
//! keystroke-triggered eval, and none of that should be opening MIDI ports.
//! Opening is a separate step ([`ensure_open`]) that only `run_code` takes.
//!
//! So a program can be compiled on a machine with nothing plugged in and mean
//! exactly what it means on the machine with the rack — which is the same rule
//! `midiout` and `input(channel)` already follow.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use fundsp::prelude64::*;
use midir::{Ignore, MidiInput, MidiInputConnection};

use crate::midi::ports::{self, Match, Selector, CLIENT_NAME};

/// How many distinct ports one program may read from.
///
/// The table below is allocated once for all of them, so this is a memory
/// figure rather than a musical one: eight ports of every controller on every
/// channel is a quarter of a megabyte, and eight input devices is more than
/// anybody has patched into one laptop. A ninth is refused with a warning
/// rather than silently read from the wrong place.
pub const MAX_PORTS: usize = 8;

/// MIDI channels, which is not a number that will be changing.
const CHANNELS: usize = 16;

/// Controller numbers per channel, likewise.
const CONTROLLERS: usize = 128;

/// The longest a live note sounds when nothing ever ends it.
///
/// A safety net rather than a limit anybody should meet: a note-off can be
/// lost — a cable pulled mid-chord, a controller that stops mid-message — and
/// what that would otherwise leave is a voice droning until the app is
/// quit. Sixty seconds is longer than any held note and short enough that
/// losing one does not ruin the evening. It is the same worry `midi::out`
/// answers with its own release tracking, from the other side.
pub const HELD_SECS: f64 = 60.0;

/// The seven bits a controller and a velocity are carried in.
const MAX_DATA: f32 = 127.0;

/// The fourteen bits a pitch bend is carried in, and its centre.
const BEND_RANGE: f32 = 16383.0;
const BEND_CENTRE: f32 = 8192.0;

/// One port's worth of continuous values, as `f32::to_bits`.
///
/// Everything here is written by a MIDI callback on midir's own thread and
/// read by the audio callback, so all of it is atomic and none of it is ever
/// locked. Allocated once at startup and never resized — a graph node holds an
/// index into it.
struct Slot {
    /// By channel then controller number. Zero until something says otherwise,
    /// which is the right answer for a knob nobody has touched: a program that
    /// reads one before it moves gets the bottom of its range rather than a
    /// jump the first time it is nudged.
    controllers: Vec<AtomicU32>,
    /// Pitch bend per channel, already folded to -1..=1 about its centre.
    bend: Vec<AtomicU32>,
    /// Channel pressure — aftertouch — per channel, 0..=1.
    pressure: Vec<AtomicU32>,
}

impl Slot {
    fn new() -> Slot {
        Slot {
            controllers: (0..CHANNELS * CONTROLLERS).map(|_| AtomicU32::new(0)).collect(),
            bend: (0..CHANNELS).map(|_| AtomicU32::new(0)).collect(),
            pressure: (0..CHANNELS).map(|_| AtomicU32::new(0)).collect(),
        }
    }
}

/// A note that arrived, on its way to the scheduler.
///
/// Plain data with a slot rather than a port name, because this crosses to
/// another thread and what it is matched against there — the voice already
/// sounding for this key — is matched on the same three numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveNote {
    pub slot: usize,
    /// 1-16, as it is written and spoken.
    pub channel: u8,
    pub note: u8,
    /// 0..=1, as everything else in the language is. Zero on a release.
    pub velocity: f32,
    pub on: bool,
}

/// Everything arriving from outside. One per process — see [`bus`].
pub struct MidiIn {
    slots: Vec<Slot>,
    /// Which selector each slot was handed out for, in slot order. Only the
    /// setup path touches it, never the audio callback.
    interned: Mutex<Vec<Selector>>,
    /// How many slots have been handed out. Read without the lock by
    /// [`ensure_open`]'s fast path, which runs on every eval.
    taken: AtomicUsize,
    /// Open connections by slot. Held for the life of the process: dropping a
    /// `MidiInputConnection` closes the port, and the callback that writes
    /// into a slot lives inside one.
    connections: Mutex<HashMap<usize, MidiInputConnection<()>>>,
    /// Notes waiting for the scheduler to turn them into voices.
    ///
    /// A mutex rather than atomics, unlike everything above it, and the reason
    /// is what reads it: a note is delivered *once* to the one thread allowed
    /// to push a voice, and neither end of that is the audio callback. What
    /// would be unsafe here is exactly what is not happening.
    notes: Mutex<VecDeque<LiveNote>>,
}

/// The one input bus.
///
/// A `OnceLock` singleton for the same reason `audio_in::bus()` is one:
/// `realize` is a pure function of the IR called from three threads and a
/// great many tests, and what it would otherwise have to be handed is the same
/// value every time.
pub fn bus() -> &'static MidiIn {
    static BUS: OnceLock<MidiIn> = OnceLock::new();
    BUS.get_or_init(MidiIn::new)
}

/// What a node holds when the port it named could not be given a slot. Reads
/// as silence forever, which is what a program naming a ninth port should
/// hear.
pub const NO_SLOT: usize = usize::MAX;

impl MidiIn {
    fn new() -> MidiIn {
        MidiIn {
            slots: (0..MAX_PORTS).map(|_| Slot::new()).collect(),
            interned: Mutex::new(Vec::new()),
            taken: AtomicUsize::new(0),
            connections: Mutex::new(HashMap::new()),
            notes: Mutex::new(VecDeque::new()),
        }
    }

    /// The last value a controller sent, 0..=1.
    ///
    /// Called per sample from the audio callback, so it is one relaxed load
    /// and no branches worth speaking of. Out-of-range arguments answer zero
    /// rather than panicking: the realizer has already refused what a program
    /// can get wrong, and what is left is a slot that was never handed out.
    #[inline]
    pub fn controller(&self, slot: usize, channel: u8, number: u8) -> f32 {
        let Some(port) = self.slots.get(slot) else { return 0.0 };
        let at = Ord::max(channel, 1) as usize * CONTROLLERS - CONTROLLERS + number as usize;
        match port.controllers.get(at) {
            Some(v) => f32::from_bits(v.load(Ordering::Relaxed)),
            None => 0.0,
        }
    }

    /// Pitch bend, -1..=1 about the centre.
    #[inline]
    pub fn bend(&self, slot: usize, channel: u8) -> f32 {
        self.per_channel(slot, channel, |port| &port.bend)
    }

    /// Channel pressure, 0..=1.
    #[inline]
    pub fn pressure(&self, slot: usize, channel: u8) -> f32 {
        self.per_channel(slot, channel, |port| &port.pressure)
    }

    #[inline]
    fn per_channel(
        &self,
        slot: usize,
        channel: u8,
        which: impl Fn(&Slot) -> &Vec<AtomicU32>,
    ) -> f32 {
        let Some(port) = self.slots.get(slot) else { return 0.0 };
        match which(port).get(Ord::max(channel, 1) as usize - 1) {
            Some(v) => f32::from_bits(v.load(Ordering::Relaxed)),
            None => 0.0,
        }
    }

    /// Take every note that has arrived since the last ask.
    ///
    /// Drained rather than read, because a note is an event: the scheduler
    /// turns each one into exactly one voice, and a second reader would either
    /// double them or lose them. There is one caller, on the scheduler thread.
    pub fn take_notes(&self) -> Vec<LiveNote> {
        match self.notes.lock() {
            Ok(mut notes) => notes.drain(..).collect(),
            // A poisoned queue is a panic in a MIDI callback, which has already
            // gone wrong somewhere worth more than this pass's notes. Dropping
            // them keeps the scheduler running, which is what the room needs.
            Err(_) => Vec::new(),
        }
    }

    /// Everything a slot was holding down, released.
    ///
    /// Not a MIDI message — nothing is sent anywhere. It is the queue being
    /// told that whatever it thought was held is not, which is what a stop
    /// means for a keyboard: the scheduler cuts the voices it pushed, and a
    /// note-off arriving afterwards must not then cut a *new* voice on the
    /// same key.
    pub fn forget_notes(&self) {
        if let Ok(mut notes) = self.notes.lock() {
            notes.clear();
        }
    }
}

/// The slot a selector reads from, handing out a new one if this is the first
/// time it has been named.
///
/// **Touches no hardware.** It interns the selector as written — see the module
/// docs for why that matters — so lowering a program is the same work whether
/// or not anything is plugged in. [`ensure_open`] is what later goes looking
/// for the port.
///
/// `None` when every slot is taken, which is a program naming a ninth port.
pub fn slot_for(selector: &Selector) -> Option<usize> {
    let bus = bus();
    let mut interned = bus.interned.lock().ok()?;
    if let Some(slot) = interned.iter().position(|s| s == selector) {
        return Some(slot);
    }
    if interned.len() >= MAX_PORTS {
        return None;
    }
    interned.push(selector.clone());
    bus.taken.store(interned.len(), Ordering::Release);
    Some(interned.len() - 1)
}

/// Open a port for every slot that has been asked for and is not yet
/// listening.
///
/// Called from `run_code`, once per eval, which is the only place that both
/// knows a program has been compiled and is allowed to talk to the platform.
/// Idempotent and cheap on the overwhelmingly common eval, which names no MIDI
/// at all and returns on the first line.
///
/// Answers with what could not be opened, for the problems panel. A port that
/// is not here is a warning and not a failure, exactly as it is for `midiout`.
pub fn ensure_open() -> Vec<String> {
    let bus = bus();
    if bus.taken.load(Ordering::Acquire) == 0 {
        return Vec::new();
    }

    let Ok(interned) = bus.interned.lock() else { return Vec::new() };
    let Ok(mut connections) = bus.connections.lock() else { return Vec::new() };

    // Asked for once rather than per slot: enumerating talks to the platform,
    // and a program naming four ports would otherwise ask four times.
    let available = ports::inputs();
    let mut missing = Vec::new();

    for (slot, selector) in interned.iter().enumerate() {
        if connections.contains_key(&slot) {
            continue;
        }
        let name = match ports::find(&available, selector) {
            Match::One(port) | Match::Ambiguous(port, _) => port.name,
            Match::Missing => {
                missing.push(format!(
                    "no MIDI input here answers to {selector}, so nothing will come from it. \
                     The program still runs — the settings panel lists the ports this \
                     machine has, with their numbers."
                ));
                continue;
            }
        };
        match open(slot, &name) {
            Some(connection) => {
                connections.insert(slot, connection);
            }
            None => missing.push(format!(
                "the MIDI input {selector} could not be opened — another application may \
                 be holding it."
            )),
        }
    }

    missing
}

/// Open one port and start writing what it says into its slot.
///
/// The callback midir runs is the whole of this module's hot path on the
/// writing side, and it does the least it can: one store for a controller, or
/// one push for a note. Everything a program might *want* from a message —
/// scaling, smoothing, matching a note-off to the voice it ends — is somebody
/// else's job further down, where there is no callback deadline.
fn open(slot: usize, name: &str) -> Option<MidiInputConnection<()>> {
    let mut midi = MidiInput::new(CLIENT_NAME).ok()?;
    // Nothing here reads clock, sysex or active sensing yet, and active
    // sensing in particular arrives 300 times a second on some gear — waking
    // the callback for something nothing looks at.
    midi.ignore(Ignore::All);

    let port = midi
        .ports()
        .into_iter()
        .find(|p| midi.port_name(p).is_ok_and(|found| found == name))?;

    midi.connect(&port, name, move |_, bytes, _| receive(slot, bytes), ()).ok()
}

/// Status bytes, with the channel masked off.
const NOTE_OFF: u8 = 0x80;
const NOTE_ON: u8 = 0x90;
const CONTROL_CHANGE: u8 = 0xB0;
const CHANNEL_PRESSURE: u8 = 0xD0;
const PITCH_BEND: u8 = 0xE0;

/// One message, from midir's thread.
fn receive(slot: usize, bytes: &[u8]) {
    let Some((status, data)) = bytes.split_first() else { return };
    // Channels are 0-15 on the wire and 1-16 everywhere a person writes one.
    let channel = (status & 0x0F) + 1;
    let bus = bus();
    let Some(port) = bus.slots.get(slot) else { return };
    let index = channel as usize - 1;

    match (status & 0xF0, data) {
        (CONTROL_CHANGE, [number, value]) => {
            let at = index * CONTROLLERS + *number as usize;
            if let Some(cell) = port.controllers.get(at) {
                cell.store((*value as f32 / MAX_DATA).to_bits(), Ordering::Relaxed);
            }
        }
        (CHANNEL_PRESSURE, [value]) => {
            if let Some(cell) = port.pressure.get(index) {
                cell.store((*value as f32 / MAX_DATA).to_bits(), Ordering::Relaxed);
            }
        }
        (PITCH_BEND, [low, high]) => {
            // Fourteen bits, low seven first, folded to -1..=1 so that a wheel
            // at rest is zero and a program can add it to a note in semitones
            // without knowing any of this.
            let raw = ((*high as u16) << 7 | *low as u16) as f32;
            if let Some(cell) = port.bend.get(index) {
                cell.store(((raw - BEND_CENTRE) / BEND_CENTRE.max(1.0)).clamp(-1.0, 1.0)
                    .to_bits(), Ordering::Relaxed);
            }
            let _ = BEND_RANGE;
        }
        // A note-on at velocity zero *is* a note-off, and has been since the
        // running-status days. Folding the two here means nothing downstream
        // has to know that.
        (NOTE_ON, [note, velocity]) => {
            push(slot, channel, *note, *velocity, *velocity > 0);
        }
        (NOTE_OFF, [note, _]) => push(slot, channel, *note, 0, false),
        _ => {}
    }
}

/// How many notes may wait for the scheduler before the oldest are dropped.
///
/// The scheduler drains this every pass, so reaching it means it has stopped —
/// and a queue that grows without a reader is a leak that ends the evening
/// rather than a burst that survives it. Two hundred is several seconds of the
/// fastest playing anybody does.
const MAX_QUEUED: usize = 200;

fn push(slot: usize, channel: u8, note: u8, velocity: u8, on: bool) {
    let Ok(mut notes) = bus().notes.lock() else { return };
    if notes.len() >= MAX_QUEUED {
        notes.pop_front();
    }
    notes.push_back(LiveNote {
        slot,
        channel,
        note,
        velocity: velocity as f32 / MAX_DATA,
        on,
    });
}

/// Hold the process's one bus for the duration of a test.
///
/// The same guard [`crate::audio_in::exclusive`] is, for the same reason and
/// with one addition. [`bus`] is a singleton because there is one set of MIDI
/// inputs, and the suite runs in threads — so two tests with opinions about
/// what a knob last said would each be right half the time, and the note queue
/// is drained by whoever asks first.
///
/// The addition is that slots are *finite and never released*. That is right
/// for a program, which names a handful of ports and runs until it is quit,
/// and wrong for a suite, which would exhaust [`MAX_PORTS`] after eight tests.
/// So this clears the table as well as taking the lock — which is safe only
/// because it is the exclusive guard, and is why the two are one function
/// rather than two.
#[cfg(test)]
pub(crate) fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let bus = bus();
    if let Ok(mut interned) = bus.interned.lock() {
        interned.clear();
        bus.taken.store(0, Ordering::Release);
    }
    bus.forget_notes();
    for slot in &bus.slots {
        for cell in slot.controllers.iter().chain(&slot.bend).chain(&slot.pressure) {
            cell.store(0.0f32.to_bits(), Ordering::Relaxed);
        }
    }
    guard
}

/// Put a message on the bus as if it had arrived, for tests elsewhere in the
/// crate that are about what happens *after* one does.
///
/// The scheduler's tests need this and cannot reach `receive`, which is
/// private because nothing outside this module has any business deciding what
/// a MIDI message means.
#[cfg(test)]
pub(crate) fn inject(slot: usize, bytes: &[u8]) {
    receive(slot, bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Interning is what makes lowering hermetic, so this is the property
    /// worth pinning hardest: the same selector is the same slot, a different
    /// one is a different slot, and neither asked the platform anything.
    #[test]
    fn a_port_named_twice_reads_from_one_slot() {
        let _bus = exclusive();
        let a = slot_for(&Selector::Name("keys".into())).unwrap();
        let again = slot_for(&Selector::Name("keys".into())).unwrap();
        assert_eq!(a, again);
    }

    #[test]
    fn two_different_ports_read_from_different_slots() {
        let _bus = exclusive();
        let a = slot_for(&Selector::Name("keys".into())).unwrap();
        let b = slot_for(&Selector::Name("push".into())).unwrap();
        assert_ne!(a, b);
    }

    /// A name and a number are different ways of saying which port, and
    /// nothing here can tell whether they mean the same one — that is resolved
    /// later, against the machine. Interning them separately is the honest
    /// answer, and costs a slot.
    #[test]
    fn a_name_and_a_number_are_interned_apart() {
        let _bus = exclusive();
        let named = slot_for(&Selector::Name("keys".into())).unwrap();
        let numbered = slot_for(&Selector::Number(31_337)).unwrap();
        assert_ne!(named, numbered);
    }

    #[test]
    fn a_slot_nobody_wrote_to_reads_as_nothing() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        assert_eq!(bus().controller(slot, 1, 74), 0.0);
        assert_eq!(bus().bend(slot, 1), 0.0);
        assert_eq!(bus().pressure(slot, 1), 0.0);
    }

    /// A node holding [`NO_SLOT`] is one whose port could not be given one.
    /// It has to read as silence rather than panicking or reading somebody
    /// else's knob.
    #[test]
    fn a_node_with_no_slot_reads_as_nothing() {
        let _bus = exclusive();
        assert_eq!(bus().controller(NO_SLOT, 1, 74), 0.0);
        assert_eq!(bus().bend(NO_SLOT, 1), 0.0);
        assert_eq!(bus().pressure(NO_SLOT, 1), 0.0);
    }

    #[test]
    fn a_controller_message_is_read_back_as_a_fraction_of_its_range() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        receive(slot, &[CONTROL_CHANGE, 74, 127]);
        assert_eq!(bus().controller(slot, 1, 74), 1.0);
        receive(slot, &[CONTROL_CHANGE, 74, 0]);
        assert_eq!(bus().controller(slot, 1, 74), 0.0);
    }

    #[test]
    fn a_controller_is_read_on_the_channel_it_arrived_on() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        receive(slot, &[CONTROL_CHANGE | 9, 74, 127]);
        assert_eq!(bus().controller(slot, 10, 74), 1.0, "channel 10 is status | 9");
        assert_eq!(bus().controller(slot, 1, 74), 0.0, "and channel 1 heard nothing");
    }

    /// A wheel at rest is zero, not a half — so a program can add it to a note
    /// without subtracting a centre it should never have had to know about.
    #[test]
    fn a_pitch_wheel_at_rest_reads_as_zero() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        receive(slot, &[PITCH_BEND, 0x00, 0x40]);
        assert!(bus().bend(slot, 1).abs() < 1e-4, "got {}", bus().bend(slot, 1));
    }

    #[test]
    fn a_pitch_wheel_reads_minus_one_to_one() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        receive(slot, &[PITCH_BEND, 0x7F, 0x7F]);
        assert!(bus().bend(slot, 1) > 0.99);
        receive(slot, &[PITCH_BEND, 0x00, 0x00]);
        assert_eq!(bus().bend(slot, 1), -1.0);
    }

    #[test]
    fn a_note_arrives_as_an_event_rather_than_a_value() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        receive(slot, &[NOTE_ON, 60, 127]);
        let notes = bus().take_notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].note, 60);
        assert_eq!(notes[0].velocity, 1.0);
        assert!(notes[0].on);
    }

    /// Drained rather than read: the scheduler turns each note into exactly one
    /// voice, so a second ask must find nothing.
    #[test]
    fn a_note_is_only_delivered_once() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        receive(slot, &[NOTE_ON, 60, 100]);
        assert_eq!(bus().take_notes().len(), 1);
        assert_eq!(bus().take_notes().len(), 0);
    }

    /// The running-status convention, folded here so nothing downstream has to
    /// know it: a note-on at velocity zero is a release.
    #[test]
    fn a_note_on_at_no_velocity_is_a_release() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        receive(slot, &[NOTE_ON, 60, 0]);
        let notes = bus().take_notes();
        assert_eq!(notes.len(), 1);
        assert!(!notes[0].on, "velocity zero is a note-off");
    }

    #[test]
    fn a_note_off_is_a_release_too() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        receive(slot, &[NOTE_OFF, 60, 64]);
        assert!(!bus().take_notes()[0].on);
    }

    /// A queue nobody is draining is a scheduler that has stopped, and what
    /// that must not become is memory growing until the app dies.
    #[test]
    fn a_queue_nobody_drains_drops_its_oldest_rather_than_growing() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        bus().forget_notes();
        for i in 0..MAX_QUEUED + 50 {
            receive(slot, &[NOTE_ON, (i % 128) as u8, 100]);
        }
        assert_eq!(bus().take_notes().len(), MAX_QUEUED);
    }

    /// Nothing here reads clock or sysex yet, and a message nothing understands
    /// must be dropped rather than mistaken for one that is understood.
    #[test]
    fn a_message_this_does_not_read_is_ignored() {
        let _bus = exclusive();
        let slot = slot_for(&Selector::Name("keys".into())).unwrap();
        receive(slot, &[0xF8]);
        receive(slot, &[0xC0, 5]);
        receive(slot, &[]);
        assert!(bus().take_notes().is_empty());
    }
}

/// Which continuous thing a [`ControlNode`] is reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Control {
    /// One numbered controller: a knob, a wheel, a pedal.
    Controller(u8),
    /// The pitch wheel, which rests at zero rather than at the bottom.
    Bend,
    /// Channel pressure — how hard the keys are being leant on.
    Pressure,
}

/// How long a controller takes to travel most of the way to a new value.
///
/// A controller is seven bits arriving a few hundred times a second, which is
/// a staircase: wired straight to a filter cutoff it zippers, audibly, and the
/// zipper is the *steps* rather than the speed. Ten milliseconds is under the
/// point where a knob stops feeling connected to the hand turning it, and well
/// over the point where the steps stop being separately audible.
///
/// Smoothing lives in the node rather than in the bus because it is a
/// per-sample, per-read-site business — the bus holds what arrived, which is
/// the truth, and each place that reads it decides what to do between arrivals.
const SMOOTHING_SECS: f32 = 0.010;

/// A controller, a pitch wheel or channel pressure, as a graph node.
///
/// - No inputs.
/// - Output 0: the value, smoothed and mapped into the range asked for.
///
/// Unlike [`crate::audio_in::InputNode`] this is **not** stateless: it carries
/// the smoothing filter's one sample of memory. That matters where a voice is
/// concerned — the scheduler builds one of these per note, so a `cc` inside an
/// instrument starts each note at the controller's current value rather than
/// sliding up to it from wherever the last note left off, which is what a
/// shared filter would do and is not what anybody means.
#[derive(Clone)]
pub struct ControlNode {
    slot: usize,
    channel: u8,
    control: Control,
    /// The bottom and top of what this reads out as. `0..1` unless a program
    /// asked for something else; `-1..1` for a wheel.
    lo: f32,
    hi: f32,
    /// The smoothed value, in the source's own 0..1 or -1..1 — mapped on the
    /// way out, so changing the range does not jump the filter.
    held: f32,
    /// How much of the distance is closed per sample, from the rate.
    coeff: f32,
    /// True until the first sample, which is taken rather than approached: a
    /// node built in the middle of a performance should start where the knob
    /// already is, not slide up to it from zero over ten milliseconds.
    fresh: bool,
}

impl ControlNode {
    pub fn new(slot: usize, channel: u8, control: Control, lo: f32, hi: f32) -> ControlNode {
        ControlNode {
            slot,
            channel,
            control,
            lo,
            hi,
            held: 0.0,
            coeff: 1.0,
            fresh: true,
        }
    }

    /// What the wire currently says, before smoothing and before mapping.
    #[inline]
    fn target(&self) -> f32 {
        let bus = bus();
        match self.control {
            Control::Controller(number) => bus.controller(self.slot, self.channel, number),
            Control::Bend => bus.bend(self.slot, self.channel),
            Control::Pressure => bus.pressure(self.slot, self.channel),
        }
    }

    #[inline]
    fn next(&mut self) -> f32 {
        let target = self.target();
        if self.fresh {
            self.fresh = false;
            self.held = target;
        } else {
            self.held += (target - self.held) * self.coeff;
        }
        // A wheel already reads -1..=1, so its own range is what the map is
        // *from*; everything else is 0..=1. Written as one expression because
        // the two differ only in where zero sits.
        let unit = match self.control {
            Control::Bend => (self.held + 1.0) * 0.5,
            _ => self.held,
        };
        self.lo + unit * (self.hi - self.lo)
    }
}

impl AudioNode for ControlNode {
    // Beside `InputNode` at 201 and `SampleReader`, from the far end of
    // fundsp's own range.
    const ID: u64 = 202;
    type Inputs = typenum::U0;
    type Outputs = typenum::U1;

    fn set_sample_rate(&mut self, sample_rate: f64) {
        // One pole, from the time constant: the fraction of the remaining
        // distance to close each sample so that most of it is gone after
        // `SMOOTHING_SECS`.
        self.coeff = 1.0 - (-1.0 / (SMOOTHING_SECS * sample_rate as f32)).exp();
    }

    fn reset(&mut self) {
        // Back to taking the first sample whole. A reset node is one about to
        // be used somewhere new, and sliding in from the old value would be a
        // sweep nobody wrote.
        self.fresh = true;
    }

    #[inline]
    fn tick(&mut self, _input: &Frame<f32, Self::Inputs>) -> Frame<f32, Self::Outputs> {
        [self.next()].into()
    }

    fn process(&mut self, size: usize, _input: &BufferRef, output: &mut BufferMut) {
        for i in 0..size {
            let v = self.next();
            output.set_f32(0, i, v);
        }
    }

    fn route(&mut self, input: &SignalFrame, _frequency: f64) -> SignalFrame {
        // Whatever a hand outside is doing, which is unrelated to anything in
        // the graph — a generator, exactly as `input(channel)` is.
        Routing::Generator(0.0).route(input, self.outputs())
    }
}
