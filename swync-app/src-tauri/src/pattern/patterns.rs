use crate::pattern::pattern::{Event, Pattern, Span};
use crate::pattern::rate::Rate;

/// Scales the event's length rather than reaching the instrument, so
/// `legato: 0.2` is staccato and `1.5` overlaps the next note. Applied here,
/// in `query`, because a length is a property of the event.
pub const LEGATO: &str = "legato";

/// Places the voice in the stereo field rather than reaching the instrument:
/// -1 is hard left, 0 centre, 1 hard right. Applied in `build_voice`, because
/// a position is a property of the voice — it rides through here as an
/// ordinary lane value and is taken off the end.
pub const PAN: &str = "pan";

/// The lane names that never reach the instrument, each with what it does
/// instead. A parameter of one of these names could never be filled, so `play`
/// refuses it at bind time rather than leaving it to be discovered by ear.
pub const RESERVED: [(&str, &str); 2] = [
    (LEGATO, "sets the note's length"),
    (PAN, "places the voice in the stereo field"),
];

/// One named parameter, as a sequence of values rather than a shape in time.
///
/// A lane is read by position: the nth note of the binding takes the nth value,
/// wrapping when it runs out. So the two lengths are free of each other — three
/// cutoffs against four notes is a real 3-against-4, rotating a step each bar
/// and coming back into phase after three, and twenty cutoffs against two notes
/// walks all twenty over ten bars. Reading a lane by *time* instead would
/// squeeze it into the one bar it shares with the pattern, where the extra
/// values are duplicated or skipped and nothing ever moves.
#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub name: String,
    pub pattern: Pattern,
}

/// A pattern paired with the instrument that plays it.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub instrument: String,   // name of the user `fn`
    /// Structure, and the instrument's first parameter.
    pub pattern: Pattern,
    pub lanes: Vec<Lane>,
    /// Bars to wait after the origin's downbeat before this binding starts.
    /// Zero for everything `play` writes directly; `.then` sets it so what
    /// follows begins where the previous one stopped.
    pub start: f64,
    /// How long this binding sounds for, in bars, counted from the eval that
    /// published it. `None` is `play`: it loops for as long as it is playing.
    /// `play_once` and `playn` set it, and it is measured in bars rather than
    /// repeats because `rate` has already been folded into the pattern.
    pub bars: Option<f64>,
    /// How often the whole window comes back around, in bars. `None` is
    /// every binding written before `wthen` existed: the window opens once.
    ///
    /// A repeating binding sounds during `[start, start + bars)` and again
    /// every `repeat` bars after that, forever. It is what makes a choice
    /// worth rerolling — without somewhere to come back to, a branch would be
    /// picked once and that would be the end of it.
    pub repeat: Option<f64>,
    /// Which arm of which choice this binding belongs to, if it belongs to
    /// one. The window is gated on the arm being the one drawn for that
    /// repetition, so of a choice's arms exactly one sounds each time around.
    pub choice: Option<ChoiceRef>,
    /// The speed this was played at, kept after it has already been folded into
    /// `pattern` as a `Fast`.
    ///
    /// Nothing about *timing* reads it — the pattern places its own notes, and
    /// that is why it could be discarded before. A voice reads it: `qvs` is the
    /// beat of the clock the note is played on, and a pattern at rate 2 has a
    /// beat half as long as the transport's. It cannot be recovered from an
    /// event, which knows how long it is but not what fraction of a pattern
    /// that was.
    pub rate: Rate,
}

/// A binding's membership of a choice: which choice, and which of its arms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChoiceRef {
    pub group: usize,
    pub arm: usize,
}

/// One `wthen`: the weights its arms were given, and the seed its draws come
/// from.
///
/// The draw is a *hash*, not a running RNG, and that is the whole design.
/// `query` is called once per scheduler pass over a lookahead window that
/// overlaps the last one, and a binding near a boundary is asked about more
/// than once — so the answer has to depend only on which repetition is being
/// asked about, never on how many times it has been asked. A hash of
/// `(seed, group, repetition)` gives a fresh draw each time around and the
/// same draw every time that repetition comes up, which is what keeps a note
/// from flickering in and out as the horizon creeps past it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChoiceGroup {
    /// Non-negative, summing to something positive. Normalised at draw time
    /// rather than at build time, so weights need not be given as fractions.
    pub weights: Vec<f64>,
    /// Fixed per eval, so re-evaluating deals a new hand and `seed` pins one.
    pub seed: u64,
}

impl ChoiceGroup {
    /// Which arm sounds on repetition `n`.
    ///
    /// Returns `arm == weights.len()` for the "nothing" arm `maybe` adds, so a
    /// choice can also come up silent.
    pub fn arm_at(&self, group: usize, n: u64) -> usize {
        let total: f64 = self.weights.iter().filter(|w| w.is_finite() && **w > 0.0).sum();
        if !(total > 0.0) {
            return usize::MAX; // no arm can match: the choice is silent
        }
        let r = unit_hash(self.seed, group as u64, n) * total;
        let mut acc = 0.0;
        for (i, w) in self.weights.iter().enumerate() {
            if !w.is_finite() || *w <= 0.0 {
                continue;
            }
            acc += w;
            if r < acc {
                return i;
            }
        }
        // Only reachable when the running sum falls a bit short of `total`
        // through rounding, and then the last positive arm is the right answer.
        self.weights.iter().rposition(|w| w.is_finite() && *w > 0.0).unwrap_or(usize::MAX)
    }
}

/// SplitMix64, the finalising mix. Cheap, and it decorrelates the low bits —
/// which matters here because two of the three inputs are small integers that
/// differ by one.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// A number in `[0, 1)` from three integers. 53 bits, the most an `f64` holds
/// without a gap.
fn unit_hash(seed: u64, group: u64, n: u64) -> f64 {
    let h = mix(seed ^ mix(group.wrapping_mul(0x9E3779B97F4A7C15) ^ mix(n)));
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Everything currently playing. An eval replaces this wholesale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Patterns {
    pub bindings: Vec<Binding>,
    /// Bar time when this set was published — what a bounded binding counts
    /// its bars from, so a one-shot fires at the eval that wrote it rather
    /// than at wherever the free-running clock happens to be.
    ///
    /// Safe to hold as a bare number because the only thing that moves bar
    /// time under it is `Clock::reset`, and both callers of that either
    /// republish immediately after (an eval from silence) or clear the
    /// bindings entirely (stop).
    pub origin: f64,
    /// The choices this set's bindings refer to by index. Empty for a program
    /// with no `wthen` in it, which is most of them.
    pub choices: Vec<ChoiceGroup>,
}

/// A stretch of time a binding may sound in, and the bar its own clock
/// started at.
///
/// The two are not the same number and cannot be recovered from each other: a
/// window is clipped to the span being queried, so it usually begins in the
/// middle of the thing it belongs to, while the anchor is where that thing
/// began however long ago. The anchor is both where a rate curve is measured
/// from and where the pattern's own grid is laid from — a section starts at
/// its first step, not at whichever step the bar line happens to be passing.
///
/// `grid` is the anchor for everything the arrangement placed, and zero for a
/// plain `play` — see [`joins_in_progress`].
struct Window {
    span: Span,
    anchor: f64,
    grid: f64,
}

/// True when a binding joins whatever is already playing rather than being
/// placed by an arrangement: a plain `play`, with nothing before it and no end.
///
/// Such a binding keeps the performance's own grid — bar zero, the one every
/// pattern was laid on before sections existed — so a re-eval does not re-phase
/// a loop that is already running. Everything else is a section, and a section
/// begins where it begins.
fn joins_in_progress(b: &Binding) -> bool {
    b.start == 0.0 && b.bars.is_none()
}

/// An event with its instrument attached — what the scheduler consumes.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundEvent {
    pub instrument: String,
    pub event: Event,
    /// Lane values sampled at this event's onset, ready to be passed by name.
    /// A lane resting here is absent, so the parameter falls back to its own
    /// default rather than erroring.
    pub args: Vec<(String, f64)>,
    /// How fast the binding was running when this note began — its `rate`, and
    /// under an `accel` the speed the curve had reached by here. Sampled per
    /// note for the same reason a lane is: the voice is built once, at the
    /// onset, and holds whatever it was told for its whole length.
    pub rate: f64,
}

impl Patterns {
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub fn query(&self, span: Span) -> Vec<BoundEvent> {
        self.bindings.iter().flat_map(|b| {
            // Before it opens, past where it closed, or — for a binding under
            // a choice — in a repetition where a different arm was drawn.
            let windows = self.windows(b, span);
            if windows.is_empty() {
                return Vec::new();
            }
            // Flattened once per binding rather than per event: a lane is the
            // same line of values whichever note is asking.
            let lanes: Vec<(&str, Vec<Option<f64>>)> = b.lanes.iter()
                .map(|l| (l.name.as_str(), l.pattern.values()))
                .collect();

            windows.into_iter().flat_map(|Window { span, anchor, grid }| {
            // Queried in the binding's own time, where its first step sits at
            // zero, and mapped back at the end. A fixed rate reads no anchor —
            // it cannot, since `phase` is the same shape wherever it is asked
            // from — so shifting the question is the only way to tell a
            // periodic pattern that it began somewhere other than bar zero.
            // Without it a pass that does not divide the offset opens partway
            // through itself: seventeen eighths placed at bar 8 would start on
            // its fourteenth note and wrap round to its first.
            let span = Span::new(span.begin - grid, span.end - grid);
            let anchor = anchor - grid;
            b.pattern.query_from(span, anchor).into_iter().map(|mut event| {
                // Which note this is, counted from where the binding opened —
                // the lane's position, not a time to look up. Counted against
                // the same clock it was played on, or a rate curve would number
                // the notes differently than it placed them.
                let nth = b.pattern.onsets_before_from(event.begin, anchor);
                let mut args = Vec::with_capacity(lanes.len());
                for (name, values) in &lanes {
                    if values.is_empty() { continue }
                    let Some(v) = values[nth % values.len()] else { continue };
                    if *name == LEGATO {
                        // Applied here so `dur` and the voice's own lifetime
                        // stay the same number: the scheduler derives both from
                        // the event's span.
                        if v.is_finite() && v > 0.0 {
                            event.end = event.begin + (event.end - event.begin) * v;
                        }
                    } else {
                        args.push((name.to_string(), v));
                    }
                }
                // Read at the onset against the same anchor the note was
                // placed from, so a curve tells a voice the speed it is
                // actually being played at rather than the one it started at.
                // Asked in the shifted time the note was placed in: a curve
                // reads the difference and a fixed rate reads neither, so the
                // shift cancels either way.
                let rate = b.rate.at(event.begin, anchor);
                // Back onto the transport's clock, the only one the scheduler
                // knows about.
                event.begin += grid;
                event.end += grid;
                BoundEvent { instrument: b.instrument.clone(), event, args, rate }
            }).collect::<Vec<_>>()
            }).collect::<Vec<_>>()
        }).collect()
    }

    /// Every part of `span` a binding may sound in.
    ///
    /// One window for everything that opens once, which is `window` unchanged.
    /// A repeating binding gets one per repetition the span reaches, and a
    /// binding under a choice keeps only the repetitions its own arm was drawn
    /// for. Several, rather than one, because a lookahead window is free to
    /// straddle a repetition boundary — and at fast tempos it often does.
    fn windows(&self, b: &Binding, span: Span) -> Vec<Window> {
        // Where this binding's own clock starts, which a rate curve is measured
        // from. The same figure the window opens at — a section's first note is
        // where it begins to accelerate — and a repeating binding takes its
        // current repetition's, so a curve runs afresh each time around rather
        // than sitting at its end rate forever after the first pass.
        let opens = self.origin.ceil() + b.start;

        // A plain `play` was never placed anywhere, so it has no beginning of
        // its own to lay a grid from and keeps the transport's.
        let grid = if joins_in_progress(b) { 0.0 } else { opens };

        // The one window everything that does not repeat gets: opening once,
        // and starting its clock there.
        let once = || {
            self.window(b.start, b.bars, span)
                .map(|span| Window { span, anchor: opens, grid })
                .into_iter()
                .collect()
        };

        let Some(period) = b.repeat else {
            // Nothing repeats, so a choice would be drawn once and stay drawn;
            // `wthen` always sets both, and this is the path everything else
            // takes.
            return once();
        };
        // A repetition is only meaningful against a window that closes: an
        // open-ended one already covers every later repetition of itself.
        let (Some(bars), true) = (b.bars, period.is_finite() && period > 0.0) else {
            return once();
        };

        // Which repetitions can overlap the span at all. The window is
        // `bars` long, so one starting up to `bars` before the span may
        // still reach into it.
        let first = ((span.begin - opens - bars) / period).floor().max(0.0);
        let last = ((span.end - opens) / period).floor();
        if !first.is_finite() || !last.is_finite() || last < first {
            return Vec::new();
        }
        // Cheap insurance against a tiny period and a distant span asking for
        // millions of windows: past this many the repetitions are far shorter
        // than a note and nobody could hear them apart.
        const MAX_REPETITIONS: f64 = 4096.0;
        let last = last.min(first + MAX_REPETITIONS);

        let mut out = Vec::new();
        let mut n = first;
        while n <= last {
            let repetition = n as u64;
            let sounds = match b.choice {
                None => true,
                Some(ChoiceRef { group, arm }) => self
                    .choices
                    .get(group)
                    .is_some_and(|c| c.arm_at(group, repetition) == arm),
            };
            if sounds {
                let repeats_at = opens + n * period;
                let begin = span.begin.max(repeats_at);
                let end = span.end.min(repeats_at + bars);
                if end > begin {
                    // Each repetition is its own section: the curve runs again
                    // and the pattern starts again, both from here.
                    out.push(Window {
                        span: Span::new(begin, end),
                        anchor: repeats_at,
                        grid: repeats_at,
                    });
                }
            }
            n += 1.0;
        }
        out
    }

    /// The part of `span` a binding bounded to `bars` may still sound in, or
    /// `None` if none of it is.
    ///
    /// The window opens at the first whole bar at or after the origin, so a
    /// one-shot dropped into a running performance lands on a downbeat and is
    /// heard from its first step rather than joining halfway through. Playing
    /// from silence puts the origin a lead-in *before* bar 0, which rounds up
    /// to 0 — the one-shot starts at once.
    fn window(&self, start: f64, bars: Option<f64>, span: Span) -> Option<Span> {
        // Plain `play` with nothing before it joins the performance already in
        // progress, so a re-eval mid-bar does not gap until the next downbeat.
        // The same binding keeps the performance's grid, for the same reason —
        // see `joins_in_progress`.
        if start == 0.0 && bars.is_none() {
            return Some(span);
        }

        let opens = self.origin.ceil() + start;
        let begin = span.begin.max(opens);
        let end = match bars {
            None => span.end,
            // Also catches NaN, which would otherwise open a window nothing
            // closes.
            Some(c) if c > 0.0 => span.end.min(opens + c),
            Some(_) => return None,
        };
        (end > begin).then(|| Span::new(begin, end))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::pattern::Step;

    #[test]
    fn bound_events_carry_their_instrument() {
        let pats = Patterns {
            bindings: vec![
                Binding {
                    instrument: "kick".into(),
                    pattern: Pattern::steps([Some(1.0), None]),
                    lanes: Vec::new(),
                    start: 0.0,
                    bars: None, repeat: None, choice: None, rate: Rate::Fixed(1.0) },
                Binding {
                    instrument: "hat".into(),
                    pattern: Pattern::steps([Some(1.0), Some(1.0)]),
                    lanes: Vec::new(),
                    start: 0.0,
                    bars: None, repeat: None, choice: None, rate: Rate::Fixed(1.0) },
            ],
            ..Default::default()
        };

        let evs = pats.query(Span::new(0.0, 1.0));
        let mut names: Vec<_> = evs.iter().map(|b| b.instrument.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["hat", "hat", "kick"]);
    }

    #[test]
    fn empty_pattern_set_queries_to_nothing() {
        let pats = Patterns::default();
        assert!(pats.is_empty());
        assert!(pats.query(Span::new(0.0, 100.0)).is_empty());
    }

    // ---- lanes ----

    fn bound(pattern: Pattern, lanes: Vec<Lane>) -> Vec<super::BoundEvent> {
        Patterns {
            bindings: vec![Binding { instrument: "i".into(), pattern, lanes, start: 0.0, bars: None, repeat: None, choice: None, rate: Rate::Fixed(1.0) }],
            ..Default::default()
        }
        .query(Span::new(0.0, 1.0))
    }

    fn lane(name: &str, steps: Vec<Option<f64>>) -> Lane {
        Lane { name: name.into(), pattern: Pattern::steps(steps) }
    }

    #[test]
    fn lanes_are_sampled_at_each_onset() {
        let evs = bound(
            Pattern::steps([Some(60.0), Some(62.0)]),
            vec![lane("cut", vec![Some(400.0), Some(2000.0)])],
        );

        assert_eq!(evs[0].args, vec![("cut".to_string(), 400.0)]);
        assert_eq!(evs[1].args, vec![("cut".to_string(), 2000.0)]);
    }

    /// A shorter lane repeats against a longer pattern, note for note — not
    /// stretched to cover it. Two values under four notes is heard twice.
    #[test]
    fn a_short_lane_repeats_under_a_long_pattern() {
        let evs = bound(
            Pattern::steps([Some(1.0), Some(2.0), Some(3.0), Some(4.0)]),
            vec![lane("cut", vec![Some(10.0), Some(20.0)])],
        );

        let cuts: Vec<f64> = evs.iter().map(|e| e.args[0].1).collect();
        assert_eq!(cuts, vec![10.0, 20.0, 10.0, 20.0]);
    }

    /// The case the whole positional reading exists for: a lane longer than the
    /// pattern is not squeezed into the bar it shares with it. Two notes and
    /// six cutoffs take three bars to come back around, and every value is
    /// heard on the way.
    #[test]
    fn a_long_lane_walks_across_cycles() {
        let pats = Patterns {
            bindings: vec![Binding {
                instrument: "i".into(),
                pattern: Pattern::steps([Some(1.0), Some(2.0)]),
                lanes: vec![lane("cut", (1..=6).map(|i| Some(i as f64 * 100.0)).collect())],
                start: 0.0,
                bars: None, repeat: None, choice: None, rate: Rate::Fixed(1.0) }],
            ..Default::default()
        };

        let cuts: Vec<f64> = (0..4)
            .flat_map(|c| pats.query(Span::new(c as f64, c as f64 + 1.0)))
            .map(|e| e.args[0].1)
            .collect();

        assert_eq!(cuts, vec![
            100.0, 200.0,   // bar 0
            300.0, 400.0,   // bar 1
            500.0, 600.0,   // bar 2
            100.0, 200.0,   // bar 3 — back in phase
        ]);
    }

    /// Lengths with a common factor rotate rather than repeat: three against
    /// four is a step further along each bar, in phase again after three.
    #[test]
    fn uneven_lengths_rotate_against_each_other() {
        let pats = Patterns {
            bindings: vec![Binding {
                instrument: "i".into(),
                pattern: Pattern::steps([Some(1.0), Some(2.0), Some(3.0), Some(4.0)]),
                lanes: vec![lane("cut", vec![Some(10.0), Some(20.0), Some(30.0)])],
                start: 0.0,
                bars: None, repeat: None, choice: None, rate: Rate::Fixed(1.0) }],
            ..Default::default()
        };

        let bar = |c: i32| -> Vec<f64> {
            pats.query(Span::new(c as f64, c as f64 + 1.0))
                .iter().map(|e| e.args[0].1).collect()
        };

        assert_eq!(bar(0), vec![10.0, 20.0, 30.0, 10.0]);
        assert_eq!(bar(1), vec![20.0, 30.0, 10.0, 20.0]);
        assert_eq!(bar(2), vec![30.0, 10.0, 20.0, 30.0]);
        assert_eq!(bar(3), vec![10.0, 20.0, 30.0, 10.0]);
    }

    /// A lane counts notes, not time, so the speed the notes go at does not
    /// move it: the nth note takes the nth value at any rate.
    #[test]
    fn rate_does_not_shift_a_lane() {
        let pats = Patterns {
            bindings: vec![Binding {
                instrument: "i".into(),
                pattern: Pattern::fast(2.0, Pattern::steps([Some(1.0), Some(2.0)])),
                lanes: vec![lane("cut", vec![Some(10.0), Some(20.0), Some(30.0)])],
                start: 0.0,
                bars: None, repeat: None, choice: None, rate: Rate::Fixed(1.0) }],
            ..Default::default()
        };

        // Twice as fast is four notes a bar, still taking the lane in order.
        let cuts: Vec<f64> = pats.query(Span::new(0.0, 1.0))
            .iter().map(|e| e.args[0].1).collect();
        assert_eq!(cuts, vec![10.0, 20.0, 30.0, 10.0]);
    }

    /// A rest in the *pattern* is not a note, so it takes no lane value with
    /// it: the lane advances by what sounds, not by what was written.
    #[test]
    fn a_rest_in_the_pattern_does_not_consume_a_lane_value() {
        let evs = bound(
            Pattern::steps([Some(1.0), None, Some(2.0), Some(3.0)]),
            vec![lane("cut", vec![Some(10.0), Some(20.0), Some(30.0)])],
        );

        let cuts: Vec<f64> = evs.iter().map(|e| e.args[0].1).collect();
        assert_eq!(cuts, vec![10.0, 20.0, 30.0]);
    }

    /// A nested list in a lane is more values, not a subdivision: lanes are
    /// read by position, so nesting only affects the order.
    #[test]
    fn a_nested_lane_flattens_into_the_line() {
        let evs = bound(
            Pattern::steps([Some(1.0), Some(2.0), Some(3.0)]),
            vec![Lane {
                name: "cut".into(),
                pattern: Pattern::seq(vec![
                    Step::Value(10.0),
                    Step::Group(Box::new(Pattern::steps([Some(20.0), Some(30.0)]))),
                ]),
            }],
        );

        let cuts: Vec<f64> = evs.iter().map(|e| e.args[0].1).collect();
        assert_eq!(cuts, vec![10.0, 20.0, 30.0]);
    }

    /// A lane resting says nothing, so the parameter falls to its own default
    /// rather than being passed a value the lane never had.
    #[test]
    fn a_resting_lane_passes_nothing() {
        let evs = bound(
            Pattern::steps([Some(1.0), Some(2.0)]),
            vec![lane("cut", vec![Some(400.0), None])],
        );

        assert_eq!(evs[0].args.len(), 1);
        assert!(evs[1].args.is_empty(), "a rest in a lane must not pass a value");
    }

    /// Legato scales the event's length instead of being passed on: the
    /// scheduler derives both `dur` and the voice's lifetime from that span.
    #[test]
    fn legato_shortens_the_event_and_is_not_an_argument() {
        let evs = bound(
            Pattern::steps([Some(1.0), Some(2.0)]),
            vec![lane(LEGATO, vec![Some(0.5), Some(2.0)])],
        );

        assert!(evs.iter().all(|e| e.args.is_empty()), "legato is not passed to the instrument");
        assert!((evs[0].event.duration() - 0.25).abs() < 1e-9, "got {:?}", evs[0].event);
        assert!((evs[1].event.duration() - 1.0).abs() < 1e-9, "got {:?}", evs[1].event);
        // Onsets never move — only the end does.
        assert_eq!(evs[0].event.begin, 0.0);
        assert_eq!(evs[1].event.begin, 0.5);
    }

    /// A nonsense legato value leaves the note at its natural length rather
    /// than producing an event the sequencer would reject.
    #[test]
    fn a_bad_legato_value_is_ignored() {
        for bad in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            let evs = bound(
                Pattern::steps([Some(1.0)]),
                vec![Lane { name: LEGATO.into(), pattern: Pattern::steps([Some(bad)]) }],
            );
            assert!((evs[0].event.duration() - 1.0).abs() < 1e-9, "legato {bad} changed the note");
        }
    }

    // ---- bounded bindings ----

    fn bounded(bars: Option<f64>, origin: f64, span: Span) -> Vec<f64> {
        Patterns {
            bindings: vec![Binding {
                instrument: "i".into(),
                pattern: Pattern::steps([Some(1.0), Some(2.0)]),
                lanes: Vec::new(),
                start: 0.0,
                bars, repeat: None, choice: None, rate: Rate::Fixed(1.0) }],
            origin, choices: Vec::new() }
        .query(span)
        .iter()
        .map(|e| e.event.begin)
        .collect()
    }

    /// The default: `play` keeps going for as long as it is playing.
    #[test]
    fn an_unbounded_binding_never_stops() {
        assert_eq!(bounded(None, 0.0, Span::new(8.0, 9.0)), vec![8.0, 8.5]);
    }

    /// `play_once` from silence: the reset puts the origin a lead-in before
    /// bar 0, and the pattern plays exactly one bar from there.
    #[test]
    fn one_cycle_plays_then_stops() {
        assert_eq!(bounded(Some(1.0), -0.05, Span::new(0.0, 1.0)), vec![0.0, 0.5]);
        assert!(bounded(Some(1.0), -0.05, Span::new(1.0, 4.0)).is_empty());
    }

    #[test]
    fn a_counted_binding_plays_that_many_cycles() {
        assert_eq!(
            bounded(Some(3.0), 0.0, Span::new(0.0, 4.0)),
            vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5],
        );
    }

    /// Dropped into a running performance, a one-shot waits for the downbeat
    /// rather than starting halfway through its own pattern.
    #[test]
    fn a_one_shot_started_mid_cycle_begins_at_the_next_one() {
        assert_eq!(bounded(Some(1.0), 3.2, Span::new(3.2, 6.0)), vec![4.0, 4.5]);
    }

    /// The scheduler queries in small spans, so the window has to be assembled
    /// from pieces exactly as one big query would have it.
    #[test]
    fn a_window_queried_in_pieces_matches_one_query() {
        let whole = bounded(Some(2.0), 0.0, Span::new(0.0, 5.0));

        let mut pieced = Vec::new();
        let mut t = 0.0;
        while t < 5.0 {
            pieced.extend(bounded(Some(2.0), 0.0, Span::new(t, t + 0.13)));
            t += 0.13;
        }

        assert_eq!(whole, pieced);
    }

    /// A window that never opens is silence, not a binding that plays forever.
    #[test]
    fn a_degenerate_count_sounds_nothing() {
        for bad in [0.0, -1.0, f64::NAN] {
            assert!(
                bounded(Some(bad), 0.0, Span::new(0.0, 8.0)).is_empty(),
                "a count of {bad} should have sounded nothing",
            );
        }
    }

    /// A bounded binding that has run out must not silence the ones still
    /// looping alongside it.
    #[test]
    fn a_finished_binding_leaves_the_others_playing() {
        let pats = Patterns {
            bindings: vec![
                Binding {
                    instrument: "once".into(),
                    pattern: Pattern::steps([Some(1.0)]),
                    lanes: Vec::new(),
                    start: 0.0,
                    bars: Some(1.0), repeat: None, choice: None, rate: Rate::Fixed(1.0) },
                Binding {
                    instrument: "loop".into(),
                    pattern: Pattern::steps([Some(1.0)]),
                    lanes: Vec::new(),
                    start: 0.0,
                    bars: None, repeat: None, choice: None, rate: Rate::Fixed(1.0) },
            ],
            origin: 0.0, choices: Vec::new() };

        let names: Vec<_> = pats
            .query(Span::new(5.0, 6.0))
            .iter()
            .map(|e| e.instrument.clone())
            .collect();
        assert_eq!(names, vec!["loop"]);
    }

    #[test]
    fn several_lanes_all_reach_the_event() {
        let evs = bound(
            Pattern::steps([Some(1.0)]),
            vec![lane("cut", vec![Some(400.0)]), lane("amp", vec![Some(0.8)])],
        );

        assert_eq!(evs[0].args, vec![("cut".into(), 400.0), ("amp".into(), 0.8)]);
    }

    // ---- sequenced bindings ----

    fn started(start: f64, bars: Option<f64>, span: Span) -> Vec<f64> {
        Patterns {
            bindings: vec![Binding {
                instrument: "i".into(),
                pattern: Pattern::steps([Some(1.0), Some(2.0)]),
                lanes: Vec::new(),
                start,
                bars, repeat: None, choice: None, rate: Rate::Fixed(1.0) }],
            origin: 0.0, choices: Vec::new() }
        .query(span)
        .iter()
        .map(|e| e.event.begin)
        .collect()
    }

    /// What `.then` writes: silent until its offset, then playing normally.
    #[test]
    fn a_started_binding_waits_for_its_offset() {
        assert!(started(4.0, None, Span::new(0.0, 4.0)).is_empty());
        assert_eq!(started(4.0, None, Span::new(0.0, 5.0)), vec![4.0, 4.5]);
        assert_eq!(started(4.0, None, Span::new(9.0, 10.0)), vec![9.0, 9.5]);
    }

    /// An offset one-shot sounds for its own count, measured from its start.
    #[test]
    fn a_started_one_shot_runs_from_where_it_opens() {
        assert!(started(2.0, Some(1.0), Span::new(0.0, 2.0)).is_empty());
        assert_eq!(started(2.0, Some(1.0), Span::new(0.0, 8.0)), vec![2.0, 2.5]);
    }

    /// The scheduler queries in small spans; a sequenced binding has to be
    /// assembled from pieces exactly as one big query would have it.
    #[test]
    fn a_sequenced_window_queried_in_pieces_matches_one_query() {
        let whole = started(2.0, Some(2.0), Span::new(0.0, 8.0));

        let mut pieced = Vec::new();
        let mut t = 0.0;
        while t < 8.0 {
            pieced.extend(started(2.0, Some(2.0), Span::new(t, t + 0.13)));
            t += 0.13;
        }

        assert_eq!(whole, pieced);
    }

    /// Sequencing counts from the origin's downbeat, like a one-shot does, so
    /// a chain dropped into a running performance stays on the grid.
    #[test]
    fn an_offset_is_measured_from_the_downbeat() {
        let pats = Patterns {
            bindings: vec![Binding {
                instrument: "i".into(),
                pattern: Pattern::steps([Some(1.0)]),
                lanes: Vec::new(),
                start: 2.0,
                bars: Some(1.0), repeat: None, choice: None, rate: Rate::Fixed(1.0) }],
            origin: 3.2, choices: Vec::new() };
        // Origin 3.2 rounds up to 4, plus two bars of waiting.
        let onsets: Vec<f64> = pats
            .query(Span::new(3.2, 9.0))
            .iter()
            .map(|e| e.event.begin)
            .collect();
        assert_eq!(onsets, vec![6.0]);
    }

    // ---- rate curves ----

    /// A binding of one note per pass, accelerating from 1x to 3x over four
    /// bars, placed `start` bars after an origin.
    fn accelerating(start: f64, origin: f64, repeat: Option<f64>) -> Patterns {
        use crate::pattern::rate::Rate;
        Patterns {
            bindings: vec![Binding {
                instrument: "i".into(),
                pattern: Pattern::fast(Rate::accel(1.0, 3.0, 4.0), Pattern::steps([Some(1.0)])),
                lanes: Vec::new(),
                start,
                bars: Some(4.0),
                repeat,
                choice: None,
                // The same curve the pattern was folded with, which is what
                // `play` writes: one of them places the notes and the other
                // tells a voice how fast it is being played.
                rate: Rate::accel(1.0, 3.0, 4.0),
            }],
            origin,
            choices: Vec::new(),
        }
    }

    fn onsets_of(pats: &Patterns, span: Span) -> Vec<f64> {
        let mut out: Vec<f64> = pats.query(span).iter().map(|b| b.event.begin).collect();
        out.sort_by(|a, b| a.partial_cmp(b).unwrap());
        out
    }

    /// The point of anchoring: a section placed later accelerates from *its*
    /// first note. Anchored at the origin instead it would open at whatever
    /// rate the curve had already reached, which for anything but the first
    /// section in a file is its end rate.
    #[test]
    fn a_curve_starts_where_its_section_does() {
        let at_origin = onsets_of(&accelerating(0.0, 0.0, None), Span::new(0.0, 8.0));
        let later = onsets_of(&accelerating(3.0, 0.0, None), Span::new(0.0, 12.0));

        assert_eq!(at_origin.len(), 8, "eight passes in four bars: {at_origin:?}");
        assert_eq!(later.len(), at_origin.len());
        for (early, late) in at_origin.iter().zip(&later) {
            assert!((early + 3.0 - late).abs() < 1e-9, "{at_origin:?} vs {later:?}");
        }
    }

    /// The origin is rounded up to a downbeat before anything is placed, and
    /// the curve has to start from the same figure the window opens at — a
    /// half-bar disagreement between the two would have the section opening
    /// mid-accelerando.
    #[test]
    fn a_curve_starts_from_the_downbeat_the_window_opens_on() {
        let ons = onsets_of(&accelerating(0.0, 4.3, None), Span::new(0.0, 20.0));
        assert_eq!(ons.len(), 8);
        assert_eq!(ons[0], 5.0, "the first note is on the downbeat: {ons:?}");
        // Its gap is the widest of them, and still short of the whole bar 1x
        // would give: the rate is already climbing across that first note, so
        // the opening rate is where the curve starts rather than a speed any
        // one gap is held at.
        let gaps: Vec<f64> = ons.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps[0] < 1.0 && gaps[0] > 0.8, "opens near 1x: {gaps:?}");
        assert!(gaps.iter().skip(1).all(|g| *g < gaps[0]), "the first gap is the widest: {gaps:?}");
    }

    /// Each repetition of a `wthen` window runs the curve afresh. Measured from
    /// the origin instead, every pass after the first would sit at the end rate
    /// — the accelerando would happen once and never again.
    #[test]
    fn a_repeating_window_accelerates_again_each_time_around() {
        let pats = accelerating(0.0, 0.0, Some(4.0));
        let first = onsets_of(&pats, Span::new(0.0, 4.0));
        let second = onsets_of(&pats, Span::new(4.0, 8.0));

        assert_eq!(first.len(), 8);
        assert_eq!(second.len(), 8);
        for (a, b) in first.iter().zip(&second) {
            assert!((a + 4.0 - b).abs() < 1e-9, "{first:?} vs {second:?}");
        }
    }

    // ---- a section's own grid ----

    /// A section whose pass does not divide the bar it was placed at: five
    /// steps over two and a half bars, opening at bar 1. A whole pass long, so
    /// it should be all five steps and no more.
    fn offset_section(start: f64) -> Patterns {
        Patterns {
            bindings: vec![Binding {
                instrument: "i".into(),
                pattern: Pattern::fast(
                    Rate::Fixed(1.0 / 2.5),
                    Pattern::steps([Some(1.0), Some(2.0), Some(3.0), Some(4.0), Some(5.0)]),
                ),
                lanes: vec![lane("cut", (1..=5).map(|i| Some(i as f64 * 10.0)).collect())],
                start,
                bars: Some(2.5),
                repeat: None,
                choice: None,
                rate: Rate::Fixed(1.0),
            }],
            origin: 0.0,
            choices: Vec::new(),
        }
    }

    /// The whole point of a section: `play_once` plays *the pattern*, from its
    /// first step. Laid on the transport's grid instead, a pass that does not
    /// divide the offset opens partway through itself — five steps placed a bar
    /// in would sound its third, fourth, fifth, and only then its first two.
    #[test]
    fn a_section_starts_at_its_own_first_step() {
        let placed = offset_section(1.0);
        let evs = placed.query(Span::new(0.0, 8.0));
        let values: Vec<f64> = evs.iter().map(|e| e.event.value).collect();
        assert_eq!(values, vec![1.0, 2.0, 3.0, 4.0, 5.0], "one pass, in order");
        assert_eq!(evs[0].event.begin, 1.0, "and it begins where the section does");
    }

    /// And its lane with it, which is the audible half: a volume ramp under a
    /// section is a ramp, not a ramp starting three quarters of the way up.
    #[test]
    fn a_lane_starts_at_its_first_value_wherever_the_section_sits() {
        for start in [0.0, 1.0, 3.0, 8.0] {
            let cuts: Vec<f64> = offset_section(start)
                .query(Span::new(0.0, 16.0))
                .iter()
                .map(|e| e.args[0].1)
                .collect();
            assert_eq!(cuts, vec![10.0, 20.0, 30.0, 40.0, 50.0], "placed at bar {start}");
        }
    }

    /// A plain `play` is the exception, and stays one: it was never placed, so
    /// it keeps the transport's grid and a re-eval does not re-phase a loop
    /// that is already rotating against the bar. Two thirds of a bar to a pass,
    /// queried across a later bar: the onsets sit where the grid puts them
    /// whatever origin the eval wrote.
    #[test]
    fn a_plain_play_keeps_the_performances_grid() {
        let running = |origin: f64| Patterns {
            bindings: vec![Binding {
                instrument: "i".into(),
                pattern: Pattern::fast(1.5, Pattern::steps([Some(1.0), Some(2.0)])),
                lanes: Vec::new(),
                start: 0.0,
                bars: None,
                repeat: None,
                choice: None,
                rate: Rate::Fixed(1.0),
            }],
            origin,
            choices: Vec::new(),
        };

        let fresh = onsets_of(&running(0.0), Span::new(6.0, 8.0));
        assert_eq!(fresh.len(), 6, "three onsets a bar: {fresh:?}");
        for origin in [3.2, 4.0, 5.7] {
            assert_eq!(onsets_of(&running(origin), Span::new(6.0, 8.0)), fresh,
                       "re-evaluated at {origin}");
        }
    }

    /// A lane is read by position, and the position has to be counted on the
    /// clock the note was placed on. Counted from the origin, a section three
    /// bars in would start partway down its own lane.
    #[test]
    fn a_lane_under_a_curve_starts_at_its_first_value() {
        let mut pats = accelerating(3.0, 0.0, None);
        pats.bindings[0].lanes = vec![lane("cut", vec![Some(10.0), Some(20.0)])];

        let cuts: Vec<f64> = pats.query(Span::new(0.0, 12.0)).iter().map(|e| e.args[0].1).collect();
        assert_eq!(cuts.len(), 8);
        assert_eq!(cuts[0], 10.0, "the first note takes the first value");
        assert_eq!(&cuts[..4], &[10.0, 20.0, 10.0, 20.0]);
    }

    /// Every event says how fast its binding was running when it began, which
    /// is what a voice divides the transport's beat by. A curve says something
    /// different for every note, and it climbs the way the placement does — the
    /// first note is near the 1x it starts from and the last near the 3x it
    /// reaches.
    #[test]
    fn an_event_reports_the_rate_it_was_played_at() {
        let pats = accelerating(0.0, 0.0, None);
        let mut events = pats.query(Span::new(0.0, 8.0));
        events.sort_by(|a, b| a.event.begin.partial_cmp(&b.event.begin).unwrap());

        let rates: Vec<f64> = events.iter().map(|e| e.rate).collect();
        assert_eq!(rates.len(), 8);
        assert_eq!(rates[0], 1.0, "the first note is where the curve starts: {rates:?}");
        for pair in rates.windows(2) {
            assert!(pair[1] > pair[0], "the rate should climb: {rates:?}");
        }
        assert!(*rates.last().unwrap() < 3.0, "and never past its end: {rates:?}");
        // The rate at a note is read off the same curve its placement was, so
        // the gap after it is bracketed by the two ends of that gap: a pass of
        // a one-note pattern would take 1/rate bars at a steady speed, and
        // the speed here is climbing the whole way across it.
        let begins: Vec<f64> = events.iter().map(|e| e.event.begin).collect();
        for (i, gap) in begins.windows(2).map(|w| w[1] - w[0]).enumerate() {
            assert!(
                gap <= 1.0 / rates[i] && gap >= 1.0 / rates[i + 1],
                "note {i}: gap {gap} against rates {} to {}", rates[i], rates[i + 1],
            );
        }
    }

    /// One speed throughout is that speed at every note, wherever the binding
    /// sits and whatever it is anchored to.
    #[test]
    fn a_fixed_rate_reaches_every_event_unchanged() {
        let mut pats = accelerating(0.0, 0.0, None);
        pats.bindings[0].pattern = Pattern::fast(2.0, Pattern::steps([Some(1.0)]));
        pats.bindings[0].rate = Rate::Fixed(2.0);

        let rates: Vec<f64> = pats.query(Span::new(0.0, 4.0)).iter().map(|e| e.rate).collect();
        assert_eq!(rates.len(), 8, "two passes a bar over four bars");
        assert!(rates.iter().all(|r| *r == 2.0), "{rates:?}");
    }

    // ---- repeating windows and choice ----

    /// One arm of a two-armed choice, sounding once per bar.
    fn arm(instrument: &str, group: usize, index: usize, period: f64) -> Binding {
        Binding {
            instrument: instrument.into(),
            pattern: Pattern::steps([Some(1.0)]),
            lanes: Vec::new(),
            start: 0.0,
            bars: Some(1.0),
            repeat: Some(period),
            choice: Some(ChoiceRef { group, arm: index }),
            rate: Rate::Fixed(1.0),
        }
    }

    fn two_armed(seed: u64, weights: Vec<f64>) -> Patterns {
        Patterns {
            bindings: vec![arm("a", 0, 0, 1.0), arm("b", 0, 1, 1.0)],
            origin: 0.0,
            choices: vec![ChoiceGroup { weights, seed }],
        }
    }

    /// Which instrument sounded in each of the first `n` bars.
    fn drawn(pats: &Patterns, n: i64) -> Vec<String> {
        (0..n)
            .map(|i| {
                let evs = pats.query(Span::new(i as f64, i as f64 + 1.0));
                assert_eq!(evs.len(), 1, "exactly one arm sounds in bar {i}");
                evs[0].instrument.clone()
            })
            .collect()
    }

    /// The property the whole design rests on: of a choice's arms, exactly one
    /// sounds each time round — never both, never neither.
    #[test]
    fn exactly_one_arm_sounds_per_repetition() {
        for seed in [0u64, 1, 0x5EED, u64::MAX] {
            let pats = two_armed(seed, vec![1.0, 1.0]);
            // `drawn` asserts the count; reaching the end is the test.
            assert_eq!(drawn(&pats, 32).len(), 32);
        }
    }

    /// A choice really does come back around: over enough repetitions both
    /// arms are drawn, which a window that opened once could not manage.
    #[test]
    fn a_choice_rerolls_rather_than_settling() {
        let pats = two_armed(0x5EED, vec![1.0, 1.0]);
        let seen = drawn(&pats, 64);
        assert!(seen.iter().any(|i| i == "a"), "arm a never came up");
        assert!(seen.iter().any(|i| i == "b"), "arm b never came up");
    }

    /// Why the draw is a hash and not a running RNG.
    ///
    /// The scheduler queries an overlapping lookahead window every pass, so the
    /// same bar is asked about several times. A stateful generator would
    /// answer differently each time and the note would flicker; this asks the
    /// same span three ways and insists all three agree.
    #[test]
    fn the_same_repetition_draws_the_same_arm_however_it_is_queried() {
        let pats = two_armed(0x9E3779B9, vec![1.0, 1.0]);

        // One sweep, bar by bar.
        let once = drawn(&pats, 24);

        // The whole span in a single query.
        let mut whole: Vec<(f64, String)> = pats
            .query(Span::new(0.0, 24.0))
            .into_iter()
            .map(|e| (e.event.begin, e.instrument))
            .collect();
        whole.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let whole: Vec<String> = whole.into_iter().map(|(_, i)| i).collect();
        assert_eq!(once, whole, "one big query disagreed with bar-by-bar");

        // Overlapping windows that straddle every boundary, as the lookahead
        // does at speed.
        for bar in 0..23 {
            let straddle = pats.query(Span::new(bar as f64 + 0.5, bar as f64 + 1.5));
            assert_eq!(straddle.len(), 1, "bar {bar} boundary");
            assert_eq!(
                straddle[0].instrument, once[bar + 1],
                "a window straddling bar {bar} drew a different arm"
            );
        }
    }

    /// Weights are relative and need not be fractions; over enough draws the
    /// split tracks them. Loose bounds — this is testing that the weighting is
    /// wired up at all, not the quality of the hash.
    #[test]
    fn weights_bias_the_draw() {
        let pats = two_armed(0xC0FFEE, vec![9.0, 1.0]);
        let seen = drawn(&pats, 1000);
        let a = seen.iter().filter(|i| *i == "a").count();
        assert!((820..=980).contains(&a), "expected ~900 of 1000, got {a}");
    }

    /// A weight of zero is never drawn, and the arm that keeps it is simply
    /// never heard.
    #[test]
    fn a_zero_weight_is_never_drawn() {
        let pats = two_armed(42, vec![1.0, 0.0]);
        assert!(drawn(&pats, 200).iter().all(|i| i == "a"));
    }

    /// `maybe`'s silent arm owns no bindings, so the choice can come up empty —
    /// and does, at roughly its weight.
    #[test]
    fn a_choice_may_draw_an_arm_that_owns_nothing() {
        let pats = Patterns {
            bindings: vec![arm("a", 0, 0, 1.0)],
            origin: 0.0,
            // Arm 1 is silence: nothing refers to it.
            choices: vec![ChoiceGroup { weights: vec![0.25, 0.75], seed: 7 }],
        };
        let sounded = (0..400)
            .filter(|i| !pats.query(Span::new(*i as f64, *i as f64 + 1.0)).is_empty())
            .count();
        assert!((60..=140).contains(&sounded), "expected ~100 of 400, got {sounded}");
    }

    /// A repeating window with no choice on it simply comes back every period,
    /// which is what the arm gating is layered on top of.
    #[test]
    fn a_repeating_window_reopens_every_period() {
        let pats = Patterns {
            bindings: vec![Binding {
                instrument: "a".into(),
                pattern: Pattern::steps([Some(1.0)]),
                lanes: Vec::new(),
                start: 0.0,
                // Sounds for one bar in every four.
                bars: Some(1.0),
                repeat: Some(4.0),
                choice: None,
                rate: Rate::Fixed(1.0),
            }],
            origin: 0.0,
            choices: Vec::new(),
        };
        let onsets: Vec<f64> = pats
            .query(Span::new(0.0, 12.0))
            .into_iter()
            .map(|e| e.event.begin)
            .collect();
        assert_eq!(onsets, vec![0.0, 4.0, 8.0]);
    }

    /// A binding that repeats still respects where it opens.
    #[test]
    fn a_repeating_window_does_not_open_before_its_start() {
        let pats = Patterns {
            bindings: vec![Binding {
                instrument: "a".into(),
                pattern: Pattern::steps([Some(1.0)]),
                lanes: Vec::new(),
                start: 3.0,
                bars: Some(1.0),
                repeat: Some(2.0),
                choice: None,
                rate: Rate::Fixed(1.0),
            }],
            origin: 0.0,
            choices: Vec::new(),
        };
        let onsets: Vec<f64> = pats
            .query(Span::new(0.0, 8.0))
            .into_iter()
            .map(|e| e.event.begin)
            .collect();
        assert_eq!(onsets, vec![3.0, 5.0, 7.0]);
    }
}
