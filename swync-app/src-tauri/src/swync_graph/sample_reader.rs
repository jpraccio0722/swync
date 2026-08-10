//! Reading a buffer at a position.
//!
//! The one thing fundsp does not already have. Its own `WavePlayer` is a
//! transport: it holds an index and adds one to it every sample, so it plays
//! forwards, at one speed, from wherever it was told to start. That is a
//! sampler's *controls*, and it means rate, pitch, reverse and scrub are each a
//! separate feature that has to be added to it.
//!
//! This is the other shape. There is no transport and no state — you hand it a
//! position and it hands back what is there, the way `sin` takes a frequency.
//! Everything a transport would have to offer becomes arithmetic on the
//! position, done with operators the language already has:
//!
//! ```text
//! sample(buf, ramp(1 / buf.secs))            // forwards, natural speed
//! sample(buf, 1 - ramp(1 / buf.secs))        // backwards
//! sample(buf, ramp(2 / buf.secs))            // twice as fast, an octave up
//! sample(buf, ramp(4 / buf.secs) * 0.25)     // the first quarter, looping
//! ```
//!
//! Being stateless is also what makes it safe to build one per note on the
//! scheduler thread: there is nothing to reset and nothing to get out of sync.

use std::sync::Arc;

use fundsp::prelude64::*;
use fundsp::wave::Wave;

/// Read one channel of a buffer at a position given as a signal.
///
/// - Input 0: position, 0 at the first sample and 1 at the end.
/// - Output 0: what is there.
#[derive(Clone)]
pub struct SampleReader {
    wave: Arc<Wave>,
    channel: usize,
}

impl SampleReader {
    /// `channel` is taken modulo the buffer's channel count, so reading channel
    /// 1 of a mono file is the mono, not a panic on the audio thread.
    pub fn new(wave: Arc<Wave>, channel: usize) -> Self {
        let channel = if wave.channels() == 0 { 0 } else { channel % wave.channels() };
        SampleReader { wave, channel }
    }

    /// One sample, or zero outside the buffer. Reading past either end as
    /// silence rather than as the nearest edge matters: an edge held is a DC
    /// offset and a click, and a phasor that overshoots is the normal case.
    #[inline]
    fn at(&self, index: isize) -> f32 {
        if index < 0 || index as usize >= self.wave.length() {
            0.0
        } else {
            self.wave.at(self.channel, index as usize)
        }
    }
}

impl AudioNode for SampleReader {
    // Chosen from the far end of fundsp's own range; the ids there run to 78.
    const ID: u64 = 200;
    type Inputs = typenum::U1;
    type Outputs = typenum::U1;

    #[inline]
    fn tick(&mut self, input: &Frame<f32, Self::Inputs>) -> Frame<f32, Self::Outputs> {
        let length = self.wave.length();
        if length == 0 {
            return [0.0].into();
        }

        // The position is a signal, so it can be anything at all — including
        // the NaN a division by zero somewhere upstream produces. Everything
        // below is index arithmetic, and a NaN through it is a silent, spreading
        // corruption of the output buffer rather than a loud failure.
        let position = input[0];
        if !position.is_finite() || position < 0.0 || position > 1.0 {
            return [0.0].into();
        }

        // 0 is the first sample and 1 is one past the last, so a phasor running
        // 0..1 reads the buffer exactly once with no sample favoured.
        let x = position * length as f32;
        let i = x.floor();
        let t = x - i;
        let i = i as isize;

        // Catmull-Rom through the two samples either side. Linear would alias
        // audibly the moment a break is played at anything but its own speed,
        // which is most of the point of reading it this way.
        let (y0, y1, y2, y3) = (self.at(i - 1), self.at(i), self.at(i + 1), self.at(i + 2));
        let a = y3 - y2 - y0 + y1;
        let b = y0 - y1 - a;
        let c = y2 - y0;
        [((a * t + b) * t + c) * t + y1].into()
    }

    fn route(&mut self, input: &SignalFrame, _frequency: f64) -> SignalFrame {
        // The output is unrelated to the input's amplitude — it is whatever is
        // in the buffer — so it is a generator, like `noise`.
        Routing::Generator(0.0).route(input, self.outputs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ramp from -1 to 1 across `length` samples, so a reader's output says
    /// exactly where in the buffer it read.
    fn ramp_wave(length: usize) -> Arc<Wave> {
        let mut wave = Wave::new(1, 44100.0);
        for i in 0..length {
            wave.push((i as f32 / (length - 1) as f32) * 2.0 - 1.0);
        }
        Arc::new(wave)
    }

    fn read(reader: &mut SampleReader, position: f32) -> f32 {
        let out: Frame<f32, typenum::U1> = reader.tick(&[position].into());
        out[0]
    }

    #[test]
    fn a_position_reads_that_part_of_the_buffer() {
        let mut r = SampleReader::new(ramp_wave(1000), 0);

        assert!((read(&mut r, 0.0) - -1.0).abs() < 0.01, "0 is the start");
        assert!(read(&mut r, 0.5).abs() < 0.01, "0.5 is the middle");
        assert!((read(&mut r, 0.999) - 1.0).abs() < 0.01, "the end is the end");
    }

    /// The whole design in one assertion: reading a mirrored position gives
    /// the buffer backwards, with no reverse feature anywhere in the node.
    #[test]
    fn a_mirrored_position_reads_backwards() {
        let mut r = SampleReader::new(ramp_wave(1000), 0);

        for p in [0.1f32, 0.25, 0.5, 0.75, 0.9] {
            let forward = read(&mut r, p);
            let backward = read(&mut r, 1.0 - p);
            assert!(
                (forward + backward).abs() < 0.01,
                "at {p}: forwards {forward}, backwards {backward} — should mirror"
            );
        }
    }

    /// Reading is a function of the position alone. If it were not, a voice
    /// built per note would depend on how many samples the last one rendered.
    #[test]
    fn reading_holds_no_state() {
        let mut r = SampleReader::new(ramp_wave(1000), 0);

        let first = read(&mut r, 0.3);
        for p in [0.0, 0.9, 0.15, 1.0] {
            let _ = read(&mut r, p);
        }
        assert_eq!(first, read(&mut r, 0.3), "the same position must read the same");
    }

    #[test]
    fn outside_the_buffer_is_silence() {
        let mut r = SampleReader::new(ramp_wave(1000), 0);

        for p in [-0.5f32, -0.001, 1.001, 2.0, 1e9] {
            assert_eq!(read(&mut r, p), 0.0, "position {p} should be silent");
        }
    }

    /// A NaN position must not become a NaN sample — it would spread through
    /// every mix downstream and silence the whole program.
    #[test]
    fn a_nonsense_position_is_silence_not_a_nan() {
        let mut r = SampleReader::new(ramp_wave(1000), 0);

        for p in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let out = read(&mut r, p);
            assert!(out.is_finite(), "position {p} produced {out}");
            assert_eq!(out, 0.0);
        }
    }

    /// Interpolation, not nearest-neighbour: a position between two samples of
    /// a straight line is on the line.
    #[test]
    fn positions_between_samples_interpolate() {
        let wave = ramp_wave(1000);
        let mut r = SampleReader::new(wave, 0);

        // Halfway between sample 500 and 501, in position terms.
        let a = read(&mut r, 500.0 / 1000.0);
        let half = read(&mut r, 500.5 / 1000.0);
        let b = read(&mut r, 501.0 / 1000.0);

        assert!(half > a && half < b, "expected {a} < {half} < {b}");
        assert!(((a + b) / 2.0 - half).abs() < 1e-4, "should sit on the line");
    }

    #[test]
    fn an_empty_buffer_is_silent_rather_than_a_panic() {
        let mut r = SampleReader::new(Arc::new(Wave::new(1, 44100.0)), 0);
        assert_eq!(read(&mut r, 0.5), 0.0);
    }

    /// A channel a buffer does not have wraps rather than panicking on the
    /// audio thread.
    #[test]
    fn a_channel_past_the_end_wraps() {
        let mut r = SampleReader::new(ramp_wave(1000), 7);
        assert!((read(&mut r, 0.0) - -1.0).abs() < 0.01, "should read the mono channel");
    }

    #[test]
    fn the_node_is_one_in_one_out() {
        let r = SampleReader::new(ramp_wave(10), 0);
        assert_eq!(r.inputs(), 1);
        assert_eq!(r.outputs(), 1);
    }
}
