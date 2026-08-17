//! Hearing a file — the play button beside a sample in the project tree.
//!
//! Auditioning is not running a program. Nothing is parsed, the persistent
//! graph is left alone, and whatever is playing goes on playing. What happens
//! instead is one voice, pushed into the same sequencer every pattern note goes
//! through — which is what makes it mix with the piece, reach the recorder's
//! tap and the meters, and fall silent with everything else when the transport
//! stops. A second output stream of its own would do none of those.
//!
//! The buffer comes out of [`crate::samples::Cache`], so a file heard here and
//! then named by a `load` is decoded once between them.

use std::sync::Arc;

use fundsp::net::Net;
use fundsp::prelude64::*;
use fundsp::wave::Wave;

use crate::lowerer::sample::seconds;
use crate::swync_graph::realizer::ENV_INTERVAL;
use crate::swync_graph::sample_reader::SampleReader;

/// One playthrough of a buffer, ready for the scheduler.
pub struct Audition {
    /// 0-in, 2-out, as every voice the sequencer takes has to be.
    pub net: Net,
    /// How long it plays for. The scheduler needs it for the event's length,
    /// and the editor needs it to know when the button it lit goes out.
    pub secs: f64,
}

/// Read a whole buffer once, at its own speed, in stereo.
///
/// This is the language's own `sample(buf, ...)` twice over, built from the
/// same reader — so what the button plays is what a program that loads the file
/// will hear. A mono file's second reader wraps round to its one channel, which
/// is [`SampleReader`]'s own rule, and means mono comes out of both speakers
/// rather than only the left.
///
/// The position is a line rather than the `ramp` the natural-speed idiom is
/// written with, and that is the one departure worth explaining. A ramp is a
/// phasor and wraps, so the moment the buffer ends it starts again — and the
/// scheduler deliberately holds this note open past the end of the buffer, so
/// that its fade-out lands on silence instead of on the sample's last twenty
/// milliseconds. With a ramp that tail would be the top of the file playing
/// underneath the fade. A line simply goes past 1, and a `SampleReader` reads
/// past the end as silence.
pub fn voice(wave: &Arc<Wave>) -> Result<Audition, String> {
    let secs = seconds(wave);
    if secs <= 0.0 {
        return Err("there is no audio in that file to play".to_string());
    }

    // 0 at the first sample and 1 at the last, which is what a `SampleReader`
    // reads positions as.
    let position = An(Envelope::new(ENV_INTERVAL, move |t: f64| -> f64 { t / secs }));
    let net = Net::wrap(Box::new(
        position
            >> (An(SampleReader::new(wave.clone(), 0)) ^ An(SampleReader::new(wave.clone(), 1))),
    ));

    Ok(Audition { net, secs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fundsp::prelude64::AudioUnit;

    const RATE: f64 = 44100.0;
    const LENGTH: usize = 4410; // a tenth of a second, long enough to measure

    /// A mono ramp from -1 to 1 across the buffer, so every rendered frame says
    /// exactly where in the file it was read from.
    fn a_ramp() -> Arc<Wave> {
        let mut wave = Wave::new(1, RATE);
        for i in 0..LENGTH {
            wave.push(at(i));
        }
        Arc::new(wave)
    }

    /// The same ramp with the right channel mirrored, so a test can tell which
    /// reader it is listening to.
    fn a_stereo_ramp() -> Arc<Wave> {
        let mut wave = Wave::new(2, RATE);
        for i in 0..LENGTH {
            wave.push((at(i), -at(i)));
        }
        Arc::new(wave)
    }

    fn at(i: usize) -> f32 {
        (i as f32 / (LENGTH - 1) as f32) * 2.0 - 1.0
    }

    fn played(wave: &Arc<Wave>, frames: usize) -> Vec<(f32, f32)> {
        let mut net = voice(wave).expect("should build").net;
        net.set_sample_rate(RATE);
        (0..frames).map(|_| net.get_stereo()).collect()
    }

    /// The whole point: a file plays back as itself. Frame `i` of the output is
    /// sample `i` of the buffer, which is what "at its own speed" means — and
    /// what a rate anywhere in this path getting the wrong number would break.
    #[test]
    fn a_buffer_is_played_back_at_its_own_speed() {
        let wave = a_ramp();
        let heard = played(&wave, LENGTH);

        for i in [0, LENGTH / 4, LENGTH / 2, (LENGTH * 3) / 4, LENGTH - 2] {
            let expected = wave.at(0, i);
            assert!(
                (heard[i].0 - expected).abs() < 0.02,
                "frame {i} should be sample {i} ({expected}), heard {}",
                heard[i].0
            );
        }
    }

    /// The buffer is read once and then it is over. A phasor here would loop
    /// instead, which is the reason the position is a line — see [`voice`].
    #[test]
    fn past_the_end_of_the_buffer_is_silence_rather_than_the_start_again() {
        let wave = a_ramp();
        let heard = played(&wave, LENGTH * 2);

        for (i, (l, r)) in heard.iter().enumerate().skip(LENGTH + 8) {
            assert!(
                l.abs() < 1e-6 && r.abs() < 1e-6,
                "frame {i} is past the end and should be silent, heard ({l}, {r})"
            );
        }
    }

    /// A mono file heard on one side only is the classic version of this bug,
    /// and it sounds like the sample is quieter than it is.
    #[test]
    fn a_mono_file_is_heard_on_both_sides() {
        let heard = played(&a_ramp(), LENGTH);
        let (l, r) = heard[LENGTH / 4];
        assert!(l.abs() > 0.1, "should be audible, heard {l}");
        assert_eq!(l, r, "mono should be the same on both sides");
    }

    /// And a stereo file keeps its two sides apart, which the mirrored right
    /// channel of the fixture makes visible.
    #[test]
    fn a_stereo_file_keeps_its_channels() {
        let heard = played(&a_stereo_ramp(), LENGTH);
        let (l, r) = heard[LENGTH / 4];
        assert!(l.abs() > 0.1, "should be audible, heard {l}");
        assert!((l + r).abs() < 0.02, "the channels should differ: ({l}, {r})");
    }

    /// A voice is 0-in/2-out or the scheduler refuses it — which would be a
    /// button that does nothing and says nothing.
    #[test]
    fn the_voice_is_the_shape_the_sequencer_takes() {
        let net = voice(&a_ramp()).expect("should build").net;
        assert_eq!(net.inputs(), 0);
        assert_eq!(net.outputs(), 2);
    }

    #[test]
    fn how_long_it_plays_for_is_how_long_the_file_is() {
        let audition = voice(&a_ramp()).expect("should build");
        assert!((audition.secs - LENGTH as f64 / RATE).abs() < 1e-9);
    }

    /// A file with nothing in it has no position to read and no length to play
    /// for — the rate it would be divided by is zero.
    #[test]
    fn a_buffer_with_nothing_in_it_is_refused() {
        assert!(voice(&Arc::new(Wave::new(1, RATE))).is_err());
    }
}
