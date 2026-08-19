use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Default tempo: one bar every two seconds, which is 120 bpm in 4/4.
pub const DEFAULT_CPS: f64 = 0.5;

/// The quarter notes in a 4/4 bar.
///
/// A bare constant as well as `Meter::quarters_per_bar` because its one caller
/// cannot ask a `Meter`: `DEFAULT_BEAT_SECS` is a `const`, and a `const` cannot
/// call a method. Nothing outside the tests needs it — the app reads the
/// signature off the clock.
#[cfg(test)]
pub const DEFAULT_QUARTERS_PER_BAR: f64 = 4.0;

/// The time signature every project has until it says otherwise.
pub const DEFAULT_METER: Meter = Meter { top: 4, bottom: 4 };

/// A time signature: how many of what note make a bar.
///
/// This is the only thing in the engine that decides how long a bar is, and it
/// is what makes `[d4;q, d4, d4]` fill the bar in 3/4 rather than rotate
/// against it. Everything downstream counts bars and never asks how many beats
/// went into one.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Meter {
    /// How many beats in a bar — the top of the signature.
    pub top: u32,
    /// Which note gets the beat, as its denominator — the bottom of the
    /// signature. 4 is a quarter, 8 an eighth.
    pub bottom: u32,
}

impl Meter {
    /// The bar's length in quarter notes, which is the unit the rest of the
    /// engine counts in.
    ///
    /// Compound meters fall out of the arithmetic rather than needing a case:
    /// 6/8 is six eighths, which is the same three quarters a 3/4 bar is. That
    /// the two are the same length here is correct — what differs between them
    /// is where the accents fall, and this type says nothing about accents.
    pub fn quarters_per_bar(&self) -> f64 {
        self.top as f64 * 4.0 / self.bottom as f64
    }

    /// A signature that can actually be written in note values.
    ///
    /// The bottom must be a power of two because it names a note — there is no
    /// such thing as a third note — and both ends are bounded so a typo cannot
    /// hand the clock a bar of a million beats and stall the scheduler
    /// looking for the end of it.
    pub fn is_valid(&self) -> bool {
        self.top >= 1 && self.top <= 64 && matches!(self.bottom, 1 | 2 | 4 | 8 | 16 | 32)
    }
}

impl Default for Meter {
    fn default() -> Self {
        DEFAULT_METER
    }
}

impl std::fmt::Display for Meter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.top, self.bottom)
    }
}

pub fn cps_from_bpm(bpm: f64, meter: Meter) -> f64 {
    bpm / 60.0 / meter.quarters_per_bar()
}

pub fn bpm_from_cps(cps: f64, meter: Meter) -> f64 {
    cps * 60.0 * meter.quarters_per_bar()
}

/// How long one beat lasts at a tempo — the quarter note, which is what the
/// language's `qvs` is: 0.5 seconds at the 120 bpm default.
///
/// This is the one figure the meter does *not* move. A quarter note is a
/// quarter note in 3/4 as much as in 4/4; what the signature changes is how
/// many of them make a bar. So dropping to 3/4 shortens the bar and leaves
/// every envelope and delay written in `qvs` exactly where it was.
///
/// A free function rather than a `Clock` method because the lowerer needs it
/// for the persistent graph, where there is a tempo but no clock in reach.
pub fn beat_secs_from_cps(cps: f64, meter: Meter) -> f64 {
    1.0 / (cps * meter.quarters_per_bar())
}

/// How far ahead of the present `reset` places bar 0.
///
/// The scheduler works ahead of the audio clock and matches onsets half-open,
/// so a window never reaches back for a step it has already passed. Pinning
/// bar 0 to "now" hands it an origin the next pass has already walked past:
/// the step at bar 0 falls behind that window and never sounds, and the
/// pattern is heard from its second step. The lead-in has to clear one
/// scheduler tick plus the eval that follows it, so the pass that first sees
/// the new bindings still has the whole first bar in front of it.
pub const START_LEAD_SECS: f64 = 0.1;

/// Tempo and phase, kept behind one lock because a tempo change moves both:
/// a reader that saw the new rate against the old origin would place bar 0
/// somewhere the beat never was. Only the command and scheduler threads read
/// them — the audio callback touches nothing but the frame counter — so a
/// mutex here costs nothing real time.
#[derive(Clone, Copy)]
struct Tempo {
    /// Audio time of bar 0.
    origin_secs: f64,
    /// Bars per second.
    ///
    /// Still spelled `cps` — "cycles per second" — because the obvious rename
    /// is worse: `bps` reads as *beats* per second, and the two differ by the
    /// whole of the time signature. This is the one place the older word
    /// survives, and it survives as an abbreviation rather than as a unit.
    cps: f64,
    /// How long a bar is, in beats. Held here beside the rate because the two
    /// are read together — a bar's length in seconds is neither of them alone.
    meter: Meter,
    /// Bumped by `reset`. Bar time jumps backwards there, so anything
    /// measured in bars against an older epoch is meaningless.
    epoch: u64,
}

/// The clock, in two layers.
///
/// *Audio time* is frames rendered, counted by the audio callback and read by
/// the scheduler — the only source of truth, because `Sequencer::time()`
/// returns `None` once a backend exists.
///
/// *Musical time* is bars, derived from audio time by the tempo, the time
/// signature and an **origin** — the audio time at which bar 0 sits.
/// Re-evaluating never moves the origin, so the beat survives an edit; `reset`
/// moves it to just ahead of the present so playing from silence starts a
/// pattern at its first step.
///
/// The origin exists because the frame counter cannot be zeroed: it is what
/// stays aligned with the sequencer's own internal clock, and rewinding it
/// would put every scheduled start time in the past.
/// How the frame counter is read as seconds.
///
/// Not simply a rate, because the rate can change: the output device can be
/// moved to one that runs at 44.1 kHz when the last ran at 48, and dividing
/// the whole accumulated count by the new rate would jump audio time by
/// minutes — every pattern in the piece leaping to a different bar. So the
/// rate is anchored: seconds are counted from a base, and a rate change fixes
/// the base at the present so that only the frames *after* it are counted at
/// the new rate.
#[derive(Clone, Copy)]
struct Rate {
    sample_rate: f64,
    /// The frame count when this rate came into force.
    base_frames: u64,
    /// The audio time at that frame, in the rates that came before it.
    base_secs: f64,
}

impl Rate {
    fn secs_at(&self, frames: u64) -> f64 {
        self.base_secs + frames.saturating_sub(self.base_frames) as f64 / self.sample_rate
    }
}

#[derive(Clone)]
pub struct Clock {
    frames: Arc<AtomicU64>,
    tempo: Arc<Mutex<Tempo>>,
    /// Behind a lock for the same reason `tempo` is, and read the same way:
    /// three numbers that only mean anything together, touched by the command
    /// and scheduler threads while the audio callback moves nothing but the
    /// frame counter.
    rate: Arc<Mutex<Rate>>,
}

impl Clock {
    pub fn new(sample_rate: f64) -> Self {
        Clock::with_cps(sample_rate, DEFAULT_CPS)
    }

    /// A clock at a given rate in the default 4/4. Most tests want this: what
    /// they are pinning is a rate against a grid, and the signature that grid
    /// is divided into does not enter into it.
    pub fn with_cps(sample_rate: f64, cps: f64) -> Self {
        Clock::with_cps_and_meter(sample_rate, cps, DEFAULT_METER)
    }

    pub fn with_cps_and_meter(sample_rate: f64, cps: f64, meter: Meter) -> Self {
        Clock {
            frames: Arc::new(AtomicU64::new(0)),
            tempo: Arc::new(Mutex::new(Tempo {
                origin_secs: 0.0,
                cps,
                meter,
                epoch: 0,
            })),
            rate: Arc::new(Mutex::new(Rate {
                sample_rate,
                base_frames: 0,
                base_secs: 0.0,
            })),
        }
    }

    fn rate(&self) -> Rate {
        *self.rate.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The output device has moved to another rate.
    ///
    /// Audio time does not move with it: the frames already counted keep the
    /// rate they were rendered at, and only the ones after this are divided by
    /// the new one. A jump here would be a jump in bar time, which is every
    /// pattern currently playing landing somewhere else in the piece.
    ///
    /// The callback may count a buffer between the read and the store, and
    /// those frames are then counted at the new rate rather than the old. That
    /// is a few microseconds of error against a device switch that has already
    /// interrupted the audio, and correcting it would need the callback to
    /// take this lock.
    pub fn set_sample_rate(&self, sample_rate: f64) {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return;
        }
        let frames = self.frames.load(Ordering::Relaxed);
        let mut rate = self.rate.lock().unwrap_or_else(|e| e.into_inner());
        *rate = Rate {
            sample_rate,
            base_frames: frames,
            base_secs: rate.secs_at(frames),
        };
    }

    /// A consistent view of tempo and phase.
    ///
    /// Poisoning is recovered from rather than propagated: every writer here
    /// leaves the tempo whole, and killing the scheduler thread over a panic
    /// elsewhere would take the music with it.
    fn tempo(&self) -> Tempo {
        *self.tempo.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Called by the audio callback, once per buffer, after rendering it.
    /// Post-increment keeps this in step with the sequencer's own internal
    /// time, so the absolute start times we push line up with what it renders.
    pub fn advance(&self, frames: u64) {
        self.frames.fetch_add(frames, Ordering::Relaxed);
    }

    /// Seconds of audio rendered so far — the scheduler's "now".
    pub fn now_secs(&self) -> f64 {
        self.rate().secs_at(self.frames.load(Ordering::Relaxed))
    }

    pub fn now_bars(&self) -> f64 {
        self.bars_at(self.now_secs())
    }

    /// Audio time of bar 0.
    pub fn origin_secs(&self) -> f64 {
        self.tempo().origin_secs
    }

    /// Put bar 0 just ahead of the present, so the next pattern starts at its
    /// first step. Called when playing from silence and when stopping — never
    /// on a re-eval while something is already playing, which is what keeps an
    /// edit from jolting the groove.
    ///
    /// The lead-in is what makes the first step audible rather than merely
    /// nominal; see `START_LEAD_SECS`.
    pub fn reset(&self) {
        let now = self.now_secs();
        let mut tempo = self.tempo.lock().unwrap_or_else(|e| e.into_inner());
        tempo.origin_secs = now + START_LEAD_SECS;
        tempo.epoch = tempo.epoch.wrapping_add(1);
    }

    /// Shift bar time by a fraction of a bar, without touching the tempo.
    ///
    /// The one thing following an external clock needs that setting a tempo
    /// cannot do. `set_cps` deliberately *holds* the bar position across a
    /// tempo change — which is right for a tempo drag and exactly wrong here,
    /// where the tempo is already agreed and it is the phase that has drifted.
    ///
    /// The epoch does not move: this is a nudge of a few milliseconds, not a
    /// jump, so watermarks taken before it are still good. A correction large
    /// enough to invalidate one is a `reset`, which is what `Start` does.
    pub fn nudge_bars(&self, bars: f64) {
        if !bars.is_finite() || bars == 0.0 {
            return;
        }
        let mut tempo = self.tempo.lock().unwrap_or_else(|e| e.into_inner());
        if tempo.cps <= 0.0 {
            return;
        }
        // Bar time is `(now - origin) * cps`, so moving it forward by `bars`
        // means moving the origin *back* by that many bars' worth of seconds.
        tempo.origin_secs -= bars / tempo.cps;
    }

    /// How many times bar time has jumped backwards. A bar figure only
    /// means anything against the epoch it was measured in.
    pub fn epoch(&self) -> u64 {
        self.tempo().epoch
    }

    /// Change the tempo without moving the beat.
    ///
    /// The origin moves to hold the current bar position fixed: a change
    /// made three quarters of the way through a bar stays three quarters of
    /// the way through, and only the rate ahead of it differs. Zeroing the
    /// phase instead would stutter the groove on every drag of a tempo
    /// control.
    ///
    /// The epoch does not move: bar time stays continuous, so watermarks
    /// taken before the change are still good.
    ///
    /// A tempo that is not a positive, finite number is ignored — there is no
    /// sensible beat for it, and it would poison every time the scheduler
    /// computes from here on.
    pub fn set_cps(&self, cps: f64) {
        if !cps.is_finite() || cps <= 0.0 {
            return;
        }
        let now = self.now_secs();
        let mut tempo = self.tempo.lock().unwrap_or_else(|e| e.into_inner());
        let bars = (now - tempo.origin_secs) * tempo.cps;
        tempo.cps = cps;
        tempo.origin_secs = now - bars / cps;
    }

    /// The tempo, in beats per minute — the quarter note, whatever the
    /// signature says a bar holds.
    pub fn bpm(&self) -> f64 {
        let tempo = self.tempo();
        bpm_from_cps(tempo.cps, tempo.meter)
    }

    /// Set the tempo in beats per minute, against whatever meter is running.
    ///
    /// The conversion belongs here rather than at the call site because it
    /// needs the meter, and a caller that read the meter separately could be
    /// overtaken between the two reads and set a rate for a signature that had
    /// already changed.
    pub fn set_bpm(&self, bpm: f64) {
        let meter = self.tempo().meter;
        self.set_cps(cps_from_bpm(bpm, meter));
    }

    pub fn meter(&self) -> Meter {
        self.tempo().meter
    }

    /// Change the time signature, holding both the tempo and the beat.
    ///
    /// Two things are deliberately preserved. The **quarter note** does not
    /// move: 120 bpm is 120 bpm in 3/4, so what changes is that a bar is now
    /// three of those beats instead of four, and the bar rate changes to suit.
    /// Reading the bpm back out and setting the rate from it is what holds
    /// that. And the **bar position** is held exactly as `set_cps` holds it,
    /// so switching signature mid-performance does not stutter.
    ///
    /// The epoch does not move, for the same reason it does not on a tempo
    /// change: bar time stays continuous, even though bars are now a different
    /// length. A signature that cannot be written is ignored rather than
    /// refused — there is no sensible bar for 4/5, and taking it would poison
    /// every conversion from here on.
    pub fn set_meter(&self, meter: Meter) {
        if !meter.is_valid() {
            return;
        }
        let now = self.now_secs();
        let mut tempo = self.tempo.lock().unwrap_or_else(|e| e.into_inner());
        let bpm = bpm_from_cps(tempo.cps, tempo.meter);
        let bars = (now - tempo.origin_secs) * tempo.cps;
        tempo.meter = meter;
        tempo.cps = cps_from_bpm(bpm, meter);
        tempo.origin_secs = now - bars / tempo.cps;
    }

    pub fn bars_at(&self, secs: f64) -> f64 {
        let tempo = self.tempo();
        (secs - tempo.origin_secs) * tempo.cps
    }

    pub fn secs_at(&self, bars: f64) -> f64 {
        let tempo = self.tempo();
        bars / tempo.cps + tempo.origin_secs
    }

    pub fn cps(&self) -> f64 {
        self.tempo().cps
    }

    /// The quarter note, in seconds, at the tempo running now.
    pub fn beat_secs(&self) -> f64 {
        let tempo = self.tempo();
        beat_secs_from_cps(tempo.cps, tempo.meter)
    }

    pub fn sample_rate(&self) -> f64 {
        self.rate().sample_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_convert_to_seconds() {
        let clock = Clock::new(44100.0);
        assert_eq!(clock.now_secs(), 0.0);

        clock.advance(44100);
        assert!((clock.now_secs() - 1.0).abs() < 1e-9);

        clock.advance(22050);
        assert!((clock.now_secs() - 1.5).abs() < 1e-9);
    }

    /// Moving the output device to a device that runs at another rate must not
    /// move the music. The frame counter cannot be rewound — it is what stays
    /// aligned with the sequencer's own clock — so dividing all of it by the
    /// new rate would jump audio time by however long the app had been open,
    /// and every pattern playing would land somewhere else in the piece.
    #[test]
    fn a_device_switch_changes_the_rate_without_moving_the_time() {
        let clock = Clock::new(48000.0);
        clock.advance(48000 * 60); // an hour of an evening, near enough

        let before = clock.now_secs();
        assert!((before - 60.0).abs() < 1e-9);

        clock.set_sample_rate(44100.0);
        assert!(
            (clock.now_secs() - before).abs() < 1e-9,
            "audio time jumped from {before} to {}",
            clock.now_secs()
        );
        assert_eq!(clock.sample_rate(), 44100.0);

        // And what comes after is counted at the new rate.
        clock.advance(44100);
        assert!((clock.now_secs() - (before + 1.0)).abs() < 1e-9);
    }

    /// The bar the music is on is derived from audio time, so the point of the
    /// rebase above is really this: a switch mid-performance leaves the beat
    /// exactly where it was.
    #[test]
    fn a_device_switch_leaves_the_beat_where_it_was() {
        let clock = Clock::new(48000.0);
        clock.advance(48000 * 7);
        let bars = clock.now_bars();

        clock.set_sample_rate(96000.0);
        assert!((clock.now_bars() - bars).abs() < 1e-9, "the beat moved");
    }

    /// A rate nothing could be rendered at is ignored rather than stored: it
    /// would poison every time the scheduler computes from here on, exactly as
    /// a nonsense tempo would.
    #[test]
    fn a_rate_that_is_not_a_rate_is_ignored() {
        let clock = Clock::new(48000.0);
        clock.advance(4800);
        for bad in [0.0, -48000.0, f64::NAN, f64::INFINITY] {
            clock.set_sample_rate(bad);
            assert_eq!(clock.sample_rate(), 48000.0, "{bad} should not have been taken");
            assert!((clock.now_secs() - 0.1).abs() < 1e-9);
        }
    }

    /// A clone shares the counter — this is what lets the scheduler thread
    /// read what the audio thread wrote.
    #[test]
    fn clones_share_the_counter() {
        let clock = Clock::new(48000.0);
        let reader = clock.clone();

        clock.advance(48000);
        assert!((reader.now_secs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn accumulates_across_buffers() {
        let clock = Clock::new(44100.0);
        for _ in 0..100 {
            clock.advance(441); // 10ms buffers
        }
        assert!((clock.now_secs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn seconds_and_bars_round_trip() {
        let clock = Clock::with_cps(44100.0, 0.5);
        assert!((clock.bars_at(2.0) - 1.0).abs() < 1e-9);
        assert!((clock.secs_at(1.0) - 2.0).abs() < 1e-9);

        for bars in [0.0, 0.25, 3.75, 100.5] {
            let round = clock.bars_at(clock.secs_at(bars));
            assert!((round - bars).abs() < 1e-9, "round trip failed for {bars}");
        }
    }

    #[test]
    fn tempo_scales_bar_time() {
        let fast = Clock::with_cps(44100.0, 2.0);
        assert!((fast.bars_at(1.0) - 2.0).abs() < 1e-9);
        assert!((fast.secs_at(2.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn now_bars_tracks_rendered_audio() {
        let clock = Clock::with_cps(44100.0, 0.5);
        clock.advance(44100 * 4); // four seconds
        assert!((clock.now_bars() - 2.0).abs() < 1e-9);
    }

    /// Reset puts bar 0 just ahead of the present without touching the frame
    /// counter, which has to stay aligned with the sequencer's own clock.
    #[test]
    fn reset_moves_bar_zero_to_just_ahead_of_now() {
        let clock = Clock::with_cps(44100.0, 0.5);
        clock.advance(44100 * 5); // five seconds in
        assert!((clock.now_bars() - 2.5).abs() < 1e-9);

        clock.reset();
        // Audio time is untouched: only the origin moved.
        assert!((clock.now_secs() - 5.0).abs() < 1e-9);
        assert!((clock.origin_secs() - (5.0 + START_LEAD_SECS)).abs() < 1e-9);
        // The present sits a lead-in *before* bar 0, which is what leaves the
        // first step in front of the scheduler rather than behind it.
        assert!(clock.now_bars() < 0.0, "bar 0 must still be ahead");
        assert!((clock.now_bars() + START_LEAD_SECS * 0.5).abs() < 1e-9);
    }

    /// The bug the lead-in exists for: the scheduler reads the clock a tick
    /// after the eval that reset it, and its window has to still contain
    /// bar 0 by then.
    #[test]
    fn bar_zero_is_still_ahead_one_scheduler_tick_later() {
        let clock = Clock::with_cps(44100.0, 0.5);
        clock.reset();
        // The audio thread keeps rendering while the scheduler sleeps.
        clock.advance((44100.0 * super::super::scheduler::TICK.as_secs_f64()) as u64);

        assert!(
            clock.now_bars() < 0.0,
            "a pass one tick after the reset must not have passed bar 0, got {}",
            clock.now_bars(),
        );
    }

    /// After a reset, bar 0 maps back to the audio time it was pinned to —
    /// so a scheduled start time still lands in the sequencer's future.
    #[test]
    fn seconds_and_bars_round_trip_after_a_reset() {
        let clock = Clock::with_cps(44100.0, 0.5);
        clock.advance(44100 * 7);
        clock.reset();

        assert!((clock.secs_at(0.0) - (7.0 + START_LEAD_SECS)).abs() < 1e-9);
        assert!((clock.secs_at(1.0) - (9.0 + START_LEAD_SECS)).abs() < 1e-9);
        for bars in [0.0, 0.25, 3.75, 100.5] {
            let round = clock.bars_at(clock.secs_at(bars));
            assert!((round - bars).abs() < 1e-9, "round trip failed for {bars}");
        }
    }

    /// Clones share the origin, so the scheduler thread sees a reset made on
    /// the command thread.
    #[test]
    fn clones_share_the_origin() {
        let clock = Clock::with_cps(44100.0, 0.5);
        let reader = clock.clone();
        clock.advance(44100 * 4);
        clock.reset();
        assert!((reader.origin_secs() - (4.0 + START_LEAD_SECS)).abs() < 1e-9);
    }

    /// A tempo change is heard from here on, not retroactively: the beat we
    /// are standing on does not move.
    #[test]
    fn changing_tempo_holds_the_current_bar() {
        let clock = Clock::with_cps(44100.0, 0.5);
        clock.advance(44100 * 5); // 2.5 bars in
        assert!((clock.now_bars() - 2.5).abs() < 1e-9);

        clock.set_cps(1.0);
        assert!((clock.now_bars() - 2.5).abs() < 1e-9, "the beat must not jump");

        // And from here a bar takes a second rather than two.
        clock.advance(44100);
        assert!((clock.now_bars() - 3.5).abs() < 1e-9);
    }

    /// Times still round trip after a tempo change, at the new rate.
    #[test]
    fn seconds_and_bars_round_trip_after_a_tempo_change() {
        let clock = Clock::with_cps(44100.0, 0.5);
        clock.advance(44100 * 3);
        clock.set_cps(2.0);

        assert!((clock.cps() - 2.0).abs() < 1e-9);
        for bars in [0.0, 0.25, 3.75, 100.5] {
            let round = clock.bars_at(clock.secs_at(bars));
            assert!((round - bars).abs() < 1e-9, "round trip failed for {bars}");
        }
    }

    /// Nonsense from a control surface must not poison every later reading.
    #[test]
    fn a_bad_tempo_is_ignored() {
        let clock = Clock::with_cps(44100.0, 0.5);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            clock.set_cps(bad);
            assert!((clock.cps() - 0.5).abs() < 1e-9, "{bad} should have been ignored");
        }
        assert!(clock.now_bars().is_finite());
    }

    /// Clones share the tempo, so the scheduler thread sees a change made on
    /// the command thread.
    #[test]
    fn clones_share_the_tempo() {
        let clock = Clock::with_cps(44100.0, 0.5);
        let reader = clock.clone();
        clock.set_cps(1.5);
        assert!((reader.cps() - 1.5).abs() < 1e-9);
    }

    /// The epoch marks the discontinuity: a reset moves bar time, a tempo
    /// change does not.
    #[test]
    fn only_a_reset_moves_the_epoch() {
        let clock = Clock::with_cps(44100.0, 0.5);
        let start = clock.epoch();

        clock.set_cps(1.0);
        assert_eq!(clock.epoch(), start, "tempo changes keep bar time continuous");

        clock.reset();
        assert_ne!(clock.epoch(), start, "a reset invalidates bar figures");
    }

    #[test]
    fn beats_per_minute_convert_to_bars_per_second() {
        assert!((cps_from_bpm(120.0, DEFAULT_METER) - DEFAULT_CPS).abs() < 1e-9);
        assert!((bpm_from_cps(DEFAULT_CPS, DEFAULT_METER) - 120.0).abs() < 1e-9);
        for bpm in [20.0, 96.0, 135.0, 300.0] {
            assert!((bpm_from_cps(cps_from_bpm(bpm, DEFAULT_METER), DEFAULT_METER) - bpm).abs() < 1e-9);
        }
    }

    /// Time keeps running after a reset, from the new zero — which the lead-in
    /// puts a moment after the reset itself.
    #[test]
    fn bars_advance_from_the_new_origin() {
        let clock = Clock::with_cps(44100.0, 0.5);
        clock.advance(44100 * 5);
        clock.reset();
        clock.advance((44100.0 * (2.0 + START_LEAD_SECS)) as u64);
        assert!((clock.now_bars() - 1.0).abs() < 1e-6);
    }

    // ---- the time signature ----

    /// A bar is as many quarter notes as the signature says, and a compound
    /// meter is not a special case: 6/8 is six eighths, which is three
    /// quarters, which is what 3/4 is too.
    #[test]
    fn a_signature_says_how_many_quarters_are_in_a_bar() {
        assert_eq!(Meter { top: 4, bottom: 4 }.quarters_per_bar(), 4.0);
        assert_eq!(Meter { top: 3, bottom: 4 }.quarters_per_bar(), 3.0);
        assert_eq!(Meter { top: 6, bottom: 8 }.quarters_per_bar(), 3.0);
        assert_eq!(Meter { top: 7, bottom: 8 }.quarters_per_bar(), 3.5);
        assert_eq!(Meter { top: 2, bottom: 2 }.quarters_per_bar(), 4.0);
    }

    /// The beat is a note value, so its denominator has to be one. Everything
    /// else is a signature nobody can write, and the clock is better off not
    /// holding it.
    #[test]
    fn a_signature_the_notation_cannot_write_is_not_valid() {
        assert!(Meter { top: 3, bottom: 4 }.is_valid());
        assert!(Meter { top: 12, bottom: 8 }.is_valid());
        assert!(!Meter { top: 4, bottom: 5 }.is_valid(), "there is no fifth note");
        assert!(!Meter { top: 0, bottom: 4 }.is_valid(), "a bar of nothing");
        assert!(!Meter { top: 500, bottom: 4 }.is_valid(), "nor a bar of five hundred");
    }

    /// The signature moves the bar and leaves the beat alone. This is the
    /// property the whole design rests on: 120 bpm is 120 bpm in any meter, so
    /// switching to 3/4 shortens the bar to three of those beats rather than
    /// speeding the music up.
    #[test]
    fn changing_the_signature_holds_the_tempo_and_shortens_the_bar() {
        let clock = Clock::new(44100.0);
        assert!((clock.bpm() - 120.0).abs() < 1e-9);
        assert!((clock.beat_secs() - 0.5).abs() < 1e-9);

        clock.set_meter(Meter { top: 3, bottom: 4 });

        assert!((clock.bpm() - 120.0).abs() < 1e-9, "the tempo must not move");
        assert!((clock.beat_secs() - 0.5).abs() < 1e-9, "nor the quarter note");
        // A bar is now three half-second beats rather than four.
        assert!((clock.secs_at(1.0) - clock.secs_at(0.0) - 1.5).abs() < 1e-9);
    }

    /// And it holds the beat we are standing on, exactly as a tempo change
    /// does — switching signature mid-performance must not stutter.
    #[test]
    fn changing_the_signature_holds_the_current_bar() {
        let clock = Clock::new(44100.0);
        clock.advance(44100 * 5); // 2.5 bars in at two seconds a bar
        assert!((clock.now_bars() - 2.5).abs() < 1e-9);

        clock.set_meter(Meter { top: 3, bottom: 4 });
        assert!((clock.now_bars() - 2.5).abs() < 1e-9, "the beat must not jump");
    }

    /// A signature that cannot be written is ignored rather than taken, for
    /// the same reason a tempo of zero is: every conversion after it would be
    /// nonsense, and there is nowhere down here to report anything.
    #[test]
    fn an_impossible_signature_leaves_the_clock_alone() {
        let clock = Clock::new(44100.0);
        clock.set_meter(Meter { top: 4, bottom: 5 });
        assert_eq!(clock.meter(), DEFAULT_METER);
        assert!((clock.bpm() - 120.0).abs() < 1e-9);
    }

    /// Setting a tempo works against whatever signature is running, so the two
    /// controls do not have to be touched in a particular order.
    #[test]
    fn a_tempo_set_in_one_signature_means_the_same_beat_in_another() {
        let clock = Clock::new(44100.0);
        clock.set_meter(Meter { top: 3, bottom: 4 });
        clock.set_bpm(90.0);

        assert!((clock.bpm() - 90.0).abs() < 1e-9);
        assert!((clock.beat_secs() - 60.0 / 90.0).abs() < 1e-9);
        // Three beats to the bar at 90 bpm: two seconds.
        assert!((clock.secs_at(1.0) - clock.secs_at(0.0) - 2.0).abs() < 1e-9);
    }
}
