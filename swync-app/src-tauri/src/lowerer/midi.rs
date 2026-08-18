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

use crate::midi::input;
use crate::midi::out::{Destination, MAX_CHANNEL, MIN_CHANNEL};
use crate::midi::ports::{self, Match, Selector};
use crate::lowerer::lower::Lowerer;
use crate::parser::parser::{Arg, Expr};
use crate::swync_graph::environment::{Source, Value};
use crate::swync_graph::ugen_nodes::{NodeInput, NodeKind};

/// The name, which is also what a diagnostic calls it.
pub const MIDI_OUT: &str = "midiout";

/// Reading a keyboard, in `play`'s pattern slot.
pub const MIDI_IN: &str = "midiin";

/// The three continuous readers. Each is a graph node whose device has been
/// resolved to a slot here, because a slot is a number and a port's name is
/// not something the audio callback could look up — see `midi::input`.
pub const CC: &str = "cc";
pub const BEND: &str = "bend";
pub const AFTERTOUCH: &str = "aftertouch";

impl Lowerer {
    /// True when this call names a MIDI destination.
    pub fn is_midiout(name: &str) -> bool {
        name == MIDI_OUT
    }

    /// True when this call names a MIDI source to play.
    pub fn is_midiin(name: &str) -> bool {
        name == MIDI_IN
    }

    /// True when this call reads something continuous off a MIDI port.
    ///
    /// All three are intercepted before their arguments are evaluated for the
    /// same reason `midiout` is: the device may be written as a string, and a
    /// string is not a value in this language.
    pub fn is_midi_control(name: &str) -> bool {
        matches!(name, CC | BEND | AFTERTOUCH)
    }

    /// `cc(device, number, channel)`, `bend(device, channel)`,
    /// `aftertouch(device, channel)`, each with an optional `lo`/`hi` pair.
    ///
    /// The controller number comes before the channel because that is the
    /// order the two are reached for: which knob is the question, and which
    /// channel is a detail most rigs never have to answer.
    ///
    /// The device becomes a **slot** here and never appears in the graph as
    /// anything else. That is the whole reason this is intercepted: a node
    /// reads its controller on the audio callback, which cannot hash a port
    /// name, and interning the name to a small integer is what makes the read
    /// one relaxed load.
    pub fn midi_control(&mut self, name: &str, args: &[Arg], piped: Option<Value>)
        -> Result<Value, String>
    {
        let (device, rest) = self.device_and_rest(name, args, piped)?;

        // `cc` names a controller where the other two have only a channel.
        let (number, rest) = if name == CC {
            let Some((arg, rest)) = rest.split_first() else {
                return Err(format!(
                    "{CC} expects a controller number: {CC}(\"push\", 74)"));
            };
            (Some(self.whole(name, arg, "controller number", 0.0, 127.0)?), rest)
        } else {
            (None, rest)
        };

        let (channel, rest) = match rest.split_first() {
            None => (1.0, rest),
            Some((arg, rest)) => (self.whole(name, arg, "channel", 1.0, 16.0)?, rest),
        };

        // A wheel rests in the middle of its travel, so its natural range is
        // the one that puts zero there. The other two start at nothing and go
        // up, which is 0 to 1 like every other amount in the language.
        let (default_lo, default_hi) = if name == BEND { (-1.0, 1.0) } else { (0.0, 1.0) };
        let (lo, hi) = match rest {
            [] => (default_lo, default_hi),
            [lo, hi] => (self.arg_number(name, lo, "lo")?, self.arg_number(name, hi, "hi")?),
            _ => return Err(format!(
                "{name}: a range is both ends or neither — {name}(.., 200, 5000) maps the \
                 controller onto a cutoff, and no range at all is {default_lo} to \
                 {default_hi}")),
        };

        let slot = self.slot_for(name, &device);
        // In the order they are written, so that reading the node and reading
        // the call are the same exercise: slot, then whatever `cc` alone has,
        // then the channel, then the range.
        let inputs: Vec<NodeInput> = match number {
            Some(number) => vec![slot, number, channel, lo, hi],
            None => vec![slot, channel, lo, hi],
        }
        .into_iter()
        .map(NodeInput::Const)
        .collect();

        let kind = match name {
            CC => NodeKind::Cc,
            BEND => NodeKind::Bend,
            _ => NodeKind::Aftertouch,
        };
        Ok(Value::Signal(self.push_node(kind, inputs)))
    }

    /// `midiin(device, channel = any)` — a keyboard, to be played.
    pub fn midiin(&mut self, args: &[Arg], piped: Option<Value>) -> Result<Value, String> {
        let (device, rest) = self.device_and_rest(MIDI_IN, args, piped)?;

        // `None` is every channel, which is what a keyboard on its own means
        // and is the only sensible default: a player who has never thought
        // about MIDI channels should not have to guess which one their
        // keyboard is set to.
        let channel = match rest.split_first() {
            None => None,
            Some((arg, [])) => Some(self.whole(MIDI_IN, arg, "channel", 1.0, 16.0)? as u8),
            Some(_) => return Err(format!(
                "{MIDI_IN} expects at most 2 arguments, got {}", rest.len() + 1)),
        };

        let slot = self.slot_for(MIDI_IN, &device);
        Ok(Value::Source(Source { slot: slot as usize, channel }))
    }

    /// The device argument, and whatever follows it — the one thing all four
    /// of these names have in common.
    fn device_and_rest<'a>(
        &mut self,
        name: &str,
        args: &'a [Arg],
        piped: Option<Value>,
    ) -> Result<(Selector, &'a [Arg]), String> {
        match (piped, args.split_first()) {
            // A piped value fills the device, so every *written* argument is
            // still to come — `0.midiout(10)` is port 0 on channel 10. Taking
            // `split_first`'s tail here instead would drop the channel on the
            // floor, which is what this used to do.
            (Some(Value::Number(n)), _) => Ok((number_selector(name, n)?, args)),
            (Some(_), _) => Err(format!(
                "{name}: the device is a port's name or its number, and neither is \
                 something to chain into")),
            (None, Some((first, rest))) => Ok((selector(name, first)?, rest)),
            (None, None) => Err(format!(
                "{name} expects a port: {name}(\"keys\") names one, {name}(0) numbers one")),
        }
    }

    /// The slot this port reads from, warning if there is not one to give.
    ///
    /// Interning touches no hardware, so this is the same work whether or not
    /// anything is plugged in — see `midi::input`. Whether the port is
    /// actually *there* is settled later, by `ensure_open` on the eval that
    /// publishes this graph, because that is the only moment at which the
    /// answer is worth anything.
    fn slot_for(&mut self, name: &str, device: &Selector) -> f64 {
        match input::slot_for(device) {
            Some(slot) => slot as f64,
            None => {
                self.warnings.push(format!(
                    "{name}({device}): a program may read from {} MIDI ports and this is \
                     one more, so it will be silent. Nothing else about the program \
                     changes.",
                    input::MAX_PORTS));
                input::NO_SLOT as f64
            }
        }
    }

    /// A whole number written at the call, inside a range.
    fn whole(&mut self, name: &str, arg: &Arg, what: &str, lo: f64, hi: f64)
        -> Result<f64, String>
    {
        let n = self.arg_number(name, arg, what)?;
        if n.fract() != 0.0 || n < lo || n > hi {
            return Err(format!(
                "{name}: {what} must be a whole number from {lo} to {hi}, got {n}"));
        }
        Ok(n)
    }

    fn arg_number(&mut self, name: &str, arg: &Arg, what: &str) -> Result<f64, String> {
        match self.expr(&arg.value)? {
            Value::Number(n) if n.is_finite() => Ok(n),
            _ => Err(format!("{name}: {what} must be a compile-time number")),
        }
    }

    /// `midiout("deluge")`, `midiout(0)`, `midiout("deluge", 10)`.
    pub fn midiout(&mut self, args: &[Arg], piped: Option<Value>) -> Result<Value, String> {
        // A piped value fills the first parameter exactly as it does anywhere
        // else, so `0 >> midiout()` is the device. It is spelled out rather
        // than folded in with the positional arguments because the device is
        // read off the *syntax*, and a piped value has already been evaluated —
        // which for a number is fine and for a string can never happen.
        let (device, rest) = match (piped, args.split_first()) {
            // See `device_and_rest`: the pipe fills the device, so nothing
            // written has been consumed yet.
            (Some(Value::Number(n)), _) => (number_selector(MIDI_OUT, n)?, args),
            (Some(_), _) => return Err(format!(
                "{MIDI_OUT}: the device is a port's name or its number, and neither \
                 is something to chain into")),
            (None, Some((first, rest))) => (selector(MIDI_OUT, first)?, rest),
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
fn selector(name: &str, arg: &Arg) -> Result<Selector, String> {
    if arg.name.is_some() {
        return Err(format!("{name}: the device is not a named argument"));
    }
    match &arg.value {
        Expr::Str(name) => Ok(Selector::Name(name.clone())),
        // Read off the syntax rather than evaluated, so that the only numbers
        // that reach `number_selector` are ones written at the call. A port
        // number computed from something else would be a program whose gear
        // depends on its own arithmetic, which is not a thing anybody means.
        Expr::Num(n) => number_selector(name, *n),
        // A negative literal is a negation of a positive one as far as the
        // parser is concerned, so it has to be unwrapped here or `midiout(-1)`
        // is reported as not being written out — which is true of the syntax
        // and unhelpful about the mistake.
        Expr::Neg { expr } => match expr.as_ref() {
            Expr::Num(n) => number_selector(name, -*n),
            _ => Err(written_out(name)),
        },
        _ => Err(written_out(name)),
    }
}

fn written_out(name: &str) -> String {
    format!(
        "{name}: the device must be written out — {name}(\"deluge\") names \
         a port, {name}(0) numbers one")
}

fn number_selector(name: &str, n: f64) -> Result<Selector, String> {
    if n.fract() != 0.0 || n < 0.0 || !n.is_finite() {
        return Err(format!(
            "{name}: a port number is a whole number counted from 0, got {n}"));
    }
    Ok(Selector::Number(n as usize))
}
