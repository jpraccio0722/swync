//! Naming a MIDI port, so that a program can say which one it means.
//!
//! This is [`crate::devices`]'s counterpart and deliberately not its twin. An
//! audio device is chosen in the settings panel and remembered by a
//! `cpal::DeviceId`, because which interface is on the desk is a fact about
//! the desk and not about the piece. A MIDI port is chosen *in the program* —
//! `midiout("deluge")` — because which synth a part is written for is a fact
//! about the piece, and a piece that names its own gear can be read and
//! rehearsed without a settings panel being set up first.
//!
//! That choice is what the rest of this file is about. A name written in a
//! program has to survive being written by hand, which the ids `devices.rs`
//! persists never have to, so the two are matched quite differently.
//!
//! ## Two ways to say which port
//!
//! **By name**, matched case-insensitively on any part of the port's name.
//! Real port names are long, platform-shaped and full of words nobody chose —
//! "Deluge MIDI 1", "Scarlett 2i2 USB MIDI 1", "IAC Driver Bus 1". Demanding
//! the whole thing exactly would mean copying a string out of a panel into a
//! program that is being typed live, and would break a piece the day the
//! vendor adds a space. So `"deluge"` finds the Deluge, and the cost is that a
//! name matching two ports has to pick one — see [`Match::Ambiguous`].
//!
//! **By number**, indexing the list the operating system reports, which is
//! what [`outputs`] returns in order. It is the shorter thing to type and the
//! only thing to type when a port's name is unpronounceable. It is also the
//! less stable of the two: plugging in an interface mid-set can move every
//! number above it. That is a real cost and it is the reason the settings
//! panel lists the ports *with* their numbers — a number is only usable if
//! there is somewhere to read it off — and the reason a name is what a piece
//! meant to outlive the evening should use.
//!
//! Neither one failing is an error. See [`Match::Missing`]: a port that is not
//! here tonight is a warning in the problems panel and a program that still
//! runs, exactly as `input(channel)` is silence on a channel the device does
//! not have. A piece written against a rack has to stay editable on the
//! laptop it is edited on.

use midir::{MidiInput, MidiOutput};

/// What the client is called when it appears in other applications' port
/// lists. Fixed rather than per-connection: what somebody patching a DAW into
/// this sees should be the name of the program, not an implementation detail
/// of which part of it opened the port.
pub const CLIENT_NAME: &str = "swync";

/// One port, as the settings panel lists it and a program may name it.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    /// Its position in the list the operating system reports, which is what
    /// `midiout(0)` means. Carried rather than left implicit in the vector's
    /// order because it crosses to the frontend, where it is the number the
    /// panel prints beside the name.
    pub number: usize,
    /// What it is called, for saying so and for matching a name against.
    pub name: String,
}

/// How a program said which port it meant.
#[derive(Debug, Clone, PartialEq)]
pub enum Selector {
    /// `midiout("deluge")`.
    Name(String),
    /// `midiout(0)`.
    Number(usize),
}

impl std::fmt::Display for Selector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Quoted so that the sentence a diagnostic builds around this
            // reads as being about a name rather than about a word: `no MIDI
            // output matches "deluge"` says which part of it was written.
            Selector::Name(name) => write!(f, "\"{name}\""),
            Selector::Number(n) => write!(f, "{n}"),
        }
    }
}

/// What looking a selector up in a list of ports came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Match {
    /// Exactly one port answered to it.
    One(PortInfo),
    /// Several did. The first is used and the rest are named, because the
    /// alternative — refusing — turns a substring that has quietly become
    /// ambiguous, by something being plugged in, into silence in the middle of
    /// a set. Taking the first keeps the notes flowing and the warning says
    /// what to write instead.
    Ambiguous(PortInfo, Vec<String>),
    /// Nothing did.
    Missing,
}

/// Fold a port name for matching: case is not something anybody is going to
/// get right about a name they did not choose, and it carries no information
/// here — no two ports on one machine differ only in it.
fn folded(name: &str) -> String {
    name.to_lowercase()
}

/// Find the port a selector names among the ports there are.
///
/// A free function over a list rather than a method on a connection so that
/// the whole of this rule — which is the part with judgement in it — is
/// testable against a made-up list of names, with no MIDI hardware and no
/// machine-shaped list to depend on.
pub fn find(ports: &[PortInfo], selector: &Selector) -> Match {
    match selector {
        Selector::Number(n) => match ports.iter().find(|p| p.number == *n) {
            Some(port) => Match::One(port.clone()),
            None => Match::Missing,
        },
        Selector::Name(name) => {
            let wanted = folded(name);
            let hits: Vec<&PortInfo> =
                ports.iter().filter(|p| folded(&p.name).contains(&wanted)).collect();
            match hits.split_first() {
                None => Match::Missing,
                Some((first, [])) => Match::One((*first).clone()),
                Some((first, rest)) => Match::Ambiguous(
                    (*first).clone(),
                    rest.iter().map(|p| p.name.clone()).collect(),
                ),
            }
        }
    }
}

/// Every MIDI output on this machine, in the order the platform reports them —
/// which is the order their numbers count in.
///
/// A host that cannot be asked is an empty list rather than a failure, for the
/// same reason `devices::outputs` makes that choice: what the panel then
/// offers is nothing, which is the truth about what can be opened, and a
/// program naming a port still compiles and still runs.
pub fn outputs() -> Vec<PortInfo> {
    let Ok(midi) = MidiOutput::new(CLIENT_NAME) else {
        return Vec::new();
    };
    midi.ports()
        .iter()
        .enumerate()
        .filter_map(|(number, port)| {
            Some(PortInfo { number, name: midi.port_name(port).ok()? })
        })
        .collect()
}

/// Every MIDI input on this machine, counted the same way.
pub fn inputs() -> Vec<PortInfo> {
    let Ok(midi) = MidiInput::new(CLIENT_NAME) else {
        return Vec::new();
    };
    midi.ports()
        .iter()
        .enumerate()
        .filter_map(|(number, port)| {
            Some(PortInfo { number, name: midi.port_name(port).ok()? })
        })
        .collect()
}
