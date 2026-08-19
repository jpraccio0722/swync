//! MIDI: a program naming gear outside the machine.
//!
//! Two halves, and they meet the rest of the engine in quite different
//! places. Output ([`out`]) is a fourth thing the scheduler can do with a
//! pattern — beside a `fundsp` voice and the persistent graph — and lives on a
//! thread of its own because a MIDI message can only be sent at the moment it
//! is due. Input arrives as [`crate::audio_in`]'s neighbour: something outside
//! writing into the engine, read by the graph.
//!
//! [`ports`] is what both share, and is where the one decision that is not
//! either half's lives: a port is named *in the program* rather than chosen in
//! the settings panel, which is the opposite of what an audio device does and
//! is explained there.

pub mod input;
pub mod out;
pub mod ports;

#[cfg(test)]
mod tests;
