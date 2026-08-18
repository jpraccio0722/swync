//! `midiout(device, channel = 1)` — naming gear to play a pattern on.
//!
//! Intercepted before its arguments are evaluated, for the same reason `load`
//! is: a port may be named with a string, and a string is not a value in this
//! language. `Expr::Str` is refused everywhere else precisely so that text
//! cannot be carried around and mistaken for something that sounds — so the
//! two names that take one read it off the syntax instead.
//!
//! The other half of what happens here is the warning. A program says which
//! port it wants, and whether that port exists is a fact about the room rather
//! than about the program, so this is the one place in the lowerer that has
//! something to say without refusing anything. See [`crate::midi::ports`] for
//! why a missing port cannot be an error.

use crate::midi::out::{Destination, MAX_CHANNEL, MIN_CHANNEL};
use crate::midi::ports::{self, Match, Selector};
use crate::lowerer::lower::Lowerer;
use crate::parser::parser::{Arg, Expr};
use crate::swync_graph::environment::Value;

/// The name, which is also what a diagnostic calls it.
pub const MIDI_OUT: &str = "midiout";

impl Lowerer {
    /// True when this call names a MIDI destination.
    pub fn is_midiout(name: &str) -> bool {
        name == MIDI_OUT
    }

    /// `midiout("deluge")`, `midiout(0)`, `midiout("deluge", 10)`.
    pub fn midiout(&mut self, args: &[Arg], piped: Option<Value>) -> Result<Value, String> {
        // A piped value fills the first parameter exactly as it does anywhere
        // else, so `0 >> midiout()` is the device. It is spelled out rather
        // than folded in with the positional arguments because the device is
        // read off the *syntax*, and a piped value has already been evaluated —
        // which for a number is fine and for a string can never happen.
        let (device, rest) = match (piped, args.split_first()) {
            (Some(Value::Number(n)), rest) => {
                (number_selector(n)?, rest.map_or(&[][..], |(_, r)| r))
            }
            (Some(_), _) => return Err(format!(
                "{MIDI_OUT}: the device is a port's name or its number, and neither \
                 is something to chain into")),
            (None, Some((first, rest))) => (selector(first)?, rest),
            (None, None) => return Err(format!(
                "{MIDI_OUT} expects a port: {MIDI_OUT}(\"deluge\") names one, \
                 {MIDI_OUT}(0) numbers one")),
        };

        if let Some(named) = args.iter().find(|a| a.name.is_some()) {
            let name = named.name.as_ref().expect("found by name").0.clone();
            if name != "channel" {
                return Err(format!("{MIDI_OUT} has no parameter named '{name}'"));
            }
        }

        let channel = match rest.first() {
            None => MIN_CHANNEL,
            Some(arg) => match self.expr(&arg.value)? {
                Value::Number(n) if n.fract() == 0.0
                    && n >= MIN_CHANNEL as f64
                    && n <= MAX_CHANNEL as f64 => n as u8,
                Value::Number(n) => return Err(format!(
                    "{MIDI_OUT}: channel must be a whole number from {MIN_CHANNEL} to \
                     {MAX_CHANNEL}, got {n}")),
                _ => return Err(format!(
                    "{MIDI_OUT}: channel must be a compile-time number")),
            },
        };
        if rest.len() > 1 {
            return Err(format!(
                "{MIDI_OUT} expects at most 2 arguments, got {}", rest.len() + 1));
        }

        self.warn_about(&device);
        Ok(Value::Destination(Destination { selector: device, channel }))
    }

    /// Say something about a port that will not be found, or that more than one
    /// answers to.
    ///
    /// Asked once per `midiout` in the program rather than once per note, which
    /// is the only reason it can afford to enumerate the machine's ports at
    /// all. What it decides is thrown away — the thread that sends the notes
    /// resolves the port again, on every publish, because the rack can change
    /// between compiling a program and playing it.
    fn warn_about(&mut self, device: &Selector) {
        match ports::find(&ports::outputs(), device) {
            Match::One(_) => {}
            Match::Ambiguous(taken, others) => self.warnings.push(format!(
                "{MIDI_OUT}({device}) matches {} ports — playing to \"{}\" and not to {}. \
                 Name more of the port to choose between them.",
                others.len() + 1,
                taken.name,
                others.iter().map(|o| format!("\"{o}\"")).collect::<Vec<_>>().join(", "),
            )),
            Match::Missing => self.warnings.push(format!(
                "{MIDI_OUT}({device}): no MIDI output here answers to that, so this \
                 pattern will not be heard. The program still runs — the settings panel \
                 lists the ports this machine has, with their numbers.",
            )),
        }
    }
}

/// The device as it was written.
fn selector(arg: &Arg) -> Result<Selector, String> {
    if arg.name.is_some() {
        return Err(format!("{MIDI_OUT}: the device is not a named argument"));
    }
    match &arg.value {
        Expr::Str(name) => Ok(Selector::Name(name.clone())),
        // Read off the syntax rather than evaluated, so that the only numbers
        // that reach `number_selector` are ones written at the call. A port
        // number computed from something else would be a program whose gear
        // depends on its own arithmetic, which is not a thing anybody means.
        Expr::Num(n) => number_selector(*n),
        // A negative literal is a negation of a positive one as far as the
        // parser is concerned, so it has to be unwrapped here or `midiout(-1)`
        // is reported as not being written out — which is true of the syntax
        // and unhelpful about the mistake.
        Expr::Neg { expr } => match expr.as_ref() {
            Expr::Num(n) => number_selector(-*n),
            _ => Err(written_out()),
        },
        _ => Err(written_out()),
    }
}

fn written_out() -> String {
    format!(
        "{MIDI_OUT}: the device must be written out — {MIDI_OUT}(\"deluge\") names \
         a port, {MIDI_OUT}(0) numbers one")
}

fn number_selector(n: f64) -> Result<Selector, String> {
    if n.fract() != 0.0 || n < 0.0 || !n.is_finite() {
        return Err(format!(
            "{MIDI_OUT}: a port number is a whole number counted from 0, got {n}"));
    }
    Ok(Selector::Number(n as usize))
}
