use fundsp::prelude64::*;

use crate::audio_in::InputNode;
use crate::midi::input::{Control, ControlNode};
use crate::swync_graph::{
    graph::SwyncGraph,
    sample_reader::SampleReader,
    ugen_nodes::{NodeInput, NodeKind, UGenNode},
};

/// How often a time-based envelope samples its shape. `Envelope` linearly
/// interpolates between samples, and these shapes are piecewise linear, so
/// interpolation is exact except at the corners — 0.5 ms is inaudible while
/// keeping the closure off the per-sample path.
///
/// Note this is *not* `lfo()` / `envelope()`, which sample at 2 ms with
/// pseudorandom jitter — far too coarse for a short attack.
pub(crate) const ENV_INTERVAL: f64 = 0.0005;

/// Attack-decay-sustain shape, ignoring release. Returns 0..=1.
fn ads_level(t: f64, attack: f64, decay: f64, sustain: f64) -> f64 {
    let s = sustain.clamp(0.0, 1.0);
    if t < attack {
        // attack <= 0 is handled by the caller ordering: t < 0 is impossible,
        // so a zero attack falls straight through to the decay stage.
        t / attack
    } else if t < attack + decay {
        let x = (t - attack) / decay;
        1.0 + (s - 1.0) * x
    } else {
        s
    }
}

/// The input indices the two time-based envelopes take their times from.
const ENV_RELEASE_INPUT: usize = 3;
const PERC_ATTACK_INPUT: usize = 0;
const PERC_RELEASE_INPUT: usize = 1;
const LINE_END_INPUT: usize = 1;
const LINE_DURATION_INPUT: usize = 2;

/// The input indices the time-displacing nodes take their times from. A tap's
/// delay is a signal rather than a constant, so it declares bounds as well.
const DELAY_TIME_INPUT: usize = 1;
const TAP_DELAY_INPUT: usize = 1;
const TAP_MIN_INPUT: usize = 2;
const TAP_MAX_INPUT: usize = 3;
const REVERB_TIME_INPUT: usize = 2;
const REVERB3_TIME_INPUT: usize = 1;

/// The longest a voice may be held open past its note, however long its own
/// chain says it needs.
///
/// A tail is not free: every voice still ringing is a whole realized graph the
/// sequencer keeps rendering, so a tail of `t` seconds against a pattern of `n`
/// notes a second costs `t * n` live networks at once. Musical delays and
/// reverbs are seconds; `reverb(x, 10, 120, 0.5)` is a typo, and honouring it
/// would pin hundreds of voices open and miss the real-time deadline — which
/// costs the whole performance, not just the note that asked. Ten seconds
/// covers every tail anyone means and bounds what a slipped digit can do.
const MAX_TAIL_SECS: f64 = 10.0;

/// A construction-time constant that is a usable length in seconds.
///
/// `None` for anything else, which is a graph the realizer is about to refuse
/// or a time that would mean nothing anyway — either way not a reason to hold
/// a voice open.
fn const_secs(n: &UGenNode, idx: usize) -> Option<f64> {
    match n.inputs.get(idx) {
        Some(NodeInput::Const(v)) if v.is_finite() && *v >= 0.0 => Some(*v),
        _ => None,
    }
}

/// How far a tap can still be reaching back when its input goes quiet.
///
/// The delay is a signal, so where it actually sits is not knowable here — but
/// the bounds it declares are, and the realized `tap` clamps to them. A
/// constant delay is worth reading anyway, because it is the common case and
/// taking the declared maximum for it would hold every voice open for a reach
/// the graph can never make: `tap(x, 0.25, 0.1, 5)` is a quarter-second echo,
/// not a five-second one.
///
/// Bounds the wrong way round is a graph `tap` is about to refuse, so the
/// clamp is written to answer rather than to panic.
fn tap_tail(n: &UGenNode) -> f64 {
    let min = const_secs(n, TAP_MIN_INPUT).unwrap_or(0.0);
    let max = const_secs(n, TAP_MAX_INPUT).unwrap_or(0.0);
    match const_secs(n, TAP_DELAY_INPUT) {
        Some(t) => t.max(min).min(max),
        None => max,
    }
}

/// How long one node goes on sounding after everything feeding it has stopped.
///
/// This is the node's *own* contribution; what reaches it from upstream is
/// added by `tail_secs`. Two kinds of node have one. **Envelopes** shape the
/// note itself, and the two are anchored differently — `env` hangs its release
/// off the end of the note, so that release is the tail whatever the note was,
/// while `perc` measures its own shape from the onset and does not know the
/// note exists, so it asks for room only when it does not fit. **Anything that
/// displaces a signal in time** has one for a different reason: a delay hands
/// its input back later, and a reverb goes on answering long after it stops
/// being fed, so the sound arrives after the thing that made it is finished.
///
/// Everything else — oscillators, filters, shapers, arithmetic — answers
/// within a sample, and passes its input's tail through untouched.
///
/// Must be kept in step with the corresponding arms of `realize` below, which
/// is where these input indices are read for real.
fn own_tail(n: &UGenNode, dur_secs: f64) -> f64 {
    match n.kind {
        NodeKind::Env => const_secs(n, ENV_RELEASE_INPUT).unwrap_or(0.0),
        NodeKind::Perc => match (
            const_secs(n, PERC_ATTACK_INPUT),
            const_secs(n, PERC_RELEASE_INPUT),
        ) {
            (Some(a), Some(r)) => (a + r - dur_secs).max(0.0),
            _ => 0.0,
        },
        // A line is measured from the onset like `perc`, but it ends at a level
        // rather than at silence, and only a shape that finishes is worth
        // holding a voice open for. A line to zero does finish, and asks for
        // whatever of it did not fit inside the note; a line to anywhere else
        // is still sounding when it arrives, so room for it would not be a tail
        // but a longer note.
        NodeKind::Line => match (
            const_secs(n, LINE_DURATION_INPUT),
            n.inputs.get(LINE_END_INPUT),
        ) {
            (Some(d), Some(NodeInput::Const(end))) if *end == 0.0 => (d - dur_secs).max(0.0),
            _ => 0.0,
        },
        NodeKind::Delay => const_secs(n, DELAY_TIME_INPUT).unwrap_or(0.0),
        NodeKind::Tap => tap_tail(n),
        // Every reverb's `time` is its decay to -60 dB, which is where it stops
        // being worth holding a voice open for. `reverb3` has no room size, so
        // its time sits one input earlier.
        NodeKind::Reverb | NodeKind::Reverb2 | NodeKind::Reverb4 => {
            const_secs(n, REVERB_TIME_INPUT).unwrap_or(0.0)
        }
        NodeKind::Reverb3 => const_secs(n, REVERB3_TIME_INPUT).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// How long a graph goes on sounding past the end of its note.
///
/// A voice is only rendered for as long as its sequencer event lasts, so
/// anything still sounding when it ends is cut off mid-shape — a rest after a
/// long `env` release, a drum whose `perc` outlasts the step it sits on, an
/// echo that has not come back yet, a reverb tail that stops dead. This is what
/// the scheduler adds to the note to give all of it room.
///
/// **Tails accumulate along the signal path**, which is why this walks the
/// graph rather than taking the longest node: an `env` into a `delay` rings for
/// its release and *then* is handed back a delay later, so the voice needs the
/// sum of the two. Parallel branches take the longest — a dry signal added to
/// its own echoes lasts as long as the last echo, not as long as all of them.
/// One pass in id order suffices because a node's inputs are always nodes
/// already pushed, the same footing `realize` wires on.
///
/// Only what reaches the output counts, and zero is the usual answer — most
/// voices end with their note, which is why most stop exactly on their step.
pub fn tail_secs(graph: &SwyncGraph, dur_secs: f64) -> f64 {
    let Some(out) = graph.output else { return 0.0 };

    let mut tails: Vec<f64> = Vec::with_capacity(graph.nodes.len());
    for n in &graph.nodes {
        let upstream = n
            .inputs
            .iter()
            .filter_map(|i| match i {
                NodeInput::Node(id) => tails.get(id.0).copied(),
                NodeInput::Const(_) => None,
            })
            .fold(0.0, f64::max);
        tails.push(upstream + own_tail(n, dur_secs));
    }

    tails.get(out.0).copied().unwrap_or(0.0).min(MAX_TAIL_SECS)
}

/// Pull input `idx` as a construction-time constant. Parameters like ADSR
/// times are baked into the unit when it is built — they are not ports, so a
/// signal wired here has nothing to connect to.
/// The slot a MIDI reader was given, as the lowerer wrote it.
///
/// Not validated against what exists: a slot is handed out by
/// `midi::input::slot_for` without touching hardware, and `NO_SLOT` — which is
/// a program naming a ninth port — reads as silence rather than failing here.
/// What this refuses is only what could not have come from the lowerer at all.
fn slot(n: &UGenNode, at: usize) -> Result<usize, String> {
    let raw = const_param(n, at, "midi slot")?;
    if raw < 0.0 || raw.fract() != 0.0 {
        return Err(format!("midi: slot must be a whole number, got {raw}"));
    }
    Ok(raw as usize)
}

/// A MIDI channel, 1-16 as it is written.
fn channel(n: &UGenNode, at: usize) -> Result<u8, String> {
    let raw = const_param(n, at, "midi channel")?;
    if raw.fract() != 0.0 || !(1.0..=16.0).contains(&raw) {
        return Err(format!("midi: channel must be a whole number from 1 to 16, got {raw}"));
    }
    Ok(raw as u8)
}

/// A controller number, which is seven bits.
fn seven_bit(n: &UGenNode, at: usize, what: &str) -> Result<u8, String> {
    let raw = const_param(n, at, what)?;
    if raw.fract() != 0.0 || !(0.0..=127.0).contains(&raw) {
        return Err(format!("{what} must be a whole number from 0 to 127, got {raw}"));
    }
    Ok(raw as u8)
}

fn const_param(n: &UGenNode, idx: usize, name: &str) -> Result<f32, String> {
    match n.inputs.get(idx) {
        Some(NodeInput::Const(v)) => Ok(*v as f32),
        Some(NodeInput::Node(_)) => Err(format!("{name} must be a constant, not a signal")),
        None => Err(format!("missing {name}")),
    }
}

/// `const_param`, rejecting values a reverb cannot survive.
///
/// The reverbs derive their feedback gain from `time` and `room_size`; at zero
/// or below that gain reaches 1 or more, which is unbounded feedback straight
/// into the audio thread. Every other UGen's bad parameters merely sound wrong.
fn positive_param(n: &UGenNode, idx: usize, name: &str) -> Result<f32, String> {
    match const_param(n, idx, name)? {
        v if v > 0.0 => Ok(v),
        v => Err(format!("{name} must be above zero, got {v}")),
    }
}

/// Adapt a stereo reverb to the mono graph: feed the signal to both inputs and
/// average the two outputs back down.
fn mono(rev: An<impl AudioNode<Inputs = U2, Outputs = U2> + 'static>) -> Box<dyn AudioUnit> {
    Box::new(split::<U2>() >> rev >> join::<U2>())
}

/// Realize an IR graph into a runnable network.
///
/// The result is always 0-in / 2-out. Both consumers require it: the engine's
/// program slot is stereo (crossfade asserts matching arity) and the sequencer
/// is stereo (push asserts it). A mono result fans out to both channels.
/// When an envelope should release, for a voice whose note has no length yet.
///
/// A key that is down has no end until it comes up, and `env`'s release is
/// baked in at build time from its fifth argument — so a voice built for a
/// held key has a release scheduled at a time that never arrives, and the only
/// way to end it is to cut it. That is a click where a release was written.
///
/// This is that fifth argument made to move: seconds of the voice's own time,
/// `f32::INFINITY` while the key is down, and the moment of the release when
/// it comes up. One `AtomicU32` per voice, written once, read on the audio
/// callback like any other envelope parameter.
pub type Gate = std::sync::Arc<std::sync::atomic::AtomicU32>;

/// A gate that is never let go, which is what a voice with a known length has.
pub fn held() -> Gate {
    std::sync::Arc::new(std::sync::atomic::AtomicU32::new(f32::INFINITY.to_bits()))
}

/// Let a gated voice go, at `secs` into its own life.
pub fn release_at(gate: &Gate, secs: f64) {
    gate.store((secs.max(0.0) as f32).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// What a gate currently says, in seconds.
fn gate_secs(gate: &Gate) -> f64 {
    f32::from_bits(gate.load(std::sync::atomic::Ordering::Relaxed)) as f64
}

pub fn realize(graph: &SwyncGraph) -> Result<Net, String> {
    realize_gated(graph, None)
}

/// Realize a voice whose note ends when a key is let go.
///
/// `gate` is `Some` only for a live voice — a keyboard's, which is the one
/// case where the length is not known at build time. Everything else in the
/// engine goes through [`realize`] and is untouched by any of this.
///
/// **Only an envelope written to last the whole note is gated.** `held` is the
/// length such a voice was given, so an `env(.., dur)` matches it exactly and
/// gates on the key, while an `env(.., 0.1)` blip inside the same instrument
/// keeps the length it asked for. Overriding both would make every short shape
/// in an instrument sustain until the key came up, which is not what any of
/// them say.
pub fn realize_gated(
    graph: &SwyncGraph,
    gate: Option<(&Gate, f64)>,
) -> Result<Net, String> {
    let mut net = Net::new(0, 2);
    let mut ids: Vec<fundsp::net::NodeId> = Vec::with_capacity(graph.nodes.len());

    for n in &graph.nodes {
        let (unit, wired): (Box<dyn AudioUnit>, usize) = match n.kind {
            NodeKind::Add => (Box::new(pass() + pass()), 2),
            NodeKind::ADSR => (
                Box::new(adsr_live(
                    const_param(n, 1, "adsr attack")?,
                    const_param(n, 2, "adsr decay")?,
                    const_param(n, 3, "adsr sustain")?,
                    const_param(n, 4, "adsr release")?,
                )),
                1,
            ),
            NodeKind::Afollow => (
                Box::new(afollow(
                    const_param(n, 1, "afollow attack")?,
                    const_param(n, 2, "afollow release")?,
                )),
                1,
            ),
            NodeKind::Allpass => (Box::new(allpass()), 3),
            NodeKind::Allpole => (Box::new(allpole()), 2),
            NodeKind::Bandpass => (Box::new(bandpass()), 3),
            NodeKind::Bandrez => (Box::new(bandrez()), 3),
            NodeKind::Bell => (Box::new(bell()), 4),
            NodeKind::Biquad => (
                Box::new(biquad(
                    const_param(n, 1, "biquad a1")?,
                    const_param(n, 2, "biquad a2")?,
                    const_param(n, 3, "biquad b0")?,
                    const_param(n, 4, "biquad b1")?,
                    const_param(n, 5, "biquad b2")?,
                )),
                1,
            ),
            NodeKind::Brown => (Box::new(brown()), 0),
            NodeKind::Butterpass => (Box::new(butterpass()), 2),
            NodeKind::Chorus => (
                Box::new(chorus(
                    const_param(n, 1, "chorus seed")? as u64,
                    const_param(n, 2, "chorus separation")?,
                    const_param(n, 3, "chorus variation")?,
                    const_param(n, 4, "chorus mod frequency")?,
                )),
                1,
            ),
            NodeKind::Clip => (Box::new(clip()), 1),
            NodeKind::ClipTo => (
                Box::new(clip_to(
                    const_param(n, 1, "clip_to minimum")?,
                    const_param(n, 2, "clip_to maximum")?,
                )),
                1,
            ),
            NodeKind::Dcblock => (Box::new(dcblock()), 1),
            NodeKind::Declick => (Box::new(declick()), 1),
            NodeKind::Delay => (Box::new(delay(const_param(n, 1, "delay time")?)), 1),
            NodeKind::Div => (Box::new(map(|i: &Frame<f32, U2>| i[0] / i[1])), 2),
            NodeKind::DsfSaw => (Box::new(dsf_saw()), 2),

            // Time-based ADSR for one-shot voices. The note holds the gate open
            // for `duration`; the release begins where the note ends and rings
            // on for its own time past it, so the whole shape lasts
            // `duration + release` and the voice is scheduled for that long
            // (see `tail_secs`). Computed as ads * release_mult so degenerate
            // cases (a note shorter than its own attack, zero-length segments)
            // fall out without special-casing — freezing the level at the gate
            // means such a note releases from wherever it got to rather than
            // jumping to a level it never reached.
            NodeKind::Env => {
                let attack = const_param(n, 0, "env attack")? as f64;
                let decay = const_param(n, 1, "env decay")? as f64;
                let sustain = const_param(n, 2, "env sustain")? as f64;
                let release = const_param(n, 3, "env release")? as f64;
                let written = (const_param(n, 4, "env duration")? as f64).max(0.0);
                // Written to last the whole note, on a voice whose note is a
                // key being held: this is the envelope the key lets go of.
                let keyed = match gate {
                    Some((cell, held)) if written >= held => Some(cell.clone()),
                    _ => None,
                };
                (
                    Box::new(An(Envelope::new(ENV_INTERVAL, move |t: f64| -> f64 {
                        // Read per call rather than captured, which is the
                        // whole point: it changes when the key comes up.
                        let gate = match &keyed {
                            Some(cell) => gate_secs(cell),
                            None => written,
                        };
                        let level = ads_level(t.min(gate), attack, decay, sustain);
                        let mult = if t < gate {
                            1.0
                        } else if release <= 0.0 {
                            0.0
                        } else {
                            (1.0 - (t - gate) / release).clamp(0.0, 1.0)
                        };
                        level * mult
                    }))),
                    0,
                )
            }
            NodeKind::DsfSquare => (Box::new(dsf_square()), 2),
            NodeKind::Fir3 => (Box::new(fir3(const_param(n, 1, "fir3 gain")?)), 1),
            NodeKind::Follow => (Box::new(follow(const_param(n, 1, "follow response time")?)), 1),
            NodeKind::Hammond => (Box::new(hammond()), 1),
            NodeKind::Highpass => (Box::new(highpass()), 3),
            NodeKind::Highpole => (Box::new(highpole()), 2),
            NodeKind::Highshelf => (Box::new(highshelf()), 4),
            NodeKind::Hold => (Box::new(hold(const_param(n, 2, "hold variability")?)), 2),
            NodeKind::Impulse => (Box::new(impulse::<U1>()), 0),

            // The live input. Which channels actually exist is a fact about
            // what is plugged in this evening, so it is deliberately *not*
            // checked here: a program written against a two-in interface must
            // still compile on the laptop it is edited on, and reads silence
            // there. What is checked is what a program can get wrong on its
            // own — a channel that is negative, fractional, or past anything
            // the bus could ever carry.
            // The three MIDI-in readers. Their device has already been
            // resolved to a slot by the lowerer — see `lowerer/midi.rs` — so
            // what arrives here is numbers, and every one of them was written
            // at the call rather than computed.
            NodeKind::Cc => (
                Box::new(An(ControlNode::new(
                    slot(n, 0)?,
                    channel(n, 2)?,
                    Control::Controller(seven_bit(n, 1, "cc number")?),
                    const_param(n, 3, "cc low")? as f32,
                    const_param(n, 4, "cc high")? as f32,
                ))),
                0,
            ),
            NodeKind::Bend => (
                Box::new(An(ControlNode::new(
                    slot(n, 0)?,
                    channel(n, 1)?,
                    Control::Bend,
                    const_param(n, 2, "bend low")? as f32,
                    const_param(n, 3, "bend high")? as f32,
                ))),
                0,
            ),
            NodeKind::Aftertouch => (
                Box::new(An(ControlNode::new(
                    slot(n, 0)?,
                    channel(n, 1)?,
                    Control::Pressure,
                    const_param(n, 2, "aftertouch low")? as f32,
                    const_param(n, 3, "aftertouch high")? as f32,
                ))),
                0,
            ),
            NodeKind::Input => {
                let channel = const_param(n, 0, "input channel")?;
                if channel < 0.0 || channel.fract() != 0.0 {
                    return Err(format!(
                        "input: channel must be a whole number from 0, got {channel}"
                    ));
                }
                if channel as usize >= crate::audio_in::MAX_CHANNELS {
                    return Err(format!(
                        "input: channel {channel} is past the {} this reads",
                        crate::audio_in::MAX_CHANNELS
                    ));
                }
                (Box::new(An(InputNode::new(channel as usize))), 0)
            }
            NodeKind::Limiter => (
                Box::new(limiter(
                    const_param(n, 1, "limiter attack")?,
                    const_param(n, 2, "limiter release")?,
                )),
                1,
            ),

            // One straight segment from the onset, holding at `end` once it
            // arrives. A zero duration is not a degenerate case to guard but
            // the shape's own limit — the segment takes no time, so the answer
            // is `end` from the first sample, which is what this already says.
            NodeKind::Line => {
                let start = const_param(n, 0, "line start")? as f64;
                let end = const_param(n, 1, "line end")? as f64;
                let duration = (const_param(n, 2, "line duration")? as f64).max(0.0);
                (
                    Box::new(An(Envelope::new(ENV_INTERVAL, move |t: f64| -> f64 {
                        if t >= duration {
                            end
                        } else {
                            start + (end - start) * (t / duration)
                        }
                    }))),
                    0,
                )
            }
            NodeKind::Lorenz => (Box::new(lorenz()), 1),
            NodeKind::Lowpass => (Box::new(lowpass()), 3),
            NodeKind::Lowpole => (Box::new(lowpole()), 2),
            NodeKind::Lowrez => (Box::new(lowrez()), 3),
            NodeKind::Lowshelf => (Box::new(lowshelf()), 4),
            NodeKind::Mls => (Box::new(mls()), 0),
            NodeKind::MlsBits => (Box::new(mls_bits(const_param(n, 0, "mls_bits bits")? as u64)), 0),
            NodeKind::Moog => (Box::new(moog()), 3),
            NodeKind::Morph => (Box::new(morph()), 4),
            NodeKind::Mul => (Box::new(pass() * pass()), 2),
            NodeKind::Neg => (Box::new(-pass()), 1),
            NodeKind::Noise => (Box::new(noise()), 0),
            NodeKind::Notch => (Box::new(notch()), 3),
            NodeKind::Organ => (Box::new(organ()), 1),
            NodeKind::Peak => (Box::new(peak()), 3),

            // Self-contained percussive shape: rise, fall, silence. Needs no
            // note duration, so it works in a voice or the persistent graph.
            NodeKind::Perc => {
                let attack = const_param(n, 0, "perc attack")? as f64;
                let release = const_param(n, 1, "perc release")? as f64;
                (
                    Box::new(An(Envelope::new(ENV_INTERVAL, move |t: f64| -> f64 {
                        if t < attack {
                            t / attack
                        } else if release <= 0.0 {
                            0.0
                        } else {
                            (1.0 - (t - attack) / release).clamp(0.0, 1.0)
                        }
                    }))),
                    0,
                )
            }
            NodeKind::Pink => (Box::new(pink()), 0),
            NodeKind::Pinkpass => (Box::new(pinkpass()), 1),
            NodeKind::Pluck => (
                Box::new(pluck(
                    const_param(n, 1, "pluck frequency")?,
                    const_param(n, 2, "pluck gain per second")?,
                    const_param(n, 3, "pluck damping")?,
                )),
                1,
            ),
            NodeKind::PolyPulse => (Box::new(poly_pulse()), 2),
            NodeKind::PolySaw => (Box::new(poly_saw()), 1),
            NodeKind::PolySquare => (Box::new(poly_square()), 1),
            NodeKind::Pulse => (Box::new(pulse()), 2),
            // Explicitly from zero. fundsp's `ramp()` seeds its phase from the
            // node's hash, so a bare one starts partway up and lands somewhere
            // different every time the graph is rebuilt — which is every note,
            // for a voice. A phasor whose zero is not the beginning cannot
            // drive `sample`: the chop would start at an arbitrary point in the
            // buffer, differently each time. The name says "rising ramp from
            // 0", and this is what makes that true.
            NodeKind::Ramp => (Box::new(An(Ramp::<f64>::with_phase(0.0))), 1),
            NodeKind::Resonator => (Box::new(resonator()), 3),
            NodeKind::Reverb => (
                mono(reverb_stereo(
                    positive_param(n, 1, "reverb room size")?,
                    positive_param(n, 2, "reverb time")?,
                    const_param(n, 3, "reverb damping")?,
                )),
                1,
            ),
            NodeKind::Reverb2 => (
                mono(reverb2_stereo(
                    positive_param(n, 1, "reverb2 room size")?,
                    positive_param(n, 2, "reverb2 time")?,
                    const_param(n, 3, "reverb2 diffusion")?,
                    const_param(n, 4, "reverb2 modulation")?,
                    lowpole_hz(positive_param(n, 5, "reverb2 damping cutoff")?),
                )),
                1,
            ),
            NodeKind::Reverb3 => (
                mono(reverb3_stereo(
                    positive_param(n, 1, "reverb3 time")?,
                    const_param(n, 2, "reverb3 diffusion")?,
                    lowpole_hz(positive_param(n, 3, "reverb3 damping cutoff")?),
                )),
                1,
            ),
            NodeKind::Reverb4 => (
                mono(reverb4_stereo(
                    positive_param(n, 1, "reverb4 room size")?,
                    positive_param(n, 2, "reverb4 time")?,
                )),
                1,
            ),
            NodeKind::Rossler => (Box::new(rossler()), 1),
            NodeKind::Sample => {
                // The buffer is named by index into the graph's own table —
                // the one construction-time parameter that is not a number the
                // program wrote, so it is checked here rather than trusted.
                let index = const_param(n, 1, "sample buffer")? as usize;
                let Some(wave) = graph.samples.get(index) else {
                    return Err(format!(
                        "sample: buffer {index} is not in this graph (it has {})",
                        graph.samples.len()
                    ));
                };
                let channel = const_param(n, 2, "sample channel")?;
                if channel < 0.0 || channel.fract() != 0.0 {
                    return Err(format!(
                        "sample: channel must be a whole number from 0, got {channel}"));
                }
                (
                    Box::new(An(SampleReader::new(wave.clone(), channel as usize))),
                    1,
                )
            }
            NodeKind::Saw => (Box::new(saw()), 1),
            NodeKind::Sin => (Box::new(sine()), 1),
            NodeKind::SoftSaw => (Box::new(soft_saw()), 1),
            NodeKind::Square => (Box::new(square()), 1),
            NodeKind::Sub => (Box::new(pass() - pass()), 2),
            NodeKind::Tap => (
                Box::new(tap(
                    const_param(n, 2, "tap min delay")?,
                    const_param(n, 3, "tap max delay")?,
                )),
                2,
            ),
            NodeKind::Tick => (Box::new(tick()), 1),
            NodeKind::Triangle => (Box::new(triangle()), 1),
        };

        let fid = net.push(unit);

        for (port, input) in n.inputs.iter().take(wired).enumerate() {
            match input {
                NodeInput::Node(id) => net.connect(ids[id.0], 0, fid, port),
                NodeInput::Const(v) => {
                    let c = net.push(Box::new(dc(*v as f32)));
                    net.connect(c, 0, fid, port);
                }
            }
        }

        ids.push(fid);
    }

    // A graph with no output is valid — it just makes silence.
    let out_node = match graph.output {
        Some(out) => ids[out.0],
        None => net.push(Box::new(dc(0.0))),
    };
    net.connect_output(out_node, 0, 0);
    net.connect_output(out_node, 0, 1);

    Ok(net)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swync_graph::ugen_nodes::NodeId;
    use crate::lowerer::lower::lower;
    use crate::parser::parser::parse;

    /// Full pipeline: parse → lower → realize, then render actual samples.
    #[test]
    fn added_signals_produce_audio() {
        let items = parse("sin(220) + sin(330)\n".to_string()).unwrap();
        let graph = lower(&items).unwrap().graph;
        let mut net = realize(&graph).unwrap();

        net.check(); // validates every port is wired
        net.set_sample_rate(44100.0);

        let samples: Vec<f32> = (0..4410).map(|_| net.get_mono()).collect();
        assert!(samples.iter().all(|s| s.is_finite()));
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.5, "expected audible signal, peak was {peak}");
        assert!(peak <= 2.0, "two unit sines can't exceed 2.0, got {peak}");
    }

    /// `input` realizes on a machine with nothing plugged in, and is silent
    /// there. That is the whole of what a program can rely on: a piece written
    /// against an interface has to open, compile and run on the laptop it is
    /// edited on, where the channels it names do not exist.
    #[test]
    fn the_live_input_realizes_and_is_silent_with_no_device_open() {
        let _bus = crate::audio_in::exclusive();
        for source in ["input(0)\n", "input(3) * 0.5\n", "input(0) + input(1)\n"] {
            let items = parse(source.to_string()).unwrap();
            let graph = lower(&items).unwrap().graph;
            let mut net = realize(&graph).unwrap_or_else(|e| panic!("{source} refused: {e}"));

            net.check();
            net.set_sample_rate(44100.0);

            let samples: Vec<f32> = (0..1000).map(|_| net.get_mono()).collect();
            assert!(samples.iter().all(|s| *s == 0.0), "{source} was not silent");
        }
    }

    /// The claim the whole feature rests on: what arrives at the device comes
    /// out of a program that named it.
    ///
    /// Everything either side of this is tested apart — the ring in
    /// `audio_in::tests`, the node's own reading there too — and all of it
    /// could pass with the graph still silent, because what joins them is a
    /// `OnceLock` the realizer reaches for and a block the callback fills. So
    /// this one goes the whole way: frames into the bus the input thread
    /// writes to, a program lowered and realized as an eval would, and the
    /// samples out the other end.
    #[test]
    fn a_realized_graph_carries_what_arrived_at_the_device() {
        let _lock = crate::audio_in::exclusive();
        let bus = crate::audio_in::bus();

        // Two channels of a constant each, so a sample says which channel it
        // came from — and `input(1)` naming the second is half the point.
        bus.opened(2);
        let frame: Vec<f32> = (0..MAX_BUFFER_SIZE).flat_map(|_| [0.25f32, 0.5]).collect();
        for _ in 0..8 {
            bus.push(&frame, 2);
        }

        let items = parse("input(0) + input(1)\n".to_string()).unwrap();
        let lowered = lower(&items).unwrap();
        let mut net = realize(&lowered.graph).unwrap();
        net.set_sample_rate(48000.0);

        // Rendered the way the engine renders: the ring is drained once per
        // block, before the graph runs.
        let mut out = BufferVec::new(2);
        let mut heard = 0.0f32;
        for _ in 0..4 {
            bus.take(MAX_BUFFER_SIZE);
            net.process(MAX_BUFFER_SIZE, &BufferRef::empty(), &mut out.buffer_mut());
            heard = f32::max(heard, out.buffer_ref().at_f32(0, MAX_BUFFER_SIZE - 1).abs());
        }

        bus.closed();
        assert!(
            (heard - 0.75).abs() < 1e-4,
            "the two channels should sum to 0.75, heard {heard}"
        );
    }

    /// Every way of writing `input` that the README offers, compiled. It is a
    /// signal like any other and that is the whole claim, so what would break
    /// it is something subtle about how it chains — and the manual is where
    /// anybody would find out, which makes it worth pinning from here.
    #[test]
    fn the_documented_ways_of_naming_the_input_all_compile() {
        for source in [
            "input(0)\n",
            "input(0).lowpass(800, 1)\n",
            "input(0) + input(0).reverb(10, 3, 0.5) * 0.3\n",
            "fn stab() {\n    input(0) * perc(0.001, 0.15)\n}\n\nplay([\\, \\, `, \\], stab)\n",
        ] {
            let items = parse(source.to_string())
                .unwrap_or_else(|e| panic!("{source} did not parse: {e:?}"));
            let lowered = lower(&items).unwrap_or_else(|e| panic!("{source} did not lower: {e}"));
            realize(&lowered.graph).unwrap_or_else(|e| panic!("{source} did not realize: {e}"));
        }
    }

    /// What a program *can* get wrong on its own, as against what is merely
    /// not plugged in tonight. A channel that is negative or fractional is a
    /// typo in the program and says so; one past what the bus carries is a
    /// number that could never mean anything on any machine.
    #[test]
    fn a_channel_no_input_could_ever_have_is_refused() {
        for (source, expected) in [
            ("input(-1)\n", "whole number"),
            ("input(0.5)\n", "whole number"),
            ("input(64)\n", "past the"),
        ] {
            let items = parse(source.to_string()).unwrap();
            let graph = lower(&items).unwrap().graph;
            // `Net` is not `Debug`, so the refusal is taken out by hand rather
            // than with `expect_err`.
            let Err(error) = realize(&graph) else {
                panic!("{source} should have been refused");
            };
            assert!(
                error.contains(expected) && error.contains("input"),
                "{source} was refused with {error:?}"
            );
        }
    }

    /// A `for` loop unrolls to an ordinary graph, so it realizes and renders
    /// like any hand-written sum. Four harmonics of 110 Hz.
    #[test]
    fn for_loop_produces_audio() {
        let items = parse("for i in 1..=4 { sin(i * 110) / 4 }\n".to_string()).unwrap();
        let graph = lower(&items).unwrap().graph;
        let mut net = realize(&graph).unwrap();

        net.check();
        net.set_sample_rate(44100.0);

        let samples: Vec<f32> = (0..4410).map(|_| net.get_mono()).collect();
        assert!(samples.iter().all(|s| s.is_finite()));
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.3, "expected audible signal, peak was {peak}");
        assert!(peak <= 1.0, "four quarter-amplitude sines can't exceed 1.0, got {peak}");
    }

    /// A 2 Hz sine gate retriggers the envelope; output must stay in 0..=1
    /// and actually move.
    #[test]
    fn adsr_gated_by_slow_sine() {
        let graph = SwyncGraph {
            nodes: vec![
                UGenNode {
                    kind: NodeKind::Sin,
                    inputs: vec![NodeInput::Const(2.0)],
                    span: None,
                },
                UGenNode {
                    kind: NodeKind::ADSR,
                    inputs: vec![
                        NodeInput::Node(NodeId(0)), // gate
                        NodeInput::Const(0.01),     // attack
                        NodeInput::Const(0.05),     // decay
                        NodeInput::Const(0.5),      // sustain
                        NodeInput::Const(0.1),      // release
                    ],
                    span: None,
                },
            ],
            output: Some(NodeId(1)),
            ..Default::default()
        };

        let mut net = realize(&graph).unwrap();
        net.check();
        net.set_sample_rate(44100.0);

        let samples: Vec<f32> = (0..44100).map(|_| net.get_mono()).collect();
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(*s));
        let floor = samples.iter().fold(1.0f32, |m, s| m.min(*s));
        assert!(peak > 0.9, "attack should approach 1.0, peak was {peak}");
        assert!(floor >= 0.0, "envelope must not go negative, floor was {floor}");
        assert!((floor - peak).abs() > 0.5, "envelope should move over a full gate cycle");
    }

    /// A signal in a parameter slot is a user error, not a wiring job.
    #[test]
    fn adsr_signal_parameter_is_an_error() {
        let graph = SwyncGraph {
            nodes: vec![
                UGenNode {
                    kind: NodeKind::Sin,
                    inputs: vec![NodeInput::Const(2.0)],
                    span: None,
                },
                UGenNode {
                    kind: NodeKind::ADSR,
                    inputs: vec![
                        NodeInput::Node(NodeId(0)),
                        NodeInput::Node(NodeId(0)), // attack as a signal: invalid
                        NodeInput::Const(0.05),
                        NodeInput::Const(0.5),
                        NodeInput::Const(0.1),
                    ],
                    span: None,
                },
            ],
            output: Some(NodeId(1)),
            ..Default::default()
        };

        let err = match realize(&graph) {
            Err(e) => e,
            Ok(_) => panic!("expected an error for a signal-valued adsr parameter"),
        };
        assert!(err.contains("constant"), "got: {err}");
    }
}


#[cfg(test)]
mod envelope_tests {
    use super::*;
    use crate::swync_graph::ugen_nodes::NodeId;
    use crate::lowerer::lower::{lower, lower_voice};
    use crate::parser::parser::parse;

    const SR: f64 = 44100.0;

    /// Render `src` as a voice with note length `dur` and return its samples.
    fn render_voice(src: &str, dur: f64, secs: f64) -> Vec<f32> {
        let items = parse(src.to_string()).expect("parse failed");
        let lowered = lower_voice(&items, dur, crate::lowerer::lower::DEFAULT_BEAT_SECS, Default::default(), Default::default()).expect("lower failed");
        let mut net = realize(&lowered.graph).expect("realize failed");
        net.check();
        net.set_sample_rate(SR);
        (0..(secs * SR) as usize).map(|_| net.get_mono()).collect()
    }

    fn at(samples: &[f32], t: f64) -> f32 {
        samples[(t * SR) as usize]
    }

    /// perc rises to 1 by the end of its attack and is silent after a + r.
    #[test]
    fn perc_rises_then_falls_to_silence() {
        let s = render_voice("perc(0.05, 0.2)\n", 1.0, 0.5);

        assert!(at(&s, 0.0) < 0.05, "should start near zero, got {}", at(&s, 0.0));
        assert!((at(&s, 0.05) - 1.0).abs() < 0.02, "peak at attack end: {}", at(&s, 0.05));
        assert!((at(&s, 0.15) - 0.5).abs() < 0.03, "midway through release: {}", at(&s, 0.15));
        assert!(at(&s, 0.3) < 0.01, "silent after a + r, got {}", at(&s, 0.3));
        assert!(s.iter().all(|v| *v >= -0.001 && *v <= 1.001), "stayed in 0..=1");
    }

    /// perc needs no note duration, so it works in the persistent graph too.
    #[test]
    fn perc_works_outside_a_voice() {
        let items = parse("sin(220) * perc(0.01, 0.1)\n".to_string()).unwrap();
        let g = lower(&items).unwrap().graph;
        assert!(realize(&g).is_ok());
    }

    /// env holds its sustain level for the whole note, and only then releases
    /// — the release is time *after* `dur`, not time taken out of it.
    #[test]
    fn env_sustains_for_the_note_then_releases_after_it() {
        // attack 0.1, decay 0.1, sustain 0.5, release 0.2, over a 1s note.
        let s = render_voice("env(0.1, 0.1, 0.5, 0.2, dur)\n", 1.0, 1.4);

        assert!((at(&s, 0.1) - 1.0).abs() < 0.02, "peak at attack end: {}", at(&s, 0.1));
        assert!((at(&s, 0.2) - 0.5).abs() < 0.02, "sustain after decay: {}", at(&s, 0.2));
        assert!((at(&s, 0.5) - 0.5).abs() < 0.02, "still sustaining: {}", at(&s, 0.5));
        // The bug this guards: 0.9 used to be halfway down the release.
        assert!((at(&s, 0.9) - 0.5).abs() < 0.02, "sustaining to the end: {}", at(&s, 0.9));
        assert!((at(&s, 0.999) - 0.5).abs() < 0.02, "still up at the note end: {}", at(&s, 0.999));
        assert!((at(&s, 1.1) - 0.25).abs() < 0.03, "mid-release: {}", at(&s, 1.1));
        assert!(at(&s, 1.2) < 0.02, "silent a release after the note: {}", at(&s, 1.2));
    }

    /// The note length really comes from `dur`, not a baked constant — and the
    /// release that follows is the same length whatever the note was.
    #[test]
    fn env_tracks_the_note_length() {
        let short = render_voice("env(0.01, 0.01, 0.8, 0.1, dur)\n", 0.3, 0.6);
        let long = render_voice("env(0.01, 0.01, 0.8, 0.1, dur)\n", 0.8, 1.1);

        assert!(at(&short, 0.299) > 0.5, "short note still held at its end");
        assert!(at(&short, 0.399) < 0.02, "short note done a release later");
        assert!(at(&long, 0.3) > 0.5, "long note still sustaining at 0.3s");
        assert!(at(&long, 0.799) > 0.5, "long note still held at its end");
        assert!(at(&long, 0.899) < 0.02, "long note done a release later");
    }

    /// A note shorter than its own attack releases from the level it reached,
    /// rather than jumping to one it never got to.
    #[test]
    fn a_note_cut_short_releases_from_where_it_got_to() {
        // Attack 0.4 over a 0.2s note: the gate closes halfway up.
        let s = render_voice("env(0.4, 0.1, 0.5, 0.2, dur)\n", 0.2, 0.5);

        assert!((at(&s, 0.199) - 0.5).abs() < 0.02, "halfway up at the gate: {}", at(&s, 0.199));
        assert!((at(&s, 0.3) - 0.25).abs() < 0.03, "halfway down again: {}", at(&s, 0.3));
        assert!(at(&s, 0.41) < 0.02, "silent a release later, got {}", at(&s, 0.41));
    }

    /// A line is straight between its ends and flat after them.
    #[test]
    fn line_ramps_from_start_to_end_then_holds() {
        let s = render_voice("line(0, 1, 0.2)\n", 1.0, 0.5);

        assert!(at(&s, 0.0).abs() < 0.01, "starts at `start`: {}", at(&s, 0.0));
        assert!((at(&s, 0.05) - 0.25).abs() < 0.01, "a quarter along: {}", at(&s, 0.05));
        assert!((at(&s, 0.1) - 0.5).abs() < 0.01, "halfway along: {}", at(&s, 0.1));
        assert!((at(&s, 0.2) - 1.0).abs() < 0.01, "at `end` on time: {}", at(&s, 0.2));
        assert!((at(&s, 0.45) - 1.0).abs() < 0.01, "held after: {}", at(&s, 0.45));
    }

    /// Nothing about it is bound to 0..=1: it falls as readily as it rises, and
    /// the range is whatever the two ends say. This is the pitch-drop case.
    #[test]
    fn line_runs_in_either_direction_over_any_range() {
        let s = render_voice("line(880, 220, 0.05)\n", 1.0, 0.2);

        assert!((at(&s, 0.0) - 880.0).abs() < 1.0, "starts high: {}", at(&s, 0.0));
        assert!((at(&s, 0.025) - 550.0).abs() < 5.0, "halfway down: {}", at(&s, 0.025));
        assert!((at(&s, 0.15) - 220.0).abs() < 1.0, "settled low: {}", at(&s, 0.15));
    }

    /// It is measured from the onset and needs no note length, so — like `perc`
    /// and unlike `env` — it works in the persistent graph too.
    #[test]
    fn line_works_outside_a_voice() {
        let items = parse("sin(line(220, 440, 2))\n".to_string()).unwrap();
        let g = lower(&items).unwrap().graph;
        assert!(realize(&g).is_ok());
    }

    /// A segment of no length is the whole shape arriving at once, not a
    /// division by zero.
    #[test]
    fn a_line_of_no_duration_is_its_end_from_the_start() {
        let s = render_voice("line(0, 0.5, 0)\n", 0.5, 0.2);
        assert!(s.iter().all(|v| (v - 0.5).abs() < 0.001), "expected a flat 0.5");
    }

    /// What the scheduler asks a voice for: how much longer than the note it
    /// needs to finish what it started.
    fn tail_of(src: &str, dur: f64) -> f64 {
        let items = parse(src.to_string()).expect("parse failed");
        let lowered = lower_voice(&items, dur, crate::lowerer::lower::DEFAULT_BEAT_SECS, Default::default(), Default::default()).expect("lower failed");
        tail_secs(&lowered.graph, dur)
    }

    fn assert_tail(src: &str, dur: f64, want: f64) {
        let got = tail_of(src, dur);
        assert!((got - want).abs() < 1e-6, "{src}: wanted a tail of {want}, got {got}");
    }

    #[test]
    fn the_tail_is_how_far_an_envelope_outlasts_the_note() {

        // Nothing time-shaped: the voice ends with its note.
        assert_eq!(tail_of("sin(220)\n", 1.0), 0.0);

        // `env` releases from the end of the note, so it always asks for the
        // release however long or short the note was.
        assert!((tail_of("sin(220) * env(0.01, 0.1, 0.5, 0.3, dur)\n", 1.0) - 0.3).abs() < 1e-6);
        assert!((tail_of("sin(220) * env(0.01, 0.1, 0.5, 0.3, dur)\n", 0.05) - 0.3).abs() < 1e-6);

        // `perc` is measured from the onset, so it only asks for room when the
        // shape is longer than the note it landed on.
        assert_eq!(tail_of("sin(220) * perc(0.01, 0.4)\n", 1.0), 0.0, "the note covers it");
        let cramped = tail_of("sin(220) * perc(0.01, 0.4)\n", 0.125);
        assert!((cramped - 0.285).abs() < 1e-6, "0.41 of shape on a 0.125 step: {cramped}");

        // Several in one voice: they all have to finish, so the latest wins.
        let two = "sin(220) * perc(0.01, 0.4) + sin(330) * env(0.01, 0.1, 0.5, 0.05, dur)\n";
        assert!((tail_of(two, 0.125) - 0.285).abs() < 1e-6, "got {}", tail_of(two, 0.125));
        assert!((tail_of(two, 1.0) - 0.05).abs() < 1e-6, "got {}", tail_of(two, 1.0));
    }

    /// A tail is room to finish, and only a line that ends at silence has
    /// anything to finish. One that ends anywhere else is still sounding when
    /// it gets there, so giving it room would hold the note on rather than let
    /// it end — which is the note's business, not the shape's.
    #[test]
    fn a_line_asks_for_room_only_when_it_ends_in_silence() {
        assert_tail("sin(220) * line(1, 0, 0.4)\n", 1.0, 0.0);
        assert_tail("sin(220) * line(1, 0, 0.4)\n", 0.125, 0.275);

        // Ends held at a level: nothing to wait for, however long it is. Where
        // the line was wired makes no difference — a sweep four seconds long
        // asks for nothing either way.
        assert_tail("sin(220) * line(0, 1, 4)\n", 0.125, 0.0);
        assert_tail("sin(line(880, 220, 4))\n", 0.125, 0.0);
    }

    /// The reported bug: a voice whose echoes arrive after its envelope has
    /// finished was cut off at the envelope, so the delay was inaudible from
    /// the second repeat on. What displaces a signal in time has a tail like
    /// what shapes it, and the two add up along the path — the envelope rings
    /// out, and only then is that whole sound handed back a delay later.
    #[test]
    fn a_delay_holds_the_voice_open_until_its_echo_has_come_back() {
        let env = "sin(220) * env(0.01, 0.1, 0.5, 0.3, dur)";

        assert_tail(&format!("{env} >> delay(0.5)\n"), 1.0, 0.8);
        assert_tail(&format!("{env} >> delay(0.5) >> delay(0.25)\n"), 1.0, 1.05,);

        // A dry signal beside its own echo lasts as long as the echo — the two
        // branches happen at once, so the longest of them is the answer and
        // not the sum.
        assert_tail(&format!("let a = {env}\na + (a >> delay(0.5))\n"), 1.0, 0.8);
    }

    /// A reverb goes on answering long after it stops being fed, and `time` is
    /// its own statement of how long: the decay to -60 dB.
    #[test]
    fn a_reverb_holds_the_voice_open_for_its_decay() {
        let dry = "sin(220) * perc(0.01, 0.1)";

        assert_tail(&format!("{dry} + reverb({dry}, 10, 3, 0.5) * 0.2\n"), 1.0, 3.0);
        // `reverb3` has no room size, so its time sits one input earlier —
        // reading the wrong one here would silently give a voice no tail.
        assert_tail(&format!("reverb3({dry}, 2.5, 0.5, 4000)\n"), 1.0, 2.5);
    }

    /// A tap's delay is a signal, so the graph cannot always say where it
    /// sits. The declared bounds always can, and a constant delay — which is
    /// most of them — is worth reading rather than assuming the worst.
    #[test]
    fn a_tap_asks_for_where_it_reaches_and_falls_back_on_its_bounds() {
        let dry = "sin(220) * perc(0.01, 0.1)";

        assert_tail(&format!("tap({dry}, 0.25, 0.1, 5)\n"), 1.0, 0.25);
        // Below the declared minimum is where the realized tap clamps it to.
        assert_tail(&format!("tap({dry}, 0, 0.1, 5)\n"), 1.0, 0.1);
        // Modulated: nothing constant to read, so the bound it declared stands.
        assert_tail(&format!("tap({dry}, sin(0.5) * 0.5 + 1, 0.1, 3)\n"), 1.0, 3.0);
    }

    /// A tail costs live voices, so a number nobody meant must not be able to
    /// pin the sequencer open. The cap is a ceiling, not a rounding — anything
    /// musical passes through it untouched.
    #[test]
    fn an_implausible_tail_is_capped() {
        let dry = "sin(220) * perc(0.01, 0.1)";

        assert_tail(&format!("reverb({dry}, 10, 120, 0.5)\n"), 1.0, MAX_TAIL_SECS);
        assert_tail(&format!("{dry} >> delay(4)\n"), 1.0, 4.0);
    }

    /// Only what reaches the output is still to be heard, so a binding the
    /// program never used cannot hold a voice open.
    #[test]
    fn a_tail_on_a_signal_nothing_listens_to_does_not_count() {
        let src = "let unused = sin(220) >> delay(2)\nsin(330) * perc(0.01, 0.1)\n";
        assert_tail(src, 1.0, 0.0);
    }

    /// Degenerate shapes must not panic, go negative, or exceed 1.
    #[test]
    fn env_degenerate_shapes_stay_in_range() {
        for src in [
            "env(0.0, 0.1, 0.5, 0.1, dur)\n",   // no attack
            "env(0.1, 0.0, 0.5, 0.1, dur)\n",   // no decay
            "env(0.1, 0.1, 0.0, 0.1, dur)\n",   // sustain 0
            "env(0.1, 0.1, 0.5, 0.0, dur)\n",   // no release
            "env(0.1, 0.1, 0.5, 5.0, dur)\n",   // release longer than the note
            "env(5.0, 5.0, 0.5, 0.1, dur)\n",   // attack+decay overrun the note
            "env(0.1, 0.1, 2.0, 0.1, dur)\n",   // sustain above 1, clamped
        ] {
            let s = render_voice(src, 0.5, 0.8);
            assert!(
                s.iter().all(|v| v.is_finite() && *v >= -0.001 && *v <= 1.001),
                "{src} left 0..=1"
            );
        }
        let s = render_voice("perc(0.0, 0.0)\n", 0.5, 0.2);
        assert!(s.iter().all(|v| v.is_finite() && *v >= -0.001 && *v <= 1.001));
    }

    /// `dur` exists only inside a voice.
    #[test]
    fn env_outside_a_voice_has_no_duration() {
        let items = parse("env(0.1, 0.1, 0.5, 0.2, dur)\n".to_string()).unwrap();
        // Lowered has no Debug, so unwrap_err() is unavailable here.
        let err = match lower(&items) {
            Err(e) => e,
            Ok(_) => panic!("expected `dur` to be unbound outside a voice"),
        };
        assert!(err.contains("unbound name: dur"), "got: {err}");
    }

    /// Envelope arguments are baked at construction, so a signal is rejected.
    #[test]
    fn envelope_arguments_must_be_constants() {
        let graph = SwyncGraph {
            nodes: vec![
                UGenNode { kind: NodeKind::Sin, inputs: vec![NodeInput::Const(2.0)], span: None },
                UGenNode {
                    kind: NodeKind::Perc,
                    inputs: vec![NodeInput::Node(NodeId(0)), NodeInput::Const(0.1)],
                    span: None,
                },
            ],
            output: Some(NodeId(1)),
            ..Default::default()
        };
        let err = match realize(&graph) {
            Err(e) => e,
            Ok(_) => panic!("expected an error for a signal-valued perc attack"),
        };
        assert!(err.contains("must be a constant"), "got: {err}");
    }

    /// The end-to-end shape: an oscillator shaped by an envelope decays away.
    #[test]
    fn an_enveloped_voice_decays_to_silence() {
        let s = render_voice("sin(220) * perc(0.005, 0.15)\n", 1.0, 0.6);
        let early = s[..(0.1 * SR) as usize].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let late = s[(0.4 * SR) as usize..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(early > 0.5, "should be audible early, peak {early}");
        assert!(late < 0.01, "should have decayed, peak {late}");
    }
}

#[cfg(test)]
mod reverb_tests {
    use super::*;
    use crate::lowerer::lower::lower;
    use crate::parser::parser::parse;

    const SR: f64 = 44100.0;

    /// Render `src` through the whole pipeline and return `secs` of samples.
    fn render(src: &str, secs: f64) -> Vec<f32> {
        let items = parse(src.to_string()).unwrap_or_else(|e| panic!("{src} failed to parse: {e}"));
        let g = lower(&items).unwrap_or_else(|e| panic!("{src} failed to lower: {e}")).graph;
        let mut net = realize(&g).unwrap_or_else(|e| panic!("{src} failed to realize: {e}"));
        net.check();
        net.set_sample_rate(SR);
        (0..(secs * SR) as usize).map(|_| net.get_mono()).collect()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// Each reverb turns an impulse into a tail that rings and then dies away.
    /// Short decay times keep the render — and so the test — brief.
    #[test]
    fn every_reverb_rings_and_decays() {
        for src in [
            "reverb(impulse(), 10, 0.4, 0.5)\n",
            "reverb2(impulse(), 10, 0.4, 0.5, 1, 8000)\n",
            "reverb3(impulse(), 0.4, 0.5, 8000)\n",
            "reverb4(impulse(), 20, 0.4)\n",
        ] {
            let s = render(src, 1.2);
            assert!(s.iter().all(|v| v.is_finite()), "{src} produced a non-finite sample");

            // reverb4 fades in, so the "early" window starts after its onset.
            let early = peak(&s[(0.05 * SR) as usize..(0.3 * SR) as usize]);
            let late = peak(&s[(0.9 * SR) as usize..]);
            assert!(early > 1e-4, "{src} made no tail, early peak was {early}");
            assert!(late < early * 0.5, "{src} did not decay: {early} then {late}");
        }
    }

    /// Wet only — the dry signal is not passed through, so mixing is the user's
    /// to do. A sine straight into a reverb comes back quieter than it went in.
    #[test]
    fn reverbs_are_wet_only() {
        let s = render("reverb(sin(220), 10, 0.4, 0.5)\n", 0.5);
        assert!(peak(&s) < 0.9, "a dry-inclusive reverb would keep the unit sine");
    }

    /// A non-positive decay time makes the feedback gain 1 or more. That is
    /// unbounded feedback, so it is rejected rather than realized.
    #[test]
    fn a_non_positive_time_is_rejected() {
        for src in [
            "reverb(impulse(), 10, 0, 0.5)\n",
            "reverb2(impulse(), 10, -1, 0.5, 1, 8000)\n",
            "reverb3(impulse(), 0, 0.5, 8000)\n",
            "reverb4(impulse(), 20, 0)\n",
        ] {
            let items = parse(src.to_string()).unwrap();
            let g = lower(&items).unwrap().graph;
            let err = match realize(&g) {
                Err(e) => e,
                Ok(_) => panic!("{src} should not realize"),
            };
            assert!(err.contains("must be above zero"), "{src} gave: {err}");
        }
    }

    /// The parameters are baked at construction, so a signal in one is an error
    /// rather than a wiring job.
    #[test]
    fn reverb_parameters_must_be_constants() {
        let items = parse("reverb(impulse(), 10, sin(1), 0.5)\n".to_string()).unwrap();
        let g = lower(&items).unwrap().graph;
        let err = match realize(&g) {
            Err(e) => e,
            Ok(_) => panic!("expected a signal-valued reverb time to be an error"),
        };
        assert!(err.contains("must be a constant"), "got: {err}");
    }
}

#[cfg(test)]
mod sample_tests {
    use super::*;
    use crate::lowerer::lower::lower_with_samples;
    use crate::parser::parser::parse;
    use std::sync::Arc;

    /// Lower and realize a program that reads one buffer: a one-second ramp
    /// from -1 to 1, so the output says where it was read from.
    fn render(src: &str, frames: usize) -> Vec<f32> {
        let mut wave = fundsp::wave::Wave::new(1, 1000.0);
        for i in 0..1000 {
            wave.push(i as f32 / 500.0 - 1.0);
        }
        let samples =
            crate::samples::Samples::from_pairs([("b.wav".to_string(), Arc::new(wave))]);

        let items = parse(src.to_string()).expect("parse failed");
        let lowered = lower_with_samples(&items, samples).expect("lower failed");
        let mut net = realize(&lowered.graph).expect("realize failed");
        net.check();
        net.set_sample_rate(44100.0);
        (0..frames).map(|_| net.get_mono()).collect()
    }

    /// The buffer really reaches the audio: a phasor across it renders the
    /// ramp that is in it, rising from about -1 to about 1.
    #[test]
    fn a_phasor_across_a_buffer_renders_what_is_in_it() {
        // 1 Hz over one second of buffer: one pass, in one second of audio.
        let s = render("sample(load(\"b.wav\"), ramp(1))\n", 44100);

        assert!(s.iter().all(|v| v.is_finite()));
        assert!(s[100] < -0.8, "should start near -1, got {}", s[100]);
        assert!(s[22050].abs() < 0.1, "should cross zero halfway, got {}", s[22050]);
        assert!(s[44000] > 0.8, "should end near 1, got {}", s[44000]);
    }

    /// And backwards, from the same buffer, with only the position changed.
    #[test]
    fn a_mirrored_phasor_renders_it_backwards() {
        let s = render("sample(load(\"b.wav\"), 1 - ramp(1))\n", 44100);

        assert!(s[100] > 0.8, "should start near 1, got {}", s[100]);
        assert!(s[44000] < -0.8, "should end near -1, got {}", s[44000]);
    }

    /// A position that never enters 0..1 is silence rather than a held edge.
    #[test]
    fn a_position_outside_the_buffer_is_silent() {
        let s = render("sample(load(\"b.wav\"), 2)\n", 4410);
        assert!(s.iter().all(|v| *v == 0.0), "should be silent");
    }
}

#[cfg(test)]
mod ramp_phase_tests {
    use super::*;
    use crate::lowerer::lower::lower;
    use crate::parser::parser::parse;

    fn render(src: &str, frames: usize) -> Vec<f32> {
        let items = parse(src.to_string()).expect("parse failed");
        let graph = lower(&items).expect("lower failed").graph;
        let mut net = realize(&graph).expect("realize failed");
        net.set_sample_rate(44100.0);
        (0..frames).map(|_| net.get_mono()).collect()
    }

    /// `ramp` must start at zero.
    ///
    /// fundsp seeds a bare `Ramp`'s phase from the node's hash, which makes it
    /// start partway up — and somewhere different each time the graph is built.
    /// Everything `sample` can do rests on a phasor whose zero is the start of
    /// the buffer, so this is load-bearing rather than cosmetic.
    #[test]
    fn a_ramp_starts_at_zero() {
        let s = render("ramp(1)\n", 10);
        assert!(s[0].abs() < 1e-6, "a ramp should begin at 0, got {}", s[0]);
        assert!(s[1] > s[0], "and rise from there");
    }

    /// And keeps starting there, however many nodes are around it to change
    /// the hash it would otherwise have taken its phase from.
    #[test]
    fn a_ramp_starts_at_zero_wherever_it_sits() {
        for src in [
            "ramp(2)\n",
            "sin(220) * 0 + ramp(2)\n",
            "let a = ramp(2)\nlet b = ramp(3)\na * 0 + b * 0 + ramp(2)\n",
        ] {
            let s = render(src, 4);
            assert!(s[0].abs() < 1e-6, "{src} began at {}", s[0]);
        }
    }

    /// A ramp really is a rising 0..1 phasor over its period, which is what
    /// makes `ramp(1 / buffer.secs)` read a buffer once end to end.
    #[test]
    fn a_ramp_rises_across_its_period() {
        // 1 Hz at 44100: one full pass per second.
        let s = render("ramp(1)\n", 44100);
        assert!(s[11025] > 0.24 && s[11025] < 0.26, "a quarter in, got {}", s[11025]);
        assert!(s[22050] > 0.49 && s[22050] < 0.51, "halfway, got {}", s[22050]);
        assert!(s[44099] > 0.99, "and nearly 1 at the end, got {}", s[44099]);
    }
}
