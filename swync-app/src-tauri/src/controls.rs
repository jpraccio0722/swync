//! Controls a program declares and the panel draws.
//!
//! `slider("cutoff", 200, 5000)` is a number a hand outside the program moves,
//! read from inside the graph at audio rate. That makes this
//! [`crate::midi::input`]'s neighbour rather than an invention of its own: the
//! same atomics written by one side and read by the other, the same interning
//! of a name to a slot because the audio callback cannot hash a string, the
//! same rule that declaring one touches no hardware and no window. What
//! differs is only whose hand is on it — a knob on a desk is `cc`, and a
//! slider on the screen is this.
//!
//! ## The program declares it; the panel only draws it
//!
//! There is no "add a slider" button, and that is the whole design. A slider
//! exists because a program wrote one, at the place it is used, so the control
//! and the thing it controls are the same line of text — and a piece that
//! travels takes its controls with it rather than arriving with a panel
//! somebody has to rebuild from memory. It is the same claim `midiout` makes
//! about a port and the opposite of the one an audio device makes: which synth
//! a part is for belongs to the piece, and which interface is on the desk does
//! not.
//!
//! ## Read off the text, not out of the lowerer
//!
//! [`declare_in`] walks the *syntax*, for the reason [`crate::samples`] walks
//! it looking for `load`: an instrument's body is not lowered until the
//! scheduler builds a voice from it, so a slider written inside a `fn` would
//! otherwise reach the panel only once a note had played — or never, for an
//! instrument nobody triggers tonight. The panel draws what the program says,
//! and what the program says is a fact about the text.
//!
//! ## Slots are the session's memory
//!
//! A name is interned once and keeps its slot and its position for the life of
//! the process. That is not an implementation detail — it is what makes a
//! slider survive a re-eval, which is the only thing that makes one usable
//! while live coding. You dial in a filter, edit the line above it, evaluate,
//! and the filter is where you left it, because the name found the slot it
//! already had. Quitting forgets, and the value written in the program is what
//! a fresh session starts from.
//!
//! Interning also **clamps** a remembered position into the range the program
//! now asks for, since an edited range can leave last minute's position
//! outside it — and a slider reading past its own top is a lie the panel has
//! no way to draw.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use fundsp::prelude64::*;
use serde::Serialize;

use crate::parser::parser::{Arg, Expr, SwyncItem};

/// The names, which are also what a diagnostic calls them.
pub const SLIDER: &str = "slider";
pub const TOGGLE: &str = "toggle";
pub const TRIGGER: &str = "trigger";

/// Which control a name declares.
///
/// One table holds both, because everything that makes a control a control is
/// the same for either: a slot, a position an atomic holds, a session that
/// remembers it, a node that reads it at audio rate. What differs is only the
/// travel — a slider has a range and a toggle has two ends — and the panel is
/// the only thing that has to care, since it is the only thing that draws one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// `slider(name, lo, hi, start)` — anywhere along its range.
    Slider,
    /// `toggle(name, start)` — off or on, which is 0 or 1.
    ///
    /// Not a slider with a coarse step, and the difference is what it *means*
    /// rather than how it moves: a toggle answers a question the piece asked
    /// with two answers, so a panel draws it as a switch and a program reads it
    /// as the amount it is worth. Multiplying by one is the ordinary use.
    Toggle,
    /// `trigger(name, section)` — a button that starts a section when pressed.
    ///
    /// The odd one out, and the only one that is not a number: it answers with
    /// nothing, and what a program does with it is not read but *written* —
    /// the section named beside it. So it has no position, and what its slot
    /// holds instead is the bar it was last armed at. See [`armed_at`].
    Trigger,
}

/// What a slider covers when the program does not say: the range every other
/// amount in the language is in.
pub const DEFAULT_LO: f64 = 0.0;
pub const DEFAULT_HI: f64 = 1.0;

/// How many distinct sliders one session may have.
///
/// A memory figure rather than a musical one, like
/// [`crate::midi::input::MAX_PORTS`]: the table is allocated once for all of
/// them and a slot is a handful of bytes, so this is generous by orders of
/// magnitude and still bounded. It counts a *session* rather than a program,
/// because slots are never released — a program edited all evening keeps every
/// name it has ever used. Sixty-four is more sliders than a panel can show
/// without scrolling past usefulness, and a sixty-fifth is refused with a
/// warning rather than silently sharing somebody else's slot.
pub const MAX_SLIDERS: usize = 64;

/// A trigger nobody has pressed.
///
/// Infinity rather than a flag beside the number, because every question the
/// scheduler asks of it is "has this bar come yet?" — and against infinity the
/// answer is no, forever, without a branch anywhere to forget.
pub const NEVER: f64 = f64::INFINITY;

/// What a node holds when the slider it named could not be given a slot. Reads
/// as zero forever, which is what a program declaring a sixty-fifth slider
/// should hear.
pub const NO_SLOT: usize = usize::MAX;

/// How long a slider takes to travel most of the way to a new position.
///
/// The same ten milliseconds [`crate::midi::input`] smooths a controller over,
/// and for the same reason rather than by imitation: a pointer drag arrives as
/// a few dozen positions a second, which wired straight to a cutoff is a
/// staircase, and a staircase zippers. Slower than the pointer, faster than a
/// hand can notice the control lagging it.
const SMOOTHING_SECS: f32 = 0.010;

/// What one slot is, apart from where it is standing.
///
/// The shape the *current* program gives this name. Rewritten by every
/// [`declare_in`], because a range is edited like any other part of a program;
/// what survives an edit is the position, which lives in the atomic beside it.
#[derive(Clone, Debug, PartialEq)]
struct Shape {
    name: String,
    kind: Kind,
    lo: f64,
    hi: f64,
    start: f64,
}

impl Kind {
    /// The name a program writes to declare one, which is also what a
    /// diagnostic has to call it.
    pub fn written(self) -> &'static str {
        match self {
            Kind::Slider => SLIDER,
            Kind::Toggle => TOGGLE,
            Kind::Trigger => TRIGGER,
        }
    }

    /// One written out, for a message that has to show rather than describe.
    fn example(self) -> &'static str {
        match self {
            Kind::Slider => "slider(\"cutoff\", 200, 5000)",
            Kind::Toggle => "toggle(\"mute\")",
            Kind::Trigger => "trigger(\"fill\", fill)",
        }
    }
}

/// Every control this session has: where each one is, and what the last run
/// said about them. One per process — see [`controls`].
pub struct Controls {
    /// Each slot's position, in the slider's own units, as `f32::to_bits`.
    ///
    /// Written by the panel on the main thread and read by the audio callback,
    /// so it is atomic and never locked against — exactly as a controller's
    /// value is. Allocated once and never resized, because what indexes it is
    /// held by a graph node.
    at: Vec<AtomicU32>,
    /// Whether this slot has been read as a **number** rather than as a signal
    /// — see [`Control::baked`].
    ///
    /// A flag on the slot rather than something a pass returns, because the
    /// two places that discover it are far apart and on different threads: the
    /// lowerer, when a number is demanded of one, and the realizer, when a
    /// parameter is baked into a unit at build. An instrument's voice is
    /// realized on the scheduler's thread, long after the run that published
    /// it, so a slider used only inside a `fn` becomes marked the first time a
    /// note plays — late, but never wrong, and the alternative is a return
    /// value neither of those callers has anywhere to hand back to.
    baked: Vec<AtomicBool>,
    /// The bar each trigger was last armed at, as `f64::to_bits`.
    ///
    /// [`NEVER`] until somebody presses it, which reads as a window that never
    /// opens — every comparison against it is false, so a section nobody has
    /// fired simply does not sound and needs no second flag to say so.
    ///
    /// Written by the press on the main thread and read by the scheduler on
    /// its own, so it is atomic like a position. It is the *whole* of what the
    /// scheduler learns from a button: one number, and the arithmetic it
    /// already does with the origin.
    armed: Vec<AtomicU64>,
    /// What each slot is, in slot order. Also the interning table: a name's
    /// slot is its position in here. Only the setup path touches it, never the
    /// audio callback.
    shapes: Mutex<Vec<Shape>>,
    /// How many slots have been handed out.
    taken: AtomicUsize,
    /// The slots the last successful run declared, in the order it wrote them.
    ///
    /// Separate from the table above because the two answer different
    /// questions: the table is every name this *session* has seen and is what
    /// keeps a position across an eval, while this is what the program on
    /// screen now asks for and is what the panel draws. Deleting a slider from
    /// the program takes it off the panel at the next run and leaves its
    /// remembered position alone, so putting the line back puts the control
    /// back where it was.
    declared: Mutex<Vec<usize>>,
}

/// One slider, as the program declared it and the panel draws it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Control {
    /// What the program called it, which is also what the panel labels it and
    /// what the editor searches for when the label is clicked.
    pub name: String,
    /// Which control this is, and so which way the panel draws it.
    pub kind: Kind,
    pub slot: usize,
    pub lo: f64,
    pub hi: f64,
    /// Where the program says it starts — used only the first time a session
    /// sees this name, since after that the slot remembers.
    pub start: f64,
    /// Where it is now. Filled fresh from the atomic whenever the panel asks,
    /// so what is drawn is what the graph is reading rather than what some
    /// earlier run happened to record.
    pub at: f64,
    /// True when this slider has been read as a **number** somewhere — a delay
    /// time, an envelope length, a pattern's rate — rather than only as a
    /// signal.
    ///
    /// Such a reading is baked in when the program compiles: what the graph
    /// holds is the number the slider stood at, and no amount of dragging
    /// changes it. So the panel marks these, and moving one runs the program
    /// again when the drag ends. `App.tsx` owns that decision, and the reason
    /// it waits for the end of the drag is that a re-eval crossfades the graph
    /// — doing it per pointer event would stutter the music continuously.
    pub baked: bool,
}

pub fn controls() -> &'static Controls {
    static CONTROLS: OnceLock<Controls> = OnceLock::new();
    CONTROLS.get_or_init(|| Controls {
        at: (0..MAX_SLIDERS).map(|_| AtomicU32::new(0)).collect(),
        baked: (0..MAX_SLIDERS).map(|_| AtomicBool::new(false)).collect(),
        armed: (0..MAX_SLIDERS).map(|_| AtomicU64::new(NEVER.to_bits())).collect(),
        shapes: Mutex::new(Vec::new()),
        taken: AtomicUsize::new(0),
        declared: Mutex::new(Vec::new()),
    })
}

/// Where a slot stands, in the slider's own units.
///
/// Called per sample from the audio callback, so it is one relaxed load. A
/// slot past the end of the table answers zero rather than panicking, which is
/// what [`NO_SLOT`] relies on.
#[inline]
pub fn position(slot: usize) -> f32 {
    match controls().at.get(slot) {
        Some(cell) => f32::from_bits(cell.load(Ordering::Relaxed)),
        None => 0.0,
    }
}

/// Move a slider, from the panel.
///
/// Clamped into the range the program gave it, since a position outside it is
/// one nothing could have asked for. Answers whether the name is one this
/// session knows: an unknown name is not an error worth raising anywhere,
/// because the panel draws what the last run declared and a run in between may
/// have taken it away.
pub fn set(name: &str, value: f64) -> bool {
    let controls = controls();
    let Ok(shapes) = controls.shapes.lock() else {
        return false;
    };
    let Some(slot) = shapes.iter().position(|s| s.name == name) else {
        return false;
    };
    let shape = &shapes[slot];
    let clamped = value.clamp(shape.lo.min(shape.hi), shape.lo.max(shape.hi));
    controls.at[slot].store((clamped as f32).to_bits(), Ordering::Relaxed);
    true
}

/// The bar a trigger's section starts from, or [`NEVER`].
///
/// Read by the scheduler on every pass, so it is one relaxed load. A slot past
/// the end of the table answers `NEVER`, which is what a binding pointing at
/// nothing should sound like.
#[inline]
pub fn armed_at(slot: usize) -> f64 {
    match controls().armed.get(slot) {
        Some(cell) => f64::from_bits(cell.load(Ordering::Relaxed)),
        None => NEVER,
    }
}

/// Which bar a press should arm, given where the transport is.
///
/// The next beat — but the next beat **the scheduler has not already been
/// asked about**, which is not always the same one. It works a lookahead ahead
/// of the audio clock, so a beat inside that horizon has already been decided:
/// its notes were computed as not sounding, and some were pushed. Arming there
/// would ask a window to open in the past, and what came out would be a section
/// that started late and clipped.
///
/// So the horizon is crossed first and the grid is found after. At ordinary
/// tempos a beat is far longer than the lookahead and this is simply the next
/// beat; at fast ones it is occasionally the beat after, which is the honest
/// cost of quantising something that has to be decided before it is heard.
///
/// Pure, and separate from [`press`], because it is the part worth testing and
/// the part that needs no clock to test.
pub fn arm_at(now_bars: f64, beat_bars: f64, lookahead_bars: f64) -> f64 {
    if !(beat_bars > 0.0) || !beat_bars.is_finite() {
        // A signature nothing can be divided by. The horizon is still an
        // honest answer — it starts the section as soon as it can be heard.
        return now_bars + lookahead_bars.max(0.0);
    }
    let horizon = now_bars + lookahead_bars.max(0.0);
    let beats = (horizon / beat_bars).ceil();
    // A horizon landing exactly on a beat is already that beat's, and the
    // scheduler has been asked about it — so take the next one.
    let beats = if beats * beat_bars <= horizon { beats + 1.0 } else { beats };
    beats * beat_bars
}

/// Press a trigger: arm its section from `at_bar`.
///
/// The bar is decided by the caller, because deciding it needs the clock and
/// the meter and this module has neither. See `lib.rs`, which quantises it —
/// and which has to quantise past the scheduler's lookahead, not merely onto
/// the grid, for the reason [`crate::pattern::patterns::Patterns::windows`]
/// gives: an answer the scheduler has already given must not change.
///
/// Pressing again simply overwrites, which is what makes a second press
/// restart the section rather than queue behind it. What it cannot take back
/// is a voice already pushed into the sequencer — up to a lookahead of the old
/// pass may still sound past the new one's start, which is the difference
/// between cutting a section off and never having played it.
///
/// Answers whether the name is a trigger this session knows. An unknown name
/// is not an error worth raising: the panel draws what the last run declared,
/// and a run in between may have taken it away.
pub fn press(name: &str, at_bar: f64) -> bool {
    let controls = controls();
    let Ok(shapes) = controls.shapes.lock() else {
        return false;
    };
    let Some(slot) = shapes.iter().position(|s| s.name == name) else {
        return false;
    };
    if shapes[slot].kind != Kind::Trigger {
        return false;
    }
    controls.armed[slot].store(at_bar.to_bits(), Ordering::Relaxed);
    true
}

/// Say that this slot's value has been read as a number rather than as a
/// signal — see [`Control::baked`].
pub fn mark_baked(slot: usize) {
    if let Some(cell) = controls().baked.get(slot) {
        cell.store(true, Ordering::Relaxed);
    }
}

/// Forget which sliders were baked, at the start of a run.
///
/// What a *program* bakes is a fact about the program, so an edit that stops
/// reading a slider as a number has to be able to clear the mark. The window
/// between this and the run finishing is not locked against the scheduler
/// marking one from a voice it is building — the worst that can come of that
/// race is a mark lost until the next note, which sets it again.
pub fn clear_baked() {
    for cell in &controls().baked {
        cell.store(false, Ordering::Relaxed);
    }
}

/// The slot and shape this name already has, if the program has declared it.
///
/// What the lowerer asks, so that a name written twice lowers to one slider
/// with one range — the first declaration's. Answering from the table rather
/// than from what is written at this call site is what makes "the first one
/// wins" true of the graph and not only of the warning.
pub fn shape_of(name: &str) -> Option<(usize, Kind, f64, f64, f64)> {
    let shapes = controls().shapes.lock().ok()?;
    let slot = shapes.iter().position(|s| s.name == name)?;
    let shape = &shapes[slot];
    Some((slot, shape.kind, shape.lo, shape.hi, shape.start))
}

/// Give this name a slot and this shape, keeping wherever it is already
/// standing.
///
/// Touches no window and no audio thread, which is what lets a test lower a
/// program full of sliders and lets a program compile the same on a machine
/// where the panel has never been opened — the same rule interning a MIDI port
/// follows, for the same reason.
///
/// A name already interned keeps the position it is holding, clamped into the
/// range being asked for now. A new one starts where the program says. `None`
/// when every slot is taken.
pub fn declare(name: &str, kind: Kind, lo: f64, hi: f64, start: f64) -> Option<usize> {
    // The same complaint `midi::input::slot_for` makes, for the same reason:
    // interning without the guard leaks a slot into whatever test is running
    // beside this one, and the failure lands on that test rather than on this
    // one. Asked before the table is locked, so a panic cannot poison it.
    #[cfg(test)]
    assert!(
        guarded(),
        "interning control {name:?} without holding `controls::exclusive()`. A test that \
         lowers a program declaring a slider or a toggle has to hold the guard — see \
         `exclusive` — or the slots it takes leak into whatever is running beside it.",
    );

    let controls = controls();
    let mut shapes = controls.shapes.lock().ok()?;
    let shape = Shape { name: name.to_string(), kind, lo, hi, start };

    if let Some(slot) = shapes.iter().position(|s| s.name == name) {
        let held = f32::from_bits(controls.at[slot].load(Ordering::Relaxed)) as f64;
        let clamped = held.clamp(lo.min(hi), lo.max(hi));
        if clamped != held {
            controls.at[slot].store((clamped as f32).to_bits(), Ordering::Relaxed);
        }
        shapes[slot] = shape;
        return Some(slot);
    }

    if shapes.len() >= MAX_SLIDERS {
        return None;
    }
    shapes.push(shape);
    let slot = shapes.len() - 1;
    controls.at[slot].store((start as f32).to_bits(), Ordering::Relaxed);
    controls.taken.store(shapes.len(), Ordering::Release);
    Some(slot)
}

/// Every slider a program declares, in the order it writes them.
///
/// A syntax walk — see the module note, and [`crate::parser::walk`]. What it
/// cannot read it leaves alone: a `slider` call whose name is not written out,
/// or whose range is not written in numbers, is not skipped quietly so much as
/// left to the lowerer, which refuses it in a compile error with the line in
/// front of the reader. Saying it twice would be saying it worse.
///
/// The second answer is what to put in the problems panel: a name declared
/// twice with different settings, and a program wanting more sliders than
/// there are slots. Both are warnings rather than errors, for the reason a
/// missing MIDI port is one — they are things worth knowing about a program
/// that ran, and refusing to make sound over either would be the wrong trade.
pub fn declare_in(items: &[SwyncItem]) -> (Vec<Control>, Vec<String>) {
    let mut found: Vec<Control> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    crate::parser::walk::calls_in(items, &mut |func, args| {
        let kind = match func.0.as_str() {
            SLIDER => Kind::Slider,
            TOGGLE => Kind::Toggle,
            TRIGGER => Kind::Trigger,
            _ => return,
        };
        // A trigger's second argument is its section, which this pass has no
        // business reading — only the name, which is the first. The lowerer
        // refuses a call shaped wrongly, with the line in front of the reader.
        let read = if kind == Kind::Trigger { &args[..Ord::min(args.len(), 1)] } else { args };
        let Ok(decl) = parse(kind, read) else { return };

        if let Some(first) = found.iter().find(|c| c.name == decl.name) {
            // One name is one control wherever it is written — that is what
            // makes `slider("cutoff")` in two instruments one control moving
            // both, which is usually exactly what was meant. What cannot be
            // honoured twice is the *shape*, so the first one wins and the
            // second is worth a word: a range typed differently in two places
            // is as often a stale copy as a decision.
            //
            // A name declared once as a slider and once as a toggle is the
            // same collision and gets the same answer, since the two share a
            // slot and a slot has one travel. The message names both kinds,
            // because "declared twice" would send somebody looking for a
            // second `slider` that is not there.
            if first.kind != decl.kind {
                warnings.push(format!(
                    "\"{}\" is declared as a {} and also as a {} — the {} is the one \
                     being used, since it is written first. One name is one control, so \
                     give the other a name of its own.",
                    decl.name, first.kind.written(), decl.kind.written(),
                    first.kind.written()));
            } else if (first.lo, first.hi, first.start) != (decl.lo, decl.hi, decl.start) {
                warnings.push(format!(
                    "{}(\"{}\") is declared more than once with different settings — \
                     {} to {} starting at {} is the one being used, since it is written \
                     first. One name is one control, so give the other a name of its own \
                     or make the two agree.",
                    decl.kind.written(), decl.name, first.lo, first.hi, first.start));
            }
            return;
        }

        match declare(&decl.name, decl.kind, decl.lo, decl.hi, decl.start) {
            Some(slot) => found.push(Control {
                name: decl.name,
                kind: decl.kind,
                slot,
                lo: decl.lo,
                hi: decl.hi,
                start: decl.start,
                at: position(slot) as f64,
                baked: false,
            }),
            None => warnings.push(format!(
                "{}(\"{}\"): a session may have {MAX_SLIDERS} controls and this is one \
                 more, so it will read as zero and stay off the panel. Nothing else \
                 about the program changes.",
                decl.kind.written(), decl.name)),
        }
    });

    (found, warnings)
}

/// What the program that just compiled asks the panel to draw.
///
/// Published by `run_code` on the success path only, beside the graph and the
/// patterns: a program that did not compile leaves the panel alone for the
/// same reason it leaves the music alone.
pub fn publish(sliders: &[Control]) {
    if let Ok(mut declared) = controls().declared.lock() {
        *declared = sliders.iter().map(|s| s.slot).collect();
    }
}

/// What the panel draws: the last run's sliders, each carrying where it
/// actually stands and whether it is one a drag can be heard through.
pub fn declared() -> Vec<Control> {
    let controls = controls();
    let (Ok(declared), Ok(shapes)) = (controls.declared.lock(), controls.shapes.lock()) else {
        return Vec::new();
    };
    declared
        .iter()
        .filter_map(|&slot| {
            let shape = shapes.get(slot)?;
            Some(Control {
                name: shape.name.clone(),
                kind: shape.kind,
                slot,
                lo: shape.lo,
                hi: shape.hi,
                start: shape.start,
                at: position(slot) as f64,
                baked: controls.baked[slot].load(Ordering::Relaxed),
            })
        })
        .collect()
}

/// Whether this name has been read as a number rather than as a signal —
/// [`Control::baked`], asked by name rather than by slot.
///
/// Only tests ask it this way; the panel gets it with everything else in
/// [`declared`].
#[cfg(test)]
pub(crate) fn baked_by_name(name: &str) -> bool {
    match shape_of(name) {
        Some((slot, ..)) => controls().baked[slot].load(Ordering::Relaxed),
        None => false,
    }
}

/// A `slider` call read off the syntax.
#[derive(Debug)]
pub struct Decl {
    pub name: String,
    pub kind: Kind,
    pub lo: f64,
    pub hi: f64,
    pub start: f64,
}

/// `slider(name)`, `slider(name, lo, hi)`, `slider(name, lo, hi, start)`, and
/// `toggle(name)`, `toggle(name, start)`.
///
/// Read off the arguments **as written** rather than as evaluated, and both
/// callers need it that way: the declaration pass runs before anything has
/// been evaluated at all, and the lowerer must agree with it to the last digit
/// or the panel would be drawing a control the graph does not have. So the
/// range is written in numbers, like a `load` path is written in text — a
/// computed range is refused rather than silently taken from one of the two
/// readings.
///
/// The name is first for both, because it is the only part that is never
/// optional: a control with no name is one nothing can label, nothing can find
/// twice, and nothing can remember the position of.
pub fn parse(kind: Kind, args: &[Arg]) -> Result<Decl, String> {
    let called = kind.written();

    if let Some(named) = args.iter().find(|a| a.name.is_some()) {
        let name = named.name.as_ref().map(|n| n.0.as_str()).unwrap_or_default();
        return Err(format!(
            "{called}: `{name}:` is a named argument, and a control's parts are written \
             in order — {}", kind.example()));
    }

    let Some((first, rest)) = args.split_first() else {
        return Err(format!("{called} expects a name: {}", kind.example()));
    };
    let Expr::Str(name) = &first.value else {
        return Err(format!(
            "{called}: the name is written in quotes — {called}(\"cutoff\"). It is what \
             the panel labels the control and what makes it the same control after an \
             edit, so it cannot be worked out while the program runs"));
    };
    if name.trim().is_empty() {
        return Err(format!(
            "{called}: the name is what labels the control, so it cannot be empty"));
    }

    match kind {
        // A trigger takes a section, and a section is not a value: it is
        // lowered where it is *placed*, which is the lowerer's business and
        // not this pass's. All this needs off the syntax is the name, so the
        // panel can draw a button before anything has run.
        Kind::Trigger => Ok(Decl { name: name.clone(), kind, lo: 0.0, hi: 1.0, start: 0.0 }),

        Kind::Toggle => {
            // Two ends, so there is no range to write — only which one it
            // starts at, and only as `0` or `1`. Anything else is a number
            // somebody expected to mean something, and guessing which end
            // they meant would be guessing at the sound.
            let start = match rest {
                [] => 0.0,
                [start] => match number(TOGGLE, &start.value, "the starting state")? {
                    n if n == 0.0 || n == 1.0 => n,
                    n => return Err(format!(
                        "{TOGGLE}(\"{name}\"): a toggle is off or on, so it starts at 0 or \
                         1 — got {n}")),
                },
                _ => return Err(format!(
                    "{TOGGLE} takes a name and which end it starts at — \
                     {TOGGLE}(\"{name}\", 1) starts it on. A toggle has no range to give \
                     it, since its ends are 0 and 1")),
            };
            Ok(Decl { name: name.clone(), kind, lo: 0.0, hi: 1.0, start })
        }

        Kind::Slider => {
            let (lo, hi) = match rest {
                [] => (DEFAULT_LO, DEFAULT_HI),
                [lo, hi, ..] => (
                    number(SLIDER, &lo.value, "lo")?,
                    number(SLIDER, &hi.value, "hi")?,
                ),
                [_] => return Err(format!(
                    "{SLIDER}: a range is both ends or neither — {SLIDER}(\"cutoff\", 200, \
                     5000) covers a filter, and no range at all is {DEFAULT_LO} to \
                     {DEFAULT_HI}")),
            };
            if !(hi > lo) {
                return Err(format!(
                    "{SLIDER}(\"{name}\"): the range {lo} to {hi} is empty. A slider needs \
                     somewhere to travel, so the top has to be above the bottom"));
            }

            // Where it starts is where it starts *the first time this session
            // sees the name*; after that the control remembers, which is the
            // point of it. So the default is the bottom of the range rather
            // than the middle: a control nobody has touched yet should be
            // doing nothing, and for a level, a send or a depth the bottom is
            // nothing.
            let start = match rest {
                [_, _, start] => number(SLIDER, &start.value, "start")?,
                [_, _, _, extra, ..] => return Err(format!(
                    "{SLIDER} takes a name, a range and where it starts — {} is one \
                     argument too many. {SLIDER}(\"cutoff\", 200, 5000, 800) is all of it",
                    described(&extra.value))),
                _ => lo,
            };
            if start < lo || start > hi {
                return Err(format!(
                    "{SLIDER}(\"{name}\"): it starts at {start}, which is outside the {lo} \
                     to {hi} it can travel"));
            }
            Ok(Decl { name: name.clone(), kind, lo, hi, start })
        }
    }
}

/// A number written at the call, which is the only kind there is here.
fn number(called: &str, e: &Expr, what: &str) -> Result<f64, String> {
    let n = match e {
        Expr::Num(n) => *n,
        Expr::Neg { expr } => match expr.as_ref() {
            Expr::Num(n) => -n,
            _ => return Err(written_out(called, what)),
        },
        _ => return Err(written_out(called, what)),
    };
    if !n.is_finite() {
        return Err(format!("{called}: {what} must be a real number, got {n}"));
    }
    Ok(n)
}

fn written_out(called: &str, what: &str) -> String {
    format!(
        "{called}: {what} is written out as a number — {SLIDER}(\"cutoff\", 200, 5000). The \
         panel is drawn before the program is run, so a control it would have to run the \
         program to describe is one it cannot draw")
}

/// A short noun for what was written where nothing should have been, so the
/// message can name it rather than say "something".
fn described(e: &Expr) -> &'static str {
    match e {
        Expr::Num(_) | Expr::Neg { .. } => "a fifth number",
        Expr::Str(_) => "a second name",
        _ => "a fifth argument",
    }
}

/// A slider, as a graph node.
///
/// - No inputs.
/// - Output 0: the position, smoothed.
///
/// No range of its own, unlike [`crate::midi::input::ControlNode`]: what the
/// slot holds is already in the slider's own units, because the panel knows
/// the range it is drawing and there is nothing to be gained by mapping the
/// same numbers twice. A controller has to be mapped in its node because seven
/// bits on a wire mean nothing until something says what they are of.
///
/// Not stateless, for the reason a control node is not: it carries the
/// smoothing filter's one sample of memory, and the scheduler builds one of
/// these per note — so a slider inside an instrument starts each note where
/// the slider is rather than sliding up to it from wherever the last note left
/// off.
#[derive(Clone)]
pub struct PanelNode {
    slot: usize,
    held: f32,
    /// How much of the distance is closed per sample, from the rate.
    coeff: f32,
    /// True until the first sample, which is taken whole rather than
    /// approached: a node built mid-performance should start where the slider
    /// already is, not sweep up to it over ten milliseconds.
    fresh: bool,
}

impl PanelNode {
    pub fn new(slot: usize) -> PanelNode {
        PanelNode { slot, held: 0.0, coeff: 1.0, fresh: true }
    }

    #[inline]
    fn next(&mut self) -> f32 {
        let target = position(self.slot);
        if self.fresh {
            self.fresh = false;
            self.held = target;
        } else {
            self.held += (target - self.held) * self.coeff;
        }
        self.held
    }
}

impl AudioNode for PanelNode {
    // Beside `InputNode` at 201 and `ControlNode` at 202, from the far end of
    // fundsp's own range.
    const ID: u64 = 203;
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
        // the graph — a generator, exactly as a controller and the live input
        // are.
        Routing::Generator(0.0).route(input, self.outputs())
    }
}

/// The lock [`exclusive`] hands out, at module scope so that [`declare`] can
/// ask whether a test is holding it.
#[cfg(test)]
static EXCLUSIVE: Mutex<()> = Mutex::new(());

/// Whether some test is holding [`exclusive`].
///
/// Only a *free* lock is an answer worth acting on: one held by this thread
/// and one held by another are both `Err` here, and neither is a bug.
#[cfg(test)]
fn guarded() -> bool {
    EXCLUSIVE.try_lock().is_err()
}

/// Hold the process's one set of controls for the duration of a test.
///
/// The same guard [`crate::midi::input::exclusive`] is, for the same two
/// reasons. Slots are a singleton because a session has one panel, and the
/// suite runs in threads — so two tests with opinions about where "cutoff" is
/// would each be right half the time. And slots are finite and never released,
/// which is right for a session and wrong for a suite, so this clears the
/// table as well as taking the lock.
#[cfg(test)]
pub(crate) fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    let guard = EXCLUSIVE.lock().unwrap_or_else(|e| e.into_inner());

    let controls = controls();
    if let Ok(mut shapes) = controls.shapes.lock() {
        shapes.clear();
        controls.taken.store(0, Ordering::Release);
    }
    if let Ok(mut declared) = controls.declared.lock() {
        declared.clear();
    }
    for cell in &controls.at {
        cell.store(0.0f32.to_bits(), Ordering::Relaxed);
    }
    for cell in &controls.baked {
        cell.store(false, Ordering::Relaxed);
    }
    for cell in &controls.armed {
        cell.store(NEVER.to_bits(), Ordering::Relaxed);
    }
    guard
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parser::parse as parse_program;

    fn sliders(src: &str) -> (Vec<Control>, Vec<String>) {
        declare_in(&parse_program(src.to_string()).expect("parse failed"))
    }

    #[test]
    fn a_slider_starts_where_the_program_says_it_does() {
        let _controls = exclusive();
        let slot = declare("cutoff", Kind::Slider, 200.0, 5000.0, 800.0).unwrap();
        assert_eq!(position(slot), 800.0);
    }

    #[test]
    fn a_slider_with_no_range_covers_zero_to_one_from_the_bottom() {
        let _controls = exclusive();
        let (found, warnings) = sliders("out(sin(220) * slider(\"level\"))\n");
        assert_eq!((found[0].lo, found[0].hi, found[0].start), (0.0, 1.0, 0.0));
        assert!(warnings.is_empty(), "got: {warnings:?}");
    }

    /// The whole reason slots are interned rather than handed out per run.
    /// Dialling in a filter and then editing the line above it must not undo
    /// the dialling.
    #[test]
    fn a_slider_keeps_its_position_across_a_re_evaluation() {
        let _controls = exclusive();
        let slot = declare("cutoff", Kind::Slider, 200.0, 5000.0, 800.0).unwrap();
        set("cutoff", 3200.0);

        assert_eq!(declare("cutoff", Kind::Slider, 200.0, 5000.0, 800.0), Some(slot));
        assert_eq!(position(slot), 3200.0);
    }

    /// An edited range can leave last minute's position outside it, and a
    /// slider reading past its own top is something the panel cannot draw.
    #[test]
    fn a_remembered_position_is_clamped_into_a_range_that_has_narrowed() {
        let _controls = exclusive();
        let slot = declare("cutoff", Kind::Slider, 200.0, 5000.0, 800.0).unwrap();
        set("cutoff", 4800.0);

        declare("cutoff", Kind::Slider, 200.0, 1000.0, 800.0);
        assert_eq!(position(slot), 1000.0);
    }

    /// A range is edited like any other part of a program, so the newest
    /// declaration is the one that counts — it is only the *position* that
    /// survives an eval.
    #[test]
    fn a_range_the_program_has_edited_replaces_the_one_before_it() {
        let _controls = exclusive();
        declare("cutoff", Kind::Slider, 200.0, 5000.0, 800.0);
        declare("cutoff", Kind::Slider, 40.0, 400.0, 100.0);
        assert_eq!(shape_of("cutoff").map(|(_, _, lo, hi, _)| (lo, hi)), Some((40.0, 400.0)));
    }

    #[test]
    fn two_names_are_two_slots_and_one_name_is_one() {
        let _controls = exclusive();
        let cutoff = declare("cutoff", Kind::Slider, 0.0, 1.0, 0.0).unwrap();
        let room = declare("room", Kind::Slider, 0.0, 1.0, 0.0).unwrap();
        assert_ne!(cutoff, room);
        assert_eq!(declare("cutoff", Kind::Slider, 0.0, 1.0, 0.0), Some(cutoff));
    }

    /// Refused rather than silently sharing somebody else's slot, which would
    /// be two labels moving one control.
    #[test]
    fn a_slider_past_the_last_slot_is_refused() {
        let _controls = exclusive();
        for i in 0..MAX_SLIDERS {
            assert!(declare(&format!("s{i}"), Kind::Slider, 0.0, 1.0, 0.0).is_some());
        }
        assert_eq!(declare("one more", Kind::Slider, 0.0, 1.0, 0.0), None);
    }

    /// The point of the pass being a syntax walk: an instrument is not lowered
    /// until a note plays, and the panel cannot wait for one.
    #[test]
    fn a_slider_inside_an_instrument_is_found_before_a_note_is_played() {
        let _controls = exclusive();
        let (found, _) = sliders(
            "fn lead(freq) { sin(freq) * slider(\"lead level\", 0, 1, 0.5) }\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "lead level");
        assert_eq!(found[0].start, 0.5);
    }

    #[test]
    fn sliders_reach_the_panel_in_the_order_the_program_writes_them() {
        let _controls = exclusive();
        let (found, _) = sliders(
            "out(sin(slider(\"pitch\", 100, 800)) * slider(\"level\"))\n");
        assert_eq!(
            found.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["pitch", "level"]);
    }

    /// One name is one slider, which is what makes a slider written into two
    /// instruments one control moving both.
    #[test]
    fn one_name_written_twice_is_one_slider() {
        let _controls = exclusive();
        let (found, warnings) = sliders(
            "out(sin(220) * slider(\"level\") + saw(110) * slider(\"level\"))\n");
        assert_eq!(found.len(), 1);
        assert!(warnings.is_empty(), "got: {warnings:?}");
    }

    #[test]
    fn one_name_declared_twice_with_different_ranges_keeps_the_first_and_says_so() {
        let _controls = exclusive();
        let (found, warnings) = sliders(
            "out(lowpass(saw(110), slider(\"cutoff\", 200, 5000), 1) * slider(\"cutoff\", 0, 1))\n");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].lo, found[0].hi), (200.0, 5000.0));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("declared more than once"), "got: {}", warnings[0]);
    }

    /// The lowerer refuses these with the line in front of the reader, so the
    /// pass has nothing to add — it just does not draw a control it cannot
    /// describe.
    #[test]
    fn a_slider_the_panel_cannot_read_is_left_for_the_lowerer_to_refuse() {
        let _controls = exclusive();
        let (found, warnings) = sliders("let top = 5000\nout(slider(\"cutoff\", 200, top))\n");
        assert!(found.is_empty());
        assert!(warnings.is_empty(), "got: {warnings:?}");
    }

    #[test]
    fn a_range_that_goes_nowhere_is_refused() {
        let err = parse(Kind::Slider, &[
            Arg::positional(Expr::Str("cutoff".into())),
            Arg::positional(Expr::Num(1.0)),
            Arg::positional(Expr::Num(1.0)),
        ])
        .expect_err("an empty range should be refused");
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn a_slider_that_starts_outside_its_own_range_is_refused() {
        let err = parse(Kind::Slider, &[
            Arg::positional(Expr::Str("cutoff".into())),
            Arg::positional(Expr::Num(200.0)),
            Arg::positional(Expr::Num(5000.0)),
            Arg::positional(Expr::Num(9000.0)),
        ])
        .expect_err("a start outside the range should be refused");
        assert!(err.contains("outside"), "got: {err}");
    }

    /// A range written one end at a time is more likely a half-finished edit
    /// than a request for a default, and guessing which end was meant would be
    /// guessing at the sound.
    #[test]
    fn half_a_range_is_refused() {
        let err = parse(Kind::Slider, &[
            Arg::positional(Expr::Str("cutoff".into())),
            Arg::positional(Expr::Num(200.0)),
        ])
        .expect_err("half a range should be refused");
        assert!(err.contains("both ends or neither"), "got: {err}");
    }

    #[test]
    fn a_slider_with_no_name_is_refused() {
        let err = parse(Kind::Slider, &[]).expect_err("a nameless slider should be refused");
        assert!(err.contains("expects a name"), "got: {err}");
    }

    #[test]
    fn a_negative_range_is_read_the_way_it_is_written() {
        let decl = parse(Kind::Slider, &[
            Arg::positional(Expr::Str("bend".into())),
            Arg::positional(Expr::Neg { expr: Box::new(Expr::Num(1.0)) }),
            Arg::positional(Expr::Num(1.0)),
            Arg::positional(Expr::Num(0.0)),
        ])
        .expect("a range below zero is an ordinary range");
        assert_eq!((decl.lo, decl.hi, decl.start), (-1.0, 1.0, 0.0));
    }

    /// The panel draws where the slider *is*, not where some earlier run
    /// recorded it — otherwise every drag would be undone by the next poll.
    #[test]
    fn what_the_panel_is_told_is_where_the_slider_now_stands() {
        let _controls = exclusive();
        let (found, _) = sliders("out(lowpass(saw(110), slider(\"cutoff\", 200, 5000, 800), 1))\n");
        publish(&found);
        set("cutoff", 2000.0);

        assert_eq!(declared()[0].at, 2000.0);
    }

    /// Deleting the line takes the control off the panel and leaves the
    /// remembered position alone, so putting the line back puts the control
    /// back where it was rather than back at the start.
    #[test]
    fn a_slider_deleted_from_the_program_leaves_the_panel_but_not_the_session() {
        let _controls = exclusive();
        let (found, _) = sliders("out(sin(220) * slider(\"level\", 0, 1, 0.2))\n");
        publish(&found);
        set("level", 0.9);

        let (found, _) = sliders("out(sin(220) * 0.5)\n");
        publish(&found);
        assert!(declared().is_empty());

        let (found, _) = sliders("out(sin(220) * slider(\"level\", 0, 1, 0.2))\n");
        publish(&found);
        // Within a float of 0.9: a position is held as the `f32` the audio
        // graph reads, so it comes back as near 0.9 as that can say.
        assert!((declared()[0].at - 0.9).abs() < 1e-6, "got: {}", declared()[0].at);
    }

    /// An ordinary tempo: a beat is a quarter of a bar and the lookahead is a
    /// fraction of one, so this really is the next beat.
    #[test]
    fn a_press_is_armed_at_the_next_beat() {
        assert_eq!(arm_at(2.1, 0.25, 0.02), 2.25);
        assert_eq!(arm_at(2.3, 0.25, 0.02), 2.5);
    }

    /// The beat is already inside the horizon, so it has been decided: opening
    /// a window there would ask for notes the scheduler has already said no to.
    #[test]
    fn a_press_too_close_to_the_next_beat_takes_the_one_after() {
        assert_eq!(arm_at(2.24, 0.25, 0.05), 2.5);
    }

    /// A press landing exactly on the horizon is landing on a beat the
    /// scheduler has just been asked about, so it takes the next.
    #[test]
    fn a_press_landing_exactly_on_the_horizon_takes_the_next_beat() {
        assert_eq!(arm_at(2.0, 0.25, 0.25), 2.5);
    }

    #[test]
    fn a_trigger_nobody_has_pressed_is_armed_at_never() {
        let _controls = exclusive();
        let slot = declare("fill", Kind::Trigger, 0.0, 1.0, 0.0).unwrap();
        assert_eq!(armed_at(slot), NEVER);
    }

    #[test]
    fn pressing_a_trigger_arms_it_and_pressing_again_moves_it() {
        let _controls = exclusive();
        let slot = declare("fill", Kind::Trigger, 0.0, 1.0, 0.0).unwrap();

        assert!(press("fill", 4.0));
        assert_eq!(armed_at(slot), 4.0);

        assert!(press("fill", 9.0));
        assert_eq!(armed_at(slot), 9.0, "a second press restarts rather than queues");
    }

    /// A slider is not something that can be fired, and pressing one would be
    /// a panel asking for something the program never declared.
    #[test]
    fn only_a_trigger_can_be_pressed() {
        let _controls = exclusive();
        declare("cutoff", Kind::Slider, 200.0, 5000.0, 800.0);
        assert!(!press("cutoff", 4.0));
        assert!(!press("never written", 4.0));
    }

    /// The panel draws the program on screen, and a run may have deleted the
    /// line — so a name it no longer knows is not an error, just a no.
    #[test]
    fn moving_a_slider_this_session_has_never_seen_is_refused_quietly() {
        let _controls = exclusive();
        assert!(!set("never written", 0.5));
    }

    #[test]
    fn a_slider_cannot_be_moved_past_the_range_it_was_given() {
        let _controls = exclusive();
        let slot = declare("cutoff", Kind::Slider, 200.0, 5000.0, 800.0).unwrap();
        set("cutoff", 99999.0);
        assert_eq!(position(slot), 5000.0);
    }
}
