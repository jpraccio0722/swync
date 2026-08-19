//! Following somebody else's clock.
//!
//! The other half of `midiclock`, and deliberately not its mirror. Sending
//! clock is a fact about the piece — the arps on that synth have to line up
//! with the part written for it — so it is named in the program. *Following*
//! is a fact about the evening: the same piece is master in the studio and
//! slave when the drummer's box is running the room. So it is chosen in the
//! transport panel, beside the tempo it replaces, and nothing in the language
//! mentions it.
//!
//! ## What arrives, and what has to be made of it
//!
//! Twenty-four ticks to the quarter note, and nothing else — no tempo, no bar
//! number, no time signature. Everything this has to know is inferred from
//! *when* the ticks arrive, which means two separate problems that are easy to
//! confuse:
//!
//! - **Tempo** is how fast they are coming. Estimated from the interval and
//!   smoothed hard, because MIDI clock jitter is real: a byte can sit behind
//!   another on the wire, and one late tick must not read as a tempo change.
//! - **Phase** is where in the bar we are. It comes from `Start`, which says
//!   "bar zero, now", and from counting ticks after it. Tempo alone would
//!   drift out of phase over a few minutes even if it were exactly right.
//!
//! The two are corrected separately and at different speeds, which is the
//! whole of why this is not one number.
//!
//! ## Losing it
//!
//! A cable comes out, or the other box stops without saying so. What happens
//! then is that **nothing happens**: the transport carries on at the tempo it
//! was following, because the alternative is silence in a room, and the last
//! tempo is the best guess anybody has. [`Follow::status`] says so, and the
//! transport panel shows it — being out of sync and not knowing is worse than
//! being out of sync.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::scheduler::clock::Clock;

/// Ticks per quarter note, fixed by the specification. The same figure
/// [`crate::midi::out::PPQN`] sends at, and read here rather than shared so
/// that neither direction is written in terms of the other.
const PPQN: f64 = 24.0;

/// How many ticks the tempo is measured over: one quarter note.
///
/// Measured across a window rather than smoothed tick by tick, and the reason
/// is what MIDI jitter actually *is*. A tick is not randomly early or late
/// around a true time — it is **delayed**, behind whatever else was on the
/// wire, and the one after it then arrives at its own correct time. So a late
/// tick makes one interval long and the very next one short, and a measurement
/// spanning both is already right. Filtering each interval separately fights
/// that instead of using it: a single delayed byte moves an exponential
/// average by several bpm, where across a window it moves it by a fraction of
/// one.
///
/// A quarter note is long enough to swallow any single hiccup and short enough
/// to follow a tempo knob being turned.
const TEMPO_WINDOW: usize = 24;

/// How much of each windowed reading is taken, on top of the window.
///
/// The window does the work; this only takes the corners off the step between
/// one window and the next. High enough that a real tempo change is followed
/// within a beat of the window filling.
const TEMPO_SMOOTHING: f64 = 0.3;

/// How far the phase may drift before it is pulled back, in ticks.
///
/// Not zero, because pulling on every tick would hand the jitter straight to
/// the transport — which is the thing the tempo filter exists to keep out.
/// Half a tick at 120 bpm is ten milliseconds, under what anybody hears as a
/// flam and well over what the wire does on its own.
const PHASE_SLACK_TICKS: f64 = 0.5;

/// How much of the remaining phase error is taken once it is past the slack.
///
/// Gentle for the same reason the tempo filter is gentle: a correction that
/// happened at once would be a jump in bar time, and bar time is what every
/// pattern currently playing is placed against.
const PHASE_CORRECTION: f64 = 0.1;

/// How long without a tick before the clock counts as lost.
///
/// Three ticks at a very slow 40 bpm, which is longer than any gap a running
/// box leaves and shorter than anybody plays through without noticing. What
/// happens at the end of it is only that the panel says so — see the module
/// docs.
const LOST_AFTER: Duration = Duration::from_millis(200);

/// The slowest and fastest a following transport will be driven to.
///
/// A missed tick makes the interval look twice as long and the tempo half as
/// fast; two ticks arriving together make it look infinitely fast. Neither is
/// a tempo anybody set, and the clamp is what keeps one wire hiccup from
/// throwing the transport somewhere it takes several seconds to walk back
/// from.
const MIN_BPM: f64 = 20.0;
const MAX_BPM: f64 = 400.0;

/// No port: what [`Follow::slot`] holds while nothing is being followed.
const NO_PORT: usize = usize::MAX;

/// What the transport panel shows about a clock being followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Not following anything: swync's own tempo is the tempo.
    Internal,
    /// Following, and ticks are arriving.
    Locked,
    /// Following, and they have stopped. The transport is still running, at
    /// the tempo it last saw — see the module docs.
    Lost,
    /// Following a port that is not connected, or could not be opened.
    Missing,
}

/// One clock being followed.
///
/// Written by the MIDI callback and read by the command thread that answers
/// the panel, so everything here is either atomic or behind a mutex the audio
/// callback never touches — the same rule the rest of `midi::input` follows,
/// and for the same reason.
pub struct Follow {
    /// Whether anything is being followed at all. Read first by every message
    /// so that the ordinary machine, which follows nothing, pays one load.
    active: AtomicBool,
    /// Ticks since the last `Start`. The whole of the phase.
    ticks: AtomicUsize,
    /// Whether the sender says it is running. A tick arriving without a
    /// `Start` is a box that was already going when we began listening.
    running: AtomicBool,
    /// The smoothed interval between ticks, in seconds, as `f64::to_bits`.
    /// Zero until two ticks have arrived and there is an interval to have.
    interval: AtomicU64,
    /// When the last few ticks arrived. The newest is what [`Status::Lost`]
    /// reads; the oldest is the far end of the window the tempo is measured
    /// across. Never longer than [`TEMPO_WINDOW`] + 1.
    recent: Mutex<VecDeque<Instant>>,
    /// The transport being driven. `None` in tests that are about the
    /// arithmetic rather than about the transport.
    clock: Mutex<Option<Clock>>,
    /// The port's name, for saying whether it is still on the machine. Held
    /// here rather than read back out of the settings file, so that asking how
    /// the clock is doing does not depend on anything else being managed.
    port: Mutex<Option<String>>,
    /// The input slot whose clock is being followed, or [`NO_PORT`].
    ///
    /// Held here rather than in the bus because it is one choice for the whole
    /// app: a tick arriving on any other port is not this transport's business
    /// and is dropped without being counted.
    slot: AtomicUsize,
}

impl Default for Follow {
    fn default() -> Self {
        Follow::new()
    }
}

impl Follow {
    pub fn new() -> Follow {
        Follow {
            active: AtomicBool::new(false),
            ticks: AtomicUsize::new(0),
            running: AtomicBool::new(false),
            interval: AtomicU64::new(0),
            recent: Mutex::new(VecDeque::new()),
            clock: Mutex::new(None),
            port: Mutex::new(None),
            slot: AtomicUsize::new(NO_PORT),
        }
    }

    /// Which slot's clock is being followed.
    pub fn slot(&self) -> Option<usize> {
        match self.slot.load(Ordering::Acquire) {
            NO_PORT => None,
            slot => Some(slot),
        }
    }

    /// Follow this port's clock, or nothing.
    pub fn follow(&self, port: Option<(&str, usize)>) {
        if let Ok(mut held) = self.port.lock() {
            *held = port.map(|(name, _)| name.to_string());
        }
        self.slot.store(port.map(|(_, slot)| slot).unwrap_or(NO_PORT), Ordering::Release);
        self.set_active(port.is_some());
    }

    /// The port being followed, by name.
    pub fn port(&self) -> Option<String> {
        self.port.lock().ok().and_then(|p| p.clone())
    }

    /// The transport this drives. Set once, at startup.
    pub fn drives(&self, clock: Clock) {
        if let Ok(mut held) = self.clock.lock() {
            *held = Some(clock);
        }
    }

    /// Start or stop following. Called from the transport panel.
    ///
    /// Turning it off leaves the transport exactly where it is, at the tempo
    /// it had reached — the same thing losing the clock does, and for the same
    /// reason. Nothing about stopping to follow is a reason to stop playing.
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
        if !active {
            self.interval.store(0, Ordering::Relaxed);
            if let Ok(mut recent) = self.recent.lock() {
                recent.clear();
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// What the panel should say.
    pub fn status(&self, port_present: bool) -> Status {
        if !self.is_active() {
            return Status::Internal;
        }
        if !port_present {
            return Status::Missing;
        }
        match self.recent.lock().ok().and_then(|r| r.back().copied()) {
            Some(at) if at.elapsed() < LOST_AFTER => Status::Locked,
            _ => Status::Lost,
        }
    }

    /// The tempo currently being followed, in bpm, or `None` before enough
    /// ticks have arrived to have one.
    pub fn bpm(&self) -> Option<f64> {
        let interval = f64::from_bits(self.interval.load(Ordering::Relaxed));
        (interval > 0.0).then(|| 60.0 / (interval * PPQN))
    }

    /// One clock byte, from the MIDI callback.
    ///
    /// Returns whether it was one — a caller that gets `false` has an ordinary
    /// message to deal with instead.
    pub fn receive(&self, status: u8, at: Instant) -> bool {
        match status {
            super::input::CLOCK_TICK => {
                if self.is_active() {
                    self.tick(at);
                }
                true
            }
            super::input::CLOCK_START => {
                if self.is_active() {
                    self.start();
                }
                true
            }
            super::input::CLOCK_CONTINUE => {
                if self.is_active() {
                    self.running.store(true, Ordering::Release);
                }
                true
            }
            super::input::CLOCK_STOP => {
                if self.is_active() {
                    self.running.store(false, Ordering::Release);
                }
                true
            }
            _ => false,
        }
    }

    /// `Start`: bar zero, now.
    ///
    /// The one message that carries phase outright, so it is the one moment
    /// the transport is *moved* rather than nudged. Everything after it is
    /// counted from here.
    fn start(&self) {
        self.ticks.store(0, Ordering::Release);
        self.running.store(true, Ordering::Release);
        if let Ok(clock) = self.clock.lock() {
            if let Some(clock) = clock.as_ref() {
                clock.reset();
            }
        }
    }

    /// One tick: a twenty-fourth of a quarter note has gone by.
    fn tick(&self, at: Instant) {
        // A box that was already running when we started listening never sent
        // us a `Start`. Treating its first tick as one is better than ignoring
        // everything until it happens to stop and start again.
        if !self.running.swap(true, Ordering::AcqRel) {
            self.start();
        }

        // The window: how long the last `TEMPO_WINDOW` ticks took between
        // them, which is the measurement a late byte cannot skew.
        let spanned = {
            let Ok(mut recent) = self.recent.lock() else { return };
            recent.push_back(at);
            while recent.len() > TEMPO_WINDOW + 1 {
                recent.pop_front();
            }
            // Whatever is in hand, so that a tempo exists a few ticks in
            // rather than only after a whole quarter note.
            match (recent.front(), recent.len()) {
                (Some(oldest), len) if len >= 2 => {
                    Some((at.duration_since(*oldest).as_secs_f64(), len - 1))
                }
                _ => None,
            }
        };

        let ticks = self.ticks.fetch_add(1, Ordering::AcqRel) + 1;
        let Some((spanned, over)) = spanned else { return };
        if spanned <= 0.0 {
            return;
        }
        let measured = spanned / over as f64;

        // Filtering the interval rather than the bpm keeps a long reading from
        // pulling harder than a short one, which it would if the reciprocal
        // were taken first.
        let held = f64::from_bits(self.interval.load(Ordering::Relaxed));
        let smoothed = if held <= 0.0 {
            measured
        } else {
            held + (measured - held) * TEMPO_SMOOTHING
        };
        self.interval.store(smoothed.to_bits(), Ordering::Relaxed);

        let Ok(clock) = self.clock.lock() else { return };
        let Some(clock) = clock.as_ref() else { return };

        let bpm = (60.0 / (smoothed * PPQN)).clamp(MIN_BPM, MAX_BPM);
        clock.set_bpm(bpm);
        self.correct_phase(clock, ticks);
    }

    /// Pull bar time back towards where the tick count says it should be.
    ///
    /// Tempo alone is not enough: it is an estimate, and an estimate that is
    /// right to a part in a thousand still walks a whole beat away over a few
    /// minutes. This is what stops that, and it is deliberately weak — bar
    /// time is what every playing pattern is placed against, so a correction
    /// anybody can hear is worse than the drift it fixes.
    fn correct_phase(&self, clock: &Clock, ticks: usize) {
        let ticks_per_bar = PPQN * clock.meter().quarters_per_bar();
        if ticks_per_bar <= 0.0 {
            return;
        }
        let wanted = ticks as f64 / ticks_per_bar;
        let error_bars = wanted - clock.now_bars();
        let error_ticks = error_bars * ticks_per_bar;
        if error_ticks.abs() <= PHASE_SLACK_TICKS {
            return;
        }
        clock.nudge_bars(error_bars * PHASE_CORRECTION);
    }

    /// Forget where we were, for a test or for a port being changed.
    #[cfg(test)]
    pub fn forget(&self) {
        self.ticks.store(0, Ordering::Release);
        self.running.store(false, Ordering::Release);
        self.interval.store(0, Ordering::Relaxed);
        if let Ok(mut recent) = self.recent.lock() {
            recent.clear();
        }
    }

    #[cfg(test)]
    pub fn ticks(&self) -> usize {
        self.ticks.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

/// Beats per minute from a tick interval — the arithmetic on its own, for
/// tests that are about the conversion rather than about the transport.
#[cfg(test)]
pub fn bpm_from_interval(interval_secs: f64) -> f64 {
    60.0 / (interval_secs * PPQN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::clock::{Clock, DEFAULT_METER};

    const RATE: f64 = 44100.0;

    /// A tick interval for a tempo, which is what the wire actually carries.
    fn interval_for(bpm: f64) -> Duration {
        Duration::from_secs_f64(60.0 / (bpm * PPQN))
    }

    /// A `Follow` driving a clock, with ticks delivered at exact intervals —
    /// which is the one thing a real wire never does, and is why the jitter
    /// tests below exist separately.
    fn following(bpm: f64, ticks: usize) -> (Follow, Clock) {
        let follow = Follow::new();
        let clock = Clock::new(RATE);
        follow.drives(clock.clone());
        follow.set_active(true);

        // Stamped so the *last* tick lands about now. Running them forward
        // from `now` instead would put every one of them in the future, where
        // `Instant::elapsed` saturates to zero and a clock that had stopped
        // would still look locked — which is a property worth testing, so the
        // times have to be honest.
        let mut at = Instant::now() - interval_for(bpm) * ticks as u32;
        follow.receive(crate::midi::input::CLOCK_START, at);
        for _ in 0..ticks {
            at += interval_for(bpm);
            follow.receive(crate::midi::input::CLOCK_TICK, at);
        }
        (follow, clock)
    }

    #[test]
    fn nothing_is_followed_until_something_is_chosen() {
        let follow = Follow::new();
        assert_eq!(follow.status(true), Status::Internal);
        assert!(!follow.is_active());
    }

    /// Every clock byte is claimed whether or not anything is being followed,
    /// because the alternative is a tick falling through to be read as a note.
    #[test]
    fn a_clock_byte_is_claimed_even_while_nothing_follows() {
        let follow = Follow::new();
        for byte in [crate::midi::input::CLOCK_TICK, crate::midi::input::CLOCK_START,
                     crate::midi::input::CLOCK_STOP, crate::midi::input::CLOCK_CONTINUE] {
            assert!(follow.receive(byte, Instant::now()), "{byte:#x} should be claimed");
        }
        assert!(!follow.receive(0x90, Instant::now()), "a note on is not clock");
    }

    #[test]
    fn the_tempo_is_read_off_the_interval_between_ticks() {
        let (follow, clock) = following(120.0, 200);
        assert!((clock.bpm() - 120.0).abs() < 1.0, "got {}", clock.bpm());
        assert!((follow.bpm().unwrap() - 120.0).abs() < 1.0);
    }

    #[test]
    fn a_different_tempo_is_read_as_a_different_tempo() {
        let (_, clock) = following(174.0, 300);
        assert!((clock.bpm() - 174.0).abs() < 2.0, "got {}", clock.bpm());
    }

    /// The whole reason the interval is smoothed rather than taken. A single
    /// tick arriving late is the wire, not a tempo change, and at 24 to the
    /// quarter one of them is a fifth of a beat's worth of apparent tempo.
    #[test]
    fn one_late_tick_does_not_move_the_tempo_much() {
        let follow = Follow::new();
        let clock = Clock::new(RATE);
        follow.drives(clock.clone());
        follow.set_active(true);

        let mut at = Instant::now();
        follow.receive(crate::midi::input::CLOCK_START, at);
        for _ in 0..100 {
            at += interval_for(120.0);
            follow.receive(crate::midi::input::CLOCK_TICK, at);
        }
        let settled = clock.bpm();

        // One tick twice as late as it should be.
        at += interval_for(120.0) * 2;
        follow.receive(crate::midi::input::CLOCK_TICK, at);

        let moved = (clock.bpm() - settled).abs();
        // Measured: 5.7 bpm with an exponential average over single intervals,
        // 1.5 across the window. This is a *whole missed tick*, which is the
        // worst single event the wire produces — see the test below for what
        // ordinary jitter costs.
        assert!(moved < 2.0, "a single late tick moved the tempo by {moved} bpm");
    }

    /// What jitter actually looks like, and the case the window is really for:
    /// a tick is *delayed* behind something else on the wire, and the next one
    /// arrives at its own correct time — so one interval is long and the next
    /// is short by the same amount. A measurement spanning both is already
    /// right, and this should barely move at all.
    #[test]
    fn a_delayed_tick_that_catches_up_costs_almost_nothing() {
        let follow = Follow::new();
        let clock = Clock::new(RATE);
        follow.drives(clock.clone());
        follow.set_active(true);

        let step = interval_for(120.0);
        let mut at = Instant::now() - step * 200;
        follow.receive(crate::midi::input::CLOCK_START, at);
        for _ in 0..150 {
            at += step;
            follow.receive(crate::midi::input::CLOCK_TICK, at);
        }
        let settled = clock.bpm();

        // Late by half an interval, then back on the grid.
        at += step + step / 2;
        follow.receive(crate::midi::input::CLOCK_TICK, at);
        at += step / 2;
        follow.receive(crate::midi::input::CLOCK_TICK, at);

        let moved = (clock.bpm() - settled).abs();
        // Measured at 0.5 bpm, and what is left is the smoothing catching up
        // rather than the window being wrong — the span across both ticks is
        // exactly right, and the estimate walks back to it over the next few.
        assert!(moved < 1.0, "ordinary jitter moved the tempo by {moved} bpm");
    }

    /// And the filter still has to *follow*, or it would be a constant.
    #[test]
    fn a_real_tempo_change_is_followed_within_a_couple_of_beats() {
        let follow = Follow::new();
        let clock = Clock::new(RATE);
        follow.drives(clock.clone());
        follow.set_active(true);

        let mut at = Instant::now();
        follow.receive(crate::midi::input::CLOCK_START, at);
        for _ in 0..200 {
            at += interval_for(120.0);
            follow.receive(crate::midi::input::CLOCK_TICK, at);
        }
        // Two beats at the new tempo.
        for _ in 0..48 {
            at += interval_for(150.0);
            follow.receive(crate::midi::input::CLOCK_TICK, at);
        }
        assert!((clock.bpm() - 150.0).abs() < 8.0, "got {}", clock.bpm());
    }

    /// A wire hiccup can make one interval look impossibly short or long.
    /// Neither is a tempo anybody set, and walking back from one takes several
    /// seconds of audible wrongness.
    #[test]
    fn an_impossible_interval_is_clamped_rather_than_followed() {
        let follow = Follow::new();
        let clock = Clock::new(RATE);
        follow.drives(clock.clone());
        follow.set_active(true);

        let mut at = Instant::now();
        follow.receive(crate::midi::input::CLOCK_START, at);
        // Ticks arriving essentially together, which reads as a huge tempo.
        for _ in 0..200 {
            at += Duration::from_micros(1);
            follow.receive(crate::midi::input::CLOCK_TICK, at);
        }
        assert!(clock.bpm() <= MAX_BPM + 1.0, "got {}", clock.bpm());
        assert!(clock.bpm() >= MIN_BPM - 1.0, "got {}", clock.bpm());
    }

    /// `Start` is the one message that carries phase outright, so it is the
    /// one moment the transport is moved rather than nudged.
    #[test]
    fn start_puts_the_transport_at_the_top() {
        let follow = Follow::new();
        let clock = Clock::new(RATE);
        clock.advance((RATE * 30.0) as u64);
        follow.drives(clock.clone());
        follow.set_active(true);

        follow.receive(crate::midi::input::CLOCK_START, Instant::now());
        assert!(follow.is_running());
        assert_eq!(follow.ticks(), 0);
        // `reset` puts bar zero just ahead of now, so the present is a hair
        // before it rather than thirty seconds past it.
        assert!(clock.now_bars() < 0.0, "got {}", clock.now_bars());
    }

    #[test]
    fn stop_stops_and_continue_starts_again() {
        let (follow, _) = following(120.0, 24);
        follow.receive(crate::midi::input::CLOCK_STOP, Instant::now());
        assert!(!follow.is_running());
        follow.receive(crate::midi::input::CLOCK_CONTINUE, Instant::now());
        assert!(follow.is_running());
    }

    /// A box already running when we start listening never sends a `Start`.
    /// Waiting for one would mean ignoring it until it happened to stop.
    #[test]
    fn a_box_already_running_is_picked_up_from_its_first_tick() {
        let follow = Follow::new();
        let clock = Clock::new(RATE);
        follow.drives(clock.clone());
        follow.set_active(true);

        follow.receive(crate::midi::input::CLOCK_TICK, Instant::now());
        assert!(follow.is_running(), "the first tick should start it");
    }

    /// Ticks are counted so that phase has something to be corrected against —
    /// tempo alone drifts a whole beat away over a few minutes.
    #[test]
    fn ticks_are_counted_from_the_start() {
        let (follow, _) = following(120.0, 48);
        assert_eq!(follow.ticks(), 48, "two quarter notes at 24 ppqn");
    }

    /// The transport is pulled back towards where the tick count says it
    /// should be. Started deliberately out of phase, it should close the gap.
    #[test]
    fn phase_is_pulled_back_towards_the_tick_count() {
        let follow = Follow::new();
        let clock = Clock::new(RATE);
        follow.drives(clock.clone());
        follow.set_active(true);

        let mut at = Instant::now();
        follow.receive(crate::midi::input::CLOCK_START, at);
        // Shove bar time a long way from where the ticks will say it is.
        clock.nudge_bars(0.5);

        let ticks_per_bar = PPQN * DEFAULT_METER.quarters_per_bar();
        let drift = |ticks: usize, clock: &Clock| {
            (ticks as f64 / ticks_per_bar - clock.now_bars()).abs()
        };
        let before = drift(0, &clock);

        for n in 1..=192 {
            at += interval_for(120.0);
            follow.receive(crate::midi::input::CLOCK_TICK, at);
            let _ = n;
        }
        let after = drift(follow.ticks(), &clock);
        assert!(after < before / 2.0, "phase should have closed: {before} then {after}");
    }

    /// And not so hard that it is audible. Bar time is what every playing
    /// pattern is placed against, so a correction anybody can hear is worse
    /// than the drift it fixes.
    #[test]
    fn phase_is_not_pulled_back_all_at_once() {
        let follow = Follow::new();
        let clock = Clock::new(RATE);
        follow.drives(clock.clone());
        follow.set_active(true);

        let at = Instant::now();
        follow.receive(crate::midi::input::CLOCK_START, at);
        clock.nudge_bars(0.5);
        let before = clock.now_bars();

        follow.receive(crate::midi::input::CLOCK_TICK, at + interval_for(120.0));
        let moved = (clock.now_bars() - before).abs();
        assert!(moved < 0.1, "one tick moved bar time by {moved} bars");
    }

    /// Small drift is left alone entirely, or the jitter the tempo filter
    /// keeps out would be handed straight to the transport by this instead.
    #[test]
    fn drift_inside_the_slack_is_left_alone() {
        let follow = Follow::new();
        let clock = Clock::new(RATE);
        follow.drives(clock.clone());
        follow.set_active(true);
        let at = Instant::now();
        follow.receive(crate::midi::input::CLOCK_START, at);

        let ticks_per_bar = PPQN * DEFAULT_METER.quarters_per_bar();
        // A quarter of a tick out, which is inside the slack.
        clock.nudge_bars(0.25 / ticks_per_bar);
        let before = clock.now_bars();
        follow.receive(crate::midi::input::CLOCK_TICK, at + interval_for(120.0));
        // Bar time still moves on its own; what must not have happened is a
        // correction on top of it.
        assert!((clock.now_bars() - before) < 0.01);
    }

    /// Silence in a room is the worst outcome, so a clock that stops arriving
    /// leaves the transport running at the tempo it had.
    #[test]
    fn a_clock_that_stops_arriving_is_reported_rather_than_acted_on() {
        let (follow, clock) = following(120.0, 100);
        let tempo = clock.bpm();
        assert_eq!(follow.status(true), Status::Locked);

        std::thread::sleep(LOST_AFTER + Duration::from_millis(20));
        assert_eq!(follow.status(true), Status::Lost);
        assert_eq!(clock.bpm(), tempo, "the transport should not have moved");
    }

    #[test]
    fn a_port_that_is_not_here_says_so() {
        let follow = Follow::new();
        follow.set_active(true);
        assert_eq!(follow.status(false), Status::Missing);
    }

    /// Turning following off leaves the transport exactly where it is. Nothing
    /// about ceasing to follow is a reason to stop playing.
    #[test]
    fn giving_up_following_leaves_the_transport_running() {
        let (follow, clock) = following(140.0, 100);
        let tempo = clock.bpm();
        follow.set_active(false);
        assert_eq!(follow.status(true), Status::Internal);
        assert_eq!(clock.bpm(), tempo);
    }

    #[test]
    fn the_interval_and_the_tempo_are_two_ways_of_saying_one_thing() {
        assert!((bpm_from_interval(60.0 / (120.0 * PPQN)) - 120.0).abs() < 1e-9);
    }
}
