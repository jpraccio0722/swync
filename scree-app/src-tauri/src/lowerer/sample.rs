//! `load`, `sample` and `secs` — the three names a buffer needs — and `slice`,
//! which is the two of them a portion is usually written with, said once.
//!
//! `load("break.wav")` is intercepted before its argument is evaluated, the way
//! `play` is and for the same kind of reason: the path has to be a literal.
//! `samples::Samples` was built by walking the syntax for exactly these calls
//! before any of this ran, so a path assembled at runtime would name a file
//! nobody had loaded — and the only moment it could be loaded is on the audio
//! thread, one note at a time.
//!
//! `sample(buffer, position)` is a UGen like any other except for its first
//! argument, which is a buffer rather than a signal. That is what keeps it out
//! of the `UGENS` table's ordinary path: the table's lowering takes every
//! argument through `as_input`, and a buffer has no input to be.
//!
//! `slice(buffer, start, end)` is not a fourth thing. It lowers to a `Line`
//! feeding that same `Sample`, so it adds no node, no state and no case to the
//! realizer — it only does the one piece of arithmetic that everybody writing
//! the phasor by hand got wrong. A position is a fraction of the buffer, so
//! covering `end - start` of it at the buffer's own speed takes
//! `(end - start) * secs` seconds, and a phasor whose rate does not say so
//! reads the right portion at the wrong speed. Naming the two ends means the
//! rate is never written down and so cannot disagree with them.
//!
//! `secs` is neither — it answers with a number during lowering, like the maths
//! builtins, because the length of a buffer is known the moment it is loaded.

use std::sync::Arc;

use fundsp::wave::Wave;

use crate::lowerer::lower::Lowerer;
use crate::parser::parser::{Arg, Expr};
use crate::samples::LOAD;
use crate::scree_graph::environment::Value;
use crate::scree_graph::ugen_nodes::{NodeInput, NodeKind};

/// Reading a buffer at a position.
pub const SAMPLE: &str = "sample";
/// Reading a portion of a buffer, once, at the buffer's own speed.
pub const SLICE: &str = "slice";
/// How long a buffer is, in seconds.
pub const SECS: &str = "secs";
/// How many channels a buffer has.
pub const CHANNELS: &str = "channels";

impl Lowerer {
    /// True when this call names a file rather than computing something.
    pub fn is_load(name: &str) -> bool {
        name == LOAD
    }

    /// `load("break.wav")` — the buffer that path was decoded into.
    ///
    /// Nothing is read from disk here. The map was filled at eval time; a miss
    /// means the walk that filled it did not see this call, which is a bug in
    /// the walk rather than anything a program did.
    pub fn load(&mut self, args: &[Arg], piped: Option<Value>) -> Result<Value, String> {
        if piped.is_some() {
            return Err(format!("{LOAD}: a path is not something to chain into"));
        }
        let [arg] = args else {
            return Err(format!(
                "{LOAD} expects one path, written out: {LOAD}(\"break.wav\")"));
        };
        if arg.name.is_some() {
            return Err(format!("{LOAD}: the path is not a named argument"));
        }
        let Expr::Str(path) = &arg.value else {
            return Err(format!(
                "{LOAD}: the path must be written out as a string — \
                 {LOAD}(\"break.wav\") — so the file can be read before anything plays"));
        };

        match self.samples.get(path) {
            Some(wave) => Ok(Value::Buffer(wave)),
            None => Err(format!(
                "{LOAD}: \"{path}\" was not loaded before this program ran")),
        }
    }

    /// True when this call reads or measures a buffer.
    pub fn is_buffer_builtin(name: &str) -> bool {
        matches!(name, SAMPLE | SLICE | SECS | CHANNELS)
    }

    /// The four that take a buffer as their first argument.
    ///
    /// Dispatched together, after arguments are evaluated, because all four
    /// begin by insisting on a buffer and the message for not having one should
    /// be the same however you got here.
    pub fn buffer_builtin(&mut self, name: &str, args: &[Value]) -> Result<Value, String> {
        let Some(Value::Buffer(wave)) = args.first().cloned() else {
            return Err(format!(
                "{name}: the first argument must be a buffer from `{LOAD}(\"...\")`"));
        };

        match name {
            SECS => arity(name, args, 1).map(|_| Value::Number(seconds(&wave))),
            CHANNELS => arity(name, args, 1).map(|_| Value::Number(wave.channels() as f64)),
            SAMPLE => {
                if args.len() < 2 || args.len() > 3 {
                    return Err(format!(
                        "{SAMPLE} expects a buffer and a position, and optionally a \
                         channel: {SAMPLE}(buffer, position) — got {} arguments",
                        args.len()));
                }
                let position = self.as_input(args[1].clone())?;
                let channel = channel(SAMPLE, args.get(2))?;

                let index = self.graph.intern_sample(wave) as f64;
                let node = self.push_node(
                    NodeKind::Sample,
                    vec![position, NodeInput::Const(index), NodeInput::Const(channel)],
                );
                Ok(Value::Signal(node))
            }
            SLICE => {
                if args.len() < 3 || args.len() > 5 {
                    return Err(format!(
                        "{SLICE} expects a buffer and two positions, and optionally a \
                         rate and a channel: {SLICE}(buffer, start, end) — got {} \
                         arguments",
                        args.len()));
                }
                let start = end_of_slice(START, &args[1])?;
                let end = end_of_slice(END, &args[2])?;
                let rate = rate(args.get(3))?;
                let channel = channel(SLICE, args.get(4))?;

                // The line that carries the reader from one end to the other,
                // over exactly as long as that much of the buffer lasts. A
                // slice played backwards is the same length of audio, so the
                // distance is unsigned and `start` past `end` reverses it.
                //
                // The rate is spent here and nowhere else: covering the same
                // distance in half the time is playing it twice as fast, which
                // is the only thing a speed can mean to a reader that has no
                // transport to speed up. Dividing rather than multiplying is
                // what makes 2 faster — this is a duration, and the rate is not.
                //
                // `line` holds at its end rather than falling silent, so the
                // reader parks on the last sample of the slice and that becomes
                // a DC offset. An envelope is what ends the note — the same
                // thing that ends every other voice — and a slice ending at 1
                // is silent for free, since past the buffer already reads as
                // nothing.
                let secs = (end - start).abs() * seconds(&wave) / rate;
                let position = self.push_node(
                    NodeKind::Line,
                    vec![
                        NodeInput::Const(start),
                        NodeInput::Const(end),
                        NodeInput::Const(secs),
                    ],
                );

                let index = self.graph.intern_sample(wave) as f64;
                let node = self.push_node(
                    NodeKind::Sample,
                    vec![
                        NodeInput::Node(position),
                        NodeInput::Const(index),
                        NodeInput::Const(channel),
                    ],
                );
                Ok(Value::Signal(node))
            }
            _ => Err(format!("{name} is not a buffer function")),
        }
    }
}

/// The names the two ends of a slice are refused by, so a message says which
/// one the program got wrong rather than that one of them is wrong.
const START: &str = "start";
const END: &str = "end";

/// One end of a slice.
///
/// A compile-time number rather than a signal, because it is spent on a line
/// that is drawn once — and inside the buffer, because `slice` exists to be the
/// spelling that cannot quietly read nothing. `sample` is where a position may
/// be anything at all and silence is the honest answer for the rest; here both
/// ends are written down in the source, so an unreachable one is a typo and
/// worth saying so.
fn end_of_slice(which: &str, arg: &Value) -> Result<f64, String> {
    match arg {
        // NaN fails this the same way 2 does — `contains` is false for it —
        // and lands on the message about the range, which is true of it.
        Value::Number(n) if (0.0..=1.0).contains(n) => Ok(*n),
        Value::Number(n) => Err(format!(
            "{SLICE}: {which} must be between 0 and 1 — 0 is the start of the \
             buffer and 1 is the end — got {n}")),
        _ => Err(format!(
            "{SLICE}: {which} is one end of a line drawn when the graph is built, \
             so it must be a compile-time number rather than a signal. For a \
             position that moves, use `{SAMPLE}`.")),
    }
}

/// How fast a slice is read, as a multiple of the buffer's own speed.
///
/// Compile-time for the same reason the ends are: it is spent on the length of
/// a line that is drawn once. Resampling is what makes it a speed, so it is a
/// pitch as much as a tempo — 2 is an octave up, and there is no way to have
/// one without the other short of a pitch shifter the language does not have.
///
/// Zero and below are refused rather than interpreted. At zero the reader would
/// stop rather than slow — the DC offset `slice` exists to avoid — and a
/// negative rate would be a second spelling of backwards, which the ends
/// already say more plainly. `accel` refuses its rates on the first of those
/// grounds and this is the same refusal.
fn rate(arg: Option<&Value>) -> Result<f64, String> {
    match arg {
        None => Ok(1.0),
        Some(Value::Number(n)) if *n > 0.0 && n.is_finite() => Ok(*n),
        Some(Value::Number(n)) if *n <= 0.0 => Err(format!(
            "{SLICE}: rate must be above zero, got {n} — at zero the reader would \
             stop rather than slow. To play a portion backwards, write its ends \
             the other way round: {SLICE}(b, 0.5, 0.25)")),
        Some(Value::Number(n)) => Err(format!(
            "{SLICE}: rate must be a real multiple of the buffer's own speed, got {n}")),
        Some(_) => Err(format!(
            "{SLICE}: rate sets the length of a line drawn when the graph is built, \
             so it must be a compile-time number rather than a signal. For a speed \
             that moves, use `{SAMPLE}`.")),
    }
}

/// Which channel of a buffer a reader is built over.
///
/// Shared by `sample` and `slice`: both bake it into the reader rather than
/// consulting it per sample, so both refuse a signal for the same reason and
/// should say so in the same words.
fn channel(name: &str, arg: Option<&Value>) -> Result<f64, String> {
    match arg {
        // A stereo file read on one channel is the usual case, so the channel
        // defaults rather than being asked for every time.
        None => Ok(0.0),
        Some(Value::Number(n)) if *n >= 0.0 && n.fract() == 0.0 => Ok(*n),
        Some(Value::Number(n)) => Err(format!(
            "{name}: channel must be a whole number from 0, got {n}")),
        Some(_) => Err(format!(
            "{name}: channel is chosen when the graph is built, so it must be a \
             compile-time number rather than a signal")),
    }
}

/// How long a buffer plays for, in seconds. Zero for a buffer with no sample
/// rate to speak of, rather than an infinity that would spread into whatever
/// the program divides by it.
fn seconds(wave: &Arc<Wave>) -> f64 {
    if wave.sample_rate() > 0.0 {
        wave.length() as f64 / wave.sample_rate()
    } else {
        0.0
    }
}

fn arity(name: &str, args: &[Value], expected: usize) -> Result<(), String> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(format!("{name} expects {expected} argument, got {}", args.len()))
    }
}
