//! Building one voice: an instrument function plus an event value, run through
//! the ordinary lowering and realization pipeline.

use fundsp::net::Net;

use crate::scree_graph::realizer::{realize, tail_secs};
use crate::lowerer::lower::lower_voice;
use crate::parser::parser::{Arg, ScreeItem, Expr, Ident};
use crate::pattern::patterns::PAN;
use crate::samples::Samples;
use crate::scheduler::clock::Meter;

/// The instrument definitions from the most recent eval.
///
/// These are the `fn` items of the program, kept verbatim so a voice can be
/// lowered on demand. An eval replaces this wholesale, exactly like `Patterns`.
#[derive(Clone, Default)]
pub struct Instruments {
    pub defs: Vec<ScreeItem>,
    /// The buffers the eval loaded, so an instrument that reads one can be
    /// lowered here — on the scheduler thread, per note, where opening a file
    /// is not an option.
    pub samples: Samples,
}

impl Instruments {
    /// Keep only the function definitions; everything else in a program is
    /// the persistent graph's business, not a voice's.
    pub fn from_program(items: &[ScreeItem]) -> Instruments {
        Instruments {
            defs: items
                .iter()
                .filter(|i| matches!(i, ScreeItem::Function { .. }))
                .cloned()
                .collect(),
            samples: Samples::default(),
        }
    }

    /// The same instruments, able to read the buffers an eval loaded.
    ///
    /// A separate step rather than a second parameter on `from_program` because
    /// the great majority of callers — every test that does not sample — have
    /// no buffers and nothing to say about them.
    pub fn with_samples(mut self, samples: Samples) -> Instruments {
        self.samples = samples;
        self
    }

    pub fn has(&self, name: &str) -> bool {
        self.param_count(name).is_some()
    }

    /// How many parameters an instrument declares, or `None` if there is no
    /// such instrument. A zero-parameter instrument is called with no
    /// arguments, so a fixed drum needs no placeholder parameter.
    pub fn param_count(&self, name: &str) -> Option<usize> {
        self.defs.iter().find_map(|i| match i {
            ScreeItem::Function { name: n, params, .. } if n.0 == name => Some(params.len()),
            _ => None,
        })
    }
}

/// A voice is written in mono and fanned to both channels, so the centre of
/// the stereo field is unity in each. fundsp's panner is equal-power — its two
/// gains are the cosine and sine of a quarter turn — which puts 1/√2 in each
/// channel at the centre, 3 dB below where an unpanned voice sits. Scaling the
/// whole law back up by √2 restores that: `pan: 0` leaves a voice exactly where
/// it was, and the price is paid at the extremes instead, where one channel
/// takes 3 dB of gain and the other is silent.
const CENTRE_UNITY: f32 = std::f32::consts::SQRT_2;

/// A built voice: the network to play, and how long it goes on after its note.
///
/// The tail is the instrument's business rather than the pattern's — it comes
/// out of the envelopes inside it — but only the scheduler can act on it, by
/// giving the sequencer event that much more room than the note itself. Zero
/// for an instrument that ends with its note, which is most of them.
pub struct Voice {
    pub net: Net,
    pub tail_secs: f64,
}

/// Lower and realize `instrument(value, name: v, ...)` into a playable
/// 0-in / 2-out network.
///
/// Synthesizing a call and running the normal pipeline means voices get every
/// language feature for free — `for`, `if`, nested calls, the lot. Lane values
/// are passed by name for the same reason: a parameter no lane filled then
/// falls to its own default, evaluated in the callee's scope where the earlier
/// parameters are already bound.
///
/// `pan` is the exception: it is taken off the lanes rather than passed on, and
/// spent on where the finished voice sits rather than on how it sounds.
///
/// `beat_secs` is the quarter note of the clock this note is on — already
/// divided by the binding's rate, so this is a length the instrument can shape
/// itself against without knowing anything about how it came to be playing.
///
/// `meter` is the transport's, and reaches a voice for one reason: an
/// instrument may call `bpm`, which answers in bars per second. Nothing about
/// the note's own timing needs it — the pattern placed this note before the
/// voice existed.
pub fn build_voice(
    instruments: &Instruments,
    instrument: &str,
    value: f64,
    lanes: &[(String, f64)],
    dur_secs: f64,
    beat_secs: f64,
    meter: Meter,
) -> Result<Voice, String> {
    let Some(params) = instruments.param_count(instrument) else {
        return Err(format!("no instrument named `{instrument}`"));
    };

    // An instrument that declares no parameters is called with none — the
    // event's value is simply unused, which is what `\` in a pattern means.
    let mut args = if params == 0 { vec![] } else { vec![Arg::positional(Expr::Num(value))] };
    args.extend(
        lanes.iter()
            .filter(|(name, _)| name != PAN)
            .map(|(name, v)| Arg::named(name, Expr::Num(*v))),
    );

    let mut items = instruments.defs.clone();
    items.push(ScreeItem::Expr(Expr::Call {
        func: Ident(instrument.to_string()),
        args,
    }));

    let lowered = lower_voice(&items, dur_secs, beat_secs, meter, instruments.samples.clone())?;
    let net = realize(&lowered.graph)?;

    // `play` refuses a lane given twice, so at most one of these exists.
    let pan = lanes.iter().find(|(name, _)| name == PAN).map(|(_, v)| *v);
    Ok(Voice {
        net: place(net, pan.unwrap_or(0.0)),
        tail_secs: tail_secs(&lowered.graph, dur_secs),
    })
}

/// Put a finished voice somewhere in the stereo field.
///
/// A voice arrives 0-in / 2-out with the same mono signal on both channels, so
/// either one of them is the mono the panner wants; the other is left dangling,
/// which costs nothing — the inner network renders its chain once whatever we
/// read from it.
///
/// A position that is not a number leaves the voice centred, the same way a
/// nonsense `legato` leaves a note its natural length: a lane is data, and one
/// bad value in it should cost a note its placement rather than its existence.
fn place(voice: Net, pan: f64) -> Net {
    // Centre is what a voice does already, to the last bit — and nothing is as
    // certainly untouched as a graph that was not touched.
    if !pan.is_finite() || pan == 0.0 {
        return voice;
    }

    let mut placed = Net::new(0, 2);
    let mono = placed.push(Box::new(voice));
    let panner = placed.push(Box::new(fundsp::prelude::pan(pan as f32) * CENTRE_UNITY));

    placed.connect(mono, 0, panner, 0);
    placed.connect_output(panner, 0, 0);
    placed.connect_output(panner, 1, 1);
    placed
}

/// `build_voice` at the default tempo.
///
/// Every test module below imports this *as* `build_voice`, so the tests that
/// have nothing to say about the beat — which is all of them that existed
/// before `qvs` did — go on reading as they did. A test about the beat calls
/// `build_voice` by its real name and says what it wants.
#[cfg(test)]
fn build_voice_at_default_tempo(
    instruments: &Instruments,
    instrument: &str,
    value: f64,
    lanes: &[(String, f64)],
    dur_secs: f64,
) -> Result<Voice, String> {
    build_voice(
        instruments, instrument, value, lanes, dur_secs,
        crate::lowerer::lower::DEFAULT_BEAT_SECS, Meter::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::build_voice_at_default_tempo as build_voice;
    use crate::lowerer::lower::DEFAULT_BEAT_SECS;
    use crate::parser::parser::parse;
    use fundsp::prelude64::AudioUnit;

    fn instruments(src: &str) -> Instruments {
        Instruments::from_program(&parse(src.to_string()).expect("parse failed"))
    }

    #[test]
    fn only_function_items_are_kept() {
        let ins = instruments("fn kick(f) = sin(f)\nlet x = 3\nsin(220)\n");
        assert_eq!(ins.defs.len(), 1);
        assert!(ins.has("kick"));
        assert!(!ins.has("nope"));
    }

    /// A voice is always 0-in / 2-out, which is what `Sequencer::push` asserts.
    #[test]
    fn voice_is_stereo_and_sourceless() {
        let ins = instruments("fn kick(f) = sin(f) / 4\n");
        let net = build_voice(&ins, "kick", 220.0, &[], 1.0).expect("should build").net;
        assert_eq!(net.inputs(), 0);
        assert_eq!(net.outputs(), 2);
    }

    /// The event value really reaches the instrument's parameter.
    #[test]
    fn event_value_reaches_the_instrument() {
        let ins = instruments("fn tone(f) = sin(f)\n");
        let mut net = build_voice(&ins, "tone", 110.0, &[], 1.0).expect("should build").net;
        net.set_sample_rate(44100.0);

        let samples: Vec<f32> = (0..44100).map(|_| net.get_mono()).collect();
        assert!(samples.iter().all(|s| s.is_finite()));

        // A 110 Hz sine crosses zero going positive 110 times per second.
        let crossings = samples
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!(
            (crossings as i64 - 110).abs() <= 1,
            "expected ~110 rising zero crossings, got {crossings}"
        );
    }

    /// The beat reaches the instrument, and it is the one this voice was built
    /// with rather than the transport's — which is how a `play` at rate 2 gets
    /// an instrument that sweeps twice as fast without being written twice.
    ///
    /// Counted in zero crossings, exactly as the event value is above: a beat
    /// of 1/110 s is 110 Hz, and an oscillator at `qvh` really runs there.
    #[test]
    fn the_beat_reaches_the_instrument() {
        let ins = instruments("fn tone() = sin(qvh)\n");
        let mut net = super::build_voice(&ins, "tone", 0.0, &[], 1.0, 1.0 / 110.0, Meter::default())
            .expect("should build")
            .net;
        net.set_sample_rate(44100.0);

        let samples: Vec<f32> = (0..44100).map(|_| net.get_mono()).collect();
        let crossings = samples
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!(
            (crossings as i64 - 110).abs() <= 1,
            "expected ~110 rising zero crossings, got {crossings}"
        );
    }

    /// A beat that is not a length leaves both names unbound rather than
    /// filling the voice with NaNs. Nothing produces one today — the clock
    /// refuses a tempo like this and so does `accel` — and this is what makes
    /// the failure legible if anything ever does.
    #[test]
    fn an_impossible_beat_is_an_unbound_name_rather_than_silence() {
        let ins = instruments("fn tone() = sin(qvh)\n");
        for beat in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            match super::build_voice(&ins, "tone", 0.0, &[], 1.0, beat, Meter::default()) {
                Err(e) => assert!(e.contains("unbound name: qvh"), "beat {beat}: {e}"),
                Ok(_) => panic!("beat {beat} should not have built a voice"),
            }
        }
    }

    /// Instruments can use the full language, not a restricted subset.
    #[test]
    fn voices_may_use_language_features() {
        let ins = instruments("fn rich(f) = for i in 1..=3 { sin(f * i) / 6 }\n");
        let net = build_voice(&ins, "rich", 110.0, &[], 1.0).expect("should build").net;
        assert_eq!(net.outputs(), 2);
    }

    /// A voice is lowered afresh for each note, so a random draw written inside
    /// an instrument is a new number every time it sounds — the counterpart to
    /// a draw in a *pattern*, which settles once per eval. Both are documented
    /// behaviour, and this is the half that only exists here.
    #[test]
    fn a_draw_inside_an_instrument_is_rerolled_per_note() {
        // Detune is inaudible but moves the pitch, so counting zero crossings
        // reads back what the draw was.
        let ins = instruments("fn tone(f) = sin(f + randi(0, 40))\n");
        let crossings = |_| {
            let mut net = build_voice(&ins, "tone", 200.0, &[], 1.0).expect("should build").net;
            net.set_sample_rate(44100.0);
            let samples: Vec<f32> = (0..44100).map(|_| net.get_mono()).collect();
            samples.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
        };
        let counts: std::collections::HashSet<usize> = (0..12).map(crossings).collect();
        assert!(counts.len() > 1, "every note drew the same number: {counts:?}");
        assert!(counts.iter().all(|c| (200..=240).contains(c)), "out of range: {counts:?}");
    }

    #[test]
    fn unknown_instrument_is_an_error() {
        let ins = instruments("fn kick(f) = sin(f)\n");
        // Net has no Debug, so unwrap_err() is unavailable here.
        let err = match build_voice(&ins, "snare", 1.0, &[], 1.0) {
            Err(e) => e,
            Ok(_) => panic!("expected an error for an unknown instrument"),
        };
        assert!(err.contains("no instrument"), "got: {err}");
    }

    /// Lowering errors surface as errors, not panics — the scheduler logs and
    /// keeps running.
    #[test]
    fn broken_instrument_reports_an_error() {
        let ins = instruments("fn bad(f) = nope(f)\n");
        assert!(build_voice(&ins, "bad", 1.0, &[], 1.0).is_err());
    }

    // ---- lanes ----

    fn rising_crossings(ins: &Instruments, lanes: &[(String, f64)]) -> usize {
        let mut net = build_voice(ins, "tone", 110.0, lanes, 1.0).expect("should build").net;
        net.set_sample_rate(44100.0);
        let s: Vec<f32> = (0..44100).map(|_| net.get_mono()).collect();
        s.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
    }

    /// A lane value really reaches the parameter it names: doubling `mul`
    /// doubles the pitch of a 110 Hz tone.
    #[test]
    fn a_lane_value_reaches_its_parameter() {
        let ins = instruments("fn tone(n, mul = 1) = sin(n * mul)\n");

        let plain = rising_crossings(&ins, &[]);
        let doubled = rising_crossings(&ins, &[("mul".to_string(), 2.0)]);

        assert!((plain as i64 - 110).abs() <= 1, "expected ~110, got {plain}");
        assert!((doubled as i64 - 220).abs() <= 1, "expected ~220, got {doubled}");
    }

    /// Lanes bind by name, so the order the scheduler happens to collect them
    /// in cannot matter.
    #[test]
    fn lane_order_does_not_matter() {
        let ins = instruments("fn tone(n, mul = 1, div = 1) = sin(n * mul / div)\n");

        let forward = rising_crossings(
            &ins, &[("mul".to_string(), 4.0), ("div".to_string(), 2.0)]);
        let backward = rising_crossings(
            &ins, &[("div".to_string(), 2.0), ("mul".to_string(), 4.0)]);

        assert_eq!(forward, backward);
        assert!((forward as i64 - 220).abs() <= 1, "expected ~220, got {forward}");
    }

    /// A parameter no lane filled falls to its own default — which is what a
    /// resting lane relies on.
    #[test]
    fn an_unfilled_parameter_uses_its_default() {
        let ins = instruments("fn tone(n, mul = 2) = sin(n * mul)\n");
        assert!((rising_crossings(&ins, &[]) as i64 - 220).abs() <= 1);
    }

    /// A default that reads an earlier parameter still works when a later lane
    /// is supplied by name — the reason lanes are passed as named arguments
    /// rather than flattened into positions.
    #[test]
    fn a_default_may_read_the_event_value() {
        let ins = instruments("fn tone(n, hz = n * 2, mul = 1) = sin(hz * mul)\n");
        assert!((rising_crossings(&ins, &[("mul".to_string(), 1.0)]) as i64 - 220).abs() <= 1);
    }

    // ---- pan ----

    fn pan_lane(v: f64) -> Vec<(String, f64)> {
        vec![(PAN.to_string(), v)]
    }

    /// Peak level in each channel over a tenth of a second. A voice is two
    /// channels of the same signal until it is panned, so `get_mono` would
    /// hide the whole feature.
    fn peaks(lanes: &[(String, f64)]) -> (f32, f32) {
        let ins = instruments("fn tone(n) = sin(n)\n");
        let mut net = build_voice(&ins, "tone", 110.0, lanes, 1.0).expect("should build").net;
        net.set_sample_rate(44100.0);

        let mut frame = [0.0f32; 2];
        let (mut left, mut right) = (0.0f32, 0.0f32);
        for _ in 0..4410 {
            net.tick(&[], &mut frame);
            left = left.max(frame[0].abs());
            right = right.max(frame[1].abs());
        }
        (left, right)
    }

    /// The property the whole gain law was chosen for: adding `pan: 0` to a
    /// line that was playing must not change how loud it is.
    #[test]
    fn the_centre_is_where_an_unpanned_voice_already_was() {
        let (unpanned_l, unpanned_r) = peaks(&[]);
        let (centred_l, centred_r) = peaks(&pan_lane(0.0));

        assert!((unpanned_l - unpanned_r).abs() < 1e-6, "a bare voice is centred");
        assert!((centred_l - unpanned_l).abs() < 1e-3, "{centred_l} vs {unpanned_l}");
        assert!((centred_r - unpanned_r).abs() < 1e-3, "{centred_r} vs {unpanned_r}");
    }

    #[test]
    fn hard_left_empties_the_right_channel() {
        let (left, right) = peaks(&pan_lane(-1.0));
        assert!(left > 0.9, "left should carry the voice, got {left}");
        assert!(right < 1e-6, "right should be silent, got {right}");
    }

    #[test]
    fn hard_right_empties_the_left_channel() {
        let (left, right) = peaks(&pan_lane(1.0));
        assert!(left < 1e-6, "left should be silent, got {left}");
        assert!(right > 0.9, "right should carry the voice, got {right}");
    }

    /// Halfway leans without abandoning the other side — the audible difference
    /// between a pan and a mute.
    #[test]
    fn a_partial_pan_leans() {
        let (left, right) = peaks(&pan_lane(0.5));
        assert!(right > left, "should lean right: {left} vs {right}");
        assert!(left > 0.1, "the left channel should still be there, got {left}");
    }

    /// Equal power: the two gains square to the same total wherever the voice
    /// sits, so sweeping a pan lane across a phrase does not swell or dip.
    #[test]
    fn the_law_holds_its_power_across_the_field() {
        let power = |p: f64| {
            let (l, r) = peaks(&pan_lane(p));
            l * l + r * r
        };
        let centre = power(0.0);
        for p in [-1.0, -0.75, -0.3, 0.3, 0.75, 1.0] {
            let here = power(p);
            assert!(
                (here - centre).abs() < 0.05 * centre,
                "power at {p} was {here}, centre is {centre}"
            );
        }
    }

    /// Beyond the ends is still an end, rather than a gain of its own.
    #[test]
    fn a_pan_past_the_ends_clamps() {
        let (left, right) = peaks(&pan_lane(-4.0));
        let (hard_l, hard_r) = peaks(&pan_lane(-1.0));
        assert!((left - hard_l).abs() < 1e-6 && (right - hard_r).abs() < 1e-6);
    }

    /// A lane is data. One bad value should cost a note its placement, not its
    /// existence — and must never put a NaN into the audio thread.
    #[test]
    fn a_bad_pan_value_leaves_the_voice_centred() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let (left, right) = peaks(&pan_lane(bad));
            assert!(left > 0.9 && right > 0.9, "pan {bad} silenced the voice");
            assert!((left - right).abs() < 1e-6, "pan {bad} moved the voice");
        }
    }

    /// `pan` is spent on the voice, so it must not also be handed to the
    /// instrument — which has no parameter of that name and would refuse it.
    #[test]
    fn pan_does_not_reach_the_instrument() {
        let ins = instruments("fn tone(n) = sin(n)\n");
        assert!(build_voice(&ins, "tone", 110.0, &pan_lane(-1.0), 1.0).is_ok());
    }

    /// And taking it off the lanes must not disturb the ones that remain.
    #[test]
    fn pan_sits_alongside_ordinary_lanes() {
        let ins = instruments("fn tone(n, mul = 1) = sin(n * mul)\n");
        let lanes = vec![(PAN.to_string(), -1.0), ("mul".to_string(), 2.0)];
        assert!((rising_crossings(&ins, &lanes) as i64 - 220).abs() <= 1);
    }

    // ---- sampling ----

    /// Instruments that can read one buffer, named `break.wav`: a ramp from
    /// -1 to 1 over one second, so what a voice renders says where in the
    /// buffer it read.
    fn sampling_instruments(src: &str) -> Instruments {
        use std::sync::Arc;
        let mut wave = fundsp::wave::Wave::new(1, 1000.0);
        for i in 0..1000 {
            wave.push(i as f32 / 500.0 - 1.0);
        }
        let samples = crate::samples::Samples::from_pairs(
            [("break.wav".to_string(), Arc::new(wave))]);
        instruments(src).with_samples(samples)
    }

    /// An instrument sees only `fn`s, so a buffer bound by a top-level `let` is
    /// out of its reach — it has to name the file itself.
    ///
    /// That costs nothing: one path is one buffer however many `load`s write
    /// it, so the inline spelling shares the audio with everything else that
    /// reads the same file.
    #[test]
    fn an_instrument_names_its_own_file_rather_than_a_top_level_let() {
        let bound = sampling_instruments(
            "let amen = load(\"break.wav\")\nfn chop(n) = sample(amen, ramp(n))\n");
        let err = match build_voice(&bound, "chop", 1.0, &[], 1.0) {
            Err(e) => e,
            Ok(_) => panic!("a top-level `let` should not be visible in a voice"),
        };
        assert!(err.contains("unbound name: amen"), "got: {err}");

        // The spelling that works, and the one the reference gives.
        let inline = sampling_instruments(
            "fn chop(n) = sample(load(\"break.wav\"), ramp(n))\n");
        assert!(build_voice(&inline, "chop", 1.0, &[], 1.0).is_ok());
    }

    /// The point of carrying `Samples` on `Instruments`: a voice is lowered
    /// here, per note, on the scheduler thread — so the buffer has to be in
    /// hand already. Without it this call fails rather than reading a disk.
    #[test]
    fn a_sampling_instrument_builds_into_a_voice() {
        let ins = sampling_instruments(
            "fn chop(n) = sample(load(\"break.wav\"), ramp(n))\n");

        let mut net = build_voice(&ins, "chop", 1.0, &[], 1.0).expect("should build").net;
        assert_eq!(net.outputs(), 2);
        net.set_sample_rate(44100.0);

        let s: Vec<f32> = (0..44100).map(|_| net.get_mono()).collect();
        assert!(s.iter().all(|v| v.is_finite()), "no NaNs may reach the output");
        let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.5, "the buffer should be audible, peak {peak}");
    }

    /// Instruments built without buffers must not silently read one — the
    /// error belongs at build time, where it is logged, rather than as silence.
    #[test]
    fn an_instrument_whose_buffer_was_never_loaded_fails_to_build() {
        let ins = instruments("fn chop(n) = sample(load(\"break.wav\"), ramp(n))\n");
        let err = match build_voice(&ins, "chop", 1.0, &[], 1.0) {
            Err(e) => e,
            Ok(_) => panic!("expected an error without the buffer"),
        };
        assert!(err.contains("was not loaded"), "got: {err}");
    }

    /// A per-note lane choosing where in the buffer to read: the whole chopping
    /// idiom, driven from a pattern.
    #[test]
    fn a_lane_can_choose_which_part_of_a_buffer_a_note_reads() {
        let ins = sampling_instruments(
            "fn chop(n, at = 0) = sample(load(\"break.wav\"), at + ramp(n) * 0.25)\n");

        let render = |at: f64| {
            let lanes = vec![("at".to_string(), at)];
            let mut net = build_voice(&ins, "chop", 1.0, &lanes, 1.0).expect("should build").net;
            net.set_sample_rate(44100.0);
            // A tenth of a second in, well inside the quarter being read.
            (0..4410).map(|_| net.get_mono()).last().expect("a sample")
        };

        // The buffer rises steadily, so a later slice reads a higher value.
        let (early, late) = (render(0.0), render(0.75));
        assert!(late > early, "0.75 should read later than 0.0: {early} vs {late}");
    }

    /// The shape a drum out of a sample is actually written in: no parameter,
    /// a portion of a file, an envelope. It has to survive the scheduler
    /// thread's own lowering, which is where a buffer is hardest to reach.
    ///
    /// The buffer rises steadily from -1 to 1 across one second, so the first
    /// fifth of it sits well below zero — which is what says the reader is
    /// moving through the front of the file rather than parked somewhere.
    #[test]
    fn a_sliced_drum_builds_into_a_voice_and_sounds() {
        let ins = sampling_instruments(
            "fn slam() = slice(load(\"break.wav\"), 0, 0.2) * perc(0.001, 0.2)\n");

        let mut net = build_voice(&ins, "slam", 0.0, &[], 1.0).expect("should build").net;
        net.set_sample_rate(44100.0);
        let s: Vec<f32> = (0..44100).map(|_| net.get_mono()).collect();

        assert!(s.iter().all(|v| v.is_finite()), "no NaNs may reach the output");
        let onset = s[..2205].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(onset > 0.5, "the slice should sound immediately, peak {onset}");
    }

    /// And the portion really is the one named. A constant position — the shape
    /// `slice` exists to stop anyone writing — reads one sample forever, which
    /// is a DC offset and all but silent once an envelope has had it.
    #[test]
    fn a_slice_moves_where_a_constant_position_would_sit_still() {
        let sliced = sampling_instruments(
            "fn slam() = slice(load(\"break.wav\"), 0, 0.2)\n");
        let stuck = sampling_instruments(
            "fn slam() = sample(load(\"break.wav\"), 0.2 / load(\"break.wav\").secs * 0.15)\n");

        let render = |ins: &Instruments| {
            let mut net = build_voice(ins, "slam", 0.0, &[], 1.0).expect("should build").net;
            net.set_sample_rate(44100.0);
            (0..8820).map(|_| net.get_mono()).collect::<Vec<f32>>()
        };

        let moving = render(&sliced);
        assert!(moving[0] < moving[8819], "the slice should advance through the buffer");

        let still = render(&stuck);
        assert_eq!(still[0], still[8819], "a constant position never moves");
    }

    /// A lane the instrument has no parameter for is refused at bind time, but
    /// the voice builder must not panic if one reaches it anyway.
    #[test]
    fn an_unknown_lane_is_an_error_not_a_panic() {
        let ins = instruments("fn tone(n) = sin(n)\n");
        let err = match build_voice(&ins, "tone", 110.0, &[("nope".to_string(), 1.0)], 1.0) {
            Err(e) => e,
            Ok(_) => panic!("expected an error for an unknown lane"),
        };
        assert!(err.contains("no parameter named 'nope'"), "got: {err}");
    }
}

#[cfg(test)]
mod envelope_voice_tests {
    use super::*;
    use super::build_voice_at_default_tempo as build_voice;
    use crate::lowerer::lower::lower;
    use crate::parser::parser::parse;
    use fundsp::prelude64::AudioUnit;

    const PROGRAM: &str = "\
fn kick(f) = sin(f) * perc(0.001, 0.25)
fn bass(f) = (saw(f) >> lowpass(800, 1)) * env(0.01, 0.15, 0.4, 0.2, dur)

play([55, `, 55, [55, 55]], kick)
play([110, 165], bass, 0.5)
";

    /// The full example: two enveloped instruments plus two patterns.
    #[test]
    fn the_example_program_lowers_and_binds() {
        let items = parse(PROGRAM.to_string()).expect("parse failed");
        let lowered = lower(&items).expect("lower failed");
        assert_eq!(lowered.bindings.len(), 2);

        let ins = Instruments::from_program(&items);
        assert!(ins.has("kick") && ins.has("bass"));
    }

    /// Both instruments build into playable voices, sound at once, and are
    /// finished by the time the voice's own lifetime is up — the note plus
    /// whatever tail it declared.
    #[test]
    fn enveloped_voices_start_immediately_and_decay() {
        let ins = Instruments::from_program(&parse(PROGRAM.to_string()).unwrap());

        for (name, freq, dur) in [("kick", 55.0, 1.0), ("bass", 110.0, 1.0)] {
            let voice = build_voice(&ins, name, freq, &[], dur).expect("should build");
            let mut net = voice.net;
            assert_eq!(net.outputs(), 2);
            net.set_sample_rate(44100.0);

            let frames = ((dur + voice.tail_secs) * 44100.0) as usize;
            let s: Vec<f32> = (0..frames).map(|_| net.get_mono()).collect();
            // Audible within the first 50 ms — no swallowed first note.
            let onset = s[..2205].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            // The last millisecond: the release lands exactly on the end of the
            // voice, so anything earlier is still legitimately ringing out.
            let tail = s[frames - 50..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(onset > 0.05, "{name} should sound immediately, got {onset}");
            assert!(tail < 0.02, "{name} should be quiet by the voice's end, got {tail}");
        }
    }

    /// A voice says how much longer than its note it needs, and the scheduler
    /// gives it that and no more. The two envelopes answer differently: the
    /// `env` in the bass always wants its release, while the `perc` in the
    /// kick wants nothing from a note its shape already fits inside.
    #[test]
    fn a_voice_reports_the_time_it_needs_after_its_note() {
        let ins = Instruments::from_program(&parse(PROGRAM.to_string()).unwrap());

        let kick = build_voice(&ins, "kick", 55.0, &[], 1.0).expect("should build");
        let bass = build_voice(&ins, "bass", 110.0, &[], 1.0).expect("should build");

        assert_eq!(kick.tail_secs, 0.0, "a second is room enough for a 0.251 s drum");
        assert!((bass.tail_secs - 0.2).abs() < 1e-6, "got {}", bass.tail_secs);
    }

    /// `legato` shortens the held part of the note and leaves the release
    /// alone: the same instrument on a shorter note still rings out as long.
    #[test]
    fn a_shorter_note_keeps_the_same_release() {
        let ins = Instruments::from_program(&parse(PROGRAM.to_string()).unwrap());

        let long = build_voice(&ins, "bass", 110.0, &[], 1.0).expect("should build");
        let short = build_voice(&ins, "bass", 110.0, &[], 0.05).expect("should build");
        assert_eq!(long.tail_secs, short.tail_secs);
    }

    /// A drum on a step shorter than its own shape asks for the difference,
    /// so a slow kick on a fast pattern rings out instead of being clipped.
    #[test]
    fn a_drum_asks_for_room_only_when_the_step_is_too_short() {
        let ins = Instruments::from_program(&parse(PROGRAM.to_string()).unwrap());

        // perc(0.001, 0.25) is 0.251 s of shape, against a 0.125 s step.
        let cramped = build_voice(&ins, "kick", 55.0, &[], 0.125).expect("should build");
        assert!((cramped.tail_secs - 0.126).abs() < 1e-6, "got {}", cramped.tail_secs);

        let roomy = build_voice(&ins, "kick", 55.0, &[], 0.5).expect("should build");
        assert_eq!(roomy.tail_secs, 0.0, "half a second already covers it");
    }
}

#[cfg(test)]
mod kick_example_tests {
    use super::*;
    use super::build_voice_at_default_tempo as build_voice;
    use fundsp::prelude64::AudioUnit;

    /// A synth kick. The pattern number is the fundamental; everything else is
    /// shaped by envelopes inside the instrument.
    const KICK: &str = "\
fn kick(f) = {
  let sweep = f + f * 3 * perc(0.001, 0.05)
  let body  = sin(sweep) * perc(0.002, 0.4)
  let click = noise() * perc(0.001, 0.006)
  body * 0.9 + click * 0.1
}
";

    fn render(freq: f64, dur: f64, secs: f64) -> Vec<f32> {
        let ins = Instruments::from_program(
            &crate::parser::parser::parse(KICK.to_string()).expect("parse failed"),
        );
        let mut net = build_voice(&ins, "kick", freq, &[], dur).expect("should build").net;
        net.set_sample_rate(44100.0);
        (0..(secs * 44100.0) as usize).map(|_| net.get_mono()).collect()
    }

    fn rising_crossings(s: &[f32]) -> usize {
        s.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
    }

    #[test]
    fn the_kick_sounds_and_decays() {
        let s = render(50.0, 1.0, 1.0);
        assert!(s.iter().all(|v| v.is_finite()));

        let onset = s[..2205].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let tail = s[35000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(onset > 0.5, "should hit hard, peak {onset}");
        assert!(tail < 0.02, "should have decayed, peak {tail}");
        assert!(s.iter().all(|v| v.abs() <= 1.05), "should not clip badly");
    }

    /// The pitch envelope really sweeps: the first 50 ms contains far more
    /// bars than a later window of the same length.
    #[test]
    fn the_pitch_sweeps_downward() {
        let s = render(50.0, 1.0, 1.0);
        let early = rising_crossings(&s[..2205]);        // 0 - 50 ms
        let late = rising_crossings(&s[6615..8820]);     // 150 - 200 ms
        assert!(
            early > late * 2,
            "expected a downward sweep, early={early} late={late}"
        );
    }

    /// The pattern number scales the whole drum, so it is a real parameter.
    #[test]
    fn the_pattern_number_sets_the_fundamental() {
        let low = rising_crossings(&render(40.0, 1.0, 0.4)[4410..8820]);
        let high = rising_crossings(&render(120.0, 1.0, 0.4)[4410..8820]);
        assert!(high > low * 2, "120 Hz should bar faster: {high} vs {low}");
    }
}

#[cfg(test)]
mod zero_param_tests {
    use super::*;
    use super::build_voice_at_default_tempo as build_voice;
    use crate::parser::parser::parse;
    use fundsp::prelude64::AudioUnit;

    fn instruments(src: &str) -> Instruments {
        Instruments::from_program(&parse(src.to_string()).expect("parse failed"))
    }

    /// A fixed drum needs no placeholder parameter.
    #[test]
    fn a_zero_parameter_instrument_builds() {
        let ins = instruments("fn kick() = sin(50) * perc(0.002, 0.3)\n");
        assert_eq!(ins.param_count("kick"), Some(0));

        let mut net = build_voice(&ins, "kick", 1.0, &[], 1.0).expect("should build").net;
        assert_eq!(net.outputs(), 2);
        net.set_sample_rate(44100.0);

        let s: Vec<f32> = (0..44100).map(|_| net.get_mono()).collect();
        let onset = s[..2205].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(onset > 0.5, "should sound, peak {onset}");
    }

    /// The event value is ignored, so every trigger sounds identical.
    #[test]
    fn the_event_value_is_ignored() {
        let ins = instruments("fn kick() = sin(50) * perc(0.002, 0.3)\n");

        let render = |v: f64| {
            let mut net = build_voice(&ins, "kick", v, &[], 1.0).unwrap().net;
            net.set_sample_rate(44100.0);
            (0..4410).map(|_| net.get_mono()).collect::<Vec<f32>>()
        };
        assert_eq!(render(1.0), render(9999.0), "the value must not matter");
    }

    /// One-parameter instruments still receive the value.
    #[test]
    fn one_parameter_instruments_are_unaffected() {
        let ins = instruments("fn tone(f) = sin(f)\n");
        assert_eq!(ins.param_count("tone"), Some(1));
        assert!(build_voice(&ins, "tone", 220.0, &[], 1.0).is_ok());
    }

    /// Declaring a parameter and ignoring it still works, for compatibility.
    #[test]
    fn an_ignored_parameter_still_works() {
        let ins = instruments("fn kick(_) = sin(50) * perc(0.002, 0.3)\n");
        assert_eq!(ins.param_count("kick"), Some(1));
        assert!(build_voice(&ins, "kick", 1.0, &[], 1.0).is_ok());
    }
}
