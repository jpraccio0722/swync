//! Peak levels, for the meters in the title bar.
//!
//! Both ends of the audio path are metered — what arrives at the input, and
//! what leaves for the output device — and both are read by the same poll, so
//! they are the same thing twice and are written once here.
//!
//! **A peak since the last look, not a level at the moment of it.** The editor
//! asks ten times a second; a meter that answered with wherever the waveform
//! happened to be at the instant of the ask would sit near zero on a signal
//! that is clipping, because most samples of most signals are not near the
//! peak. Reading is therefore what resets it, and what a bar draws is "the
//! loudest thing in the last tenth of a second" — which is the only reading a
//! meter you are setting a gain by can afford to give.
//!
//! **Written from the audio callback, so nothing here allocates, locks or
//! branches much.** A block's peak is found in a local and published with one
//! atomic per channel, rather than an atomic per sample.

use std::sync::atomic::{AtomicU32, Ordering};

/// The loudest sample seen on each channel since somebody last looked.
pub struct Peaks {
    /// Magnitudes as `f32::to_bits`, one per channel. Its length never
    /// changes — a meter is allocated once and shared with a callback.
    channels: Vec<AtomicU32>,
}

impl Peaks {
    pub fn new(channels: usize) -> Peaks {
        Peaks { channels: (0..channels).map(|_| AtomicU32::new(0)).collect() }
    }

    /// Offer a magnitude, from the audio callback. Keeps it if it is louder
    /// than what is already there.
    ///
    /// The load-then-store is not atomic as a pair, and does not need to be:
    /// the only other writer is a reader resetting to zero, so the worst a
    /// race can do is carry one block's peak into the next drawing. That is a
    /// meter being a frame stale, which is what a meter is.
    #[inline]
    pub fn observe(&self, channel: usize, magnitude: f32) {
        let Some(slot) = self.channels.get(channel) else {
            return;
        };
        if magnitude > f32::from_bits(slot.load(Ordering::Relaxed)) {
            slot.store(magnitude.to_bits(), Ordering::Relaxed);
        }
    }

    /// The peak on every channel since this was last called, which resets it.
    pub fn take(&self) -> Vec<f32> {
        self.take_channels(self.channels.len())
    }

    /// The same, for the first `channels` of them — what an input with fewer
    /// channels open than the ring can hold reports, so a two-in interface
    /// draws two bars rather than sixteen.
    pub fn take_channels(&self, channels: usize) -> Vec<f32> {
        self.channels
            .iter()
            .take(channels)
            .map(|slot| f32::from_bits(slot.swap(0, Ordering::Relaxed)))
            .collect()
    }

    /// Forget everything, without reporting it. What closing a device does:
    /// the last level it sent is not a level anything is at now.
    pub fn clear(&self) {
        for slot in &self.channels {
            slot.store(0, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole contract in one test: loudest since the last look, and the
    /// look is what resets it.
    #[test]
    fn a_peak_is_the_loudest_thing_since_it_was_last_read() {
        let peaks = Peaks::new(2);
        peaks.observe(0, 0.2);
        peaks.observe(0, 0.8);
        peaks.observe(0, 0.3);
        peaks.observe(1, 0.1);

        assert_eq!(peaks.take(), vec![0.8, 0.1]);
        assert_eq!(peaks.take(), vec![0.0, 0.0], "reading is what resets it");
    }

    /// A meter that fell to whatever arrived last would read near zero on a
    /// signal that is clipping — most samples of most signals are not near the
    /// peak, and a tenth of a second holds thousands of them.
    #[test]
    fn a_quieter_sample_does_not_pull_the_peak_down() {
        let peaks = Peaks::new(1);
        peaks.observe(0, 1.0);
        for _ in 0..1000 {
            peaks.observe(0, 0.001);
        }
        assert_eq!(peaks.take(), vec![1.0]);
    }

    /// The audio callback is holding this, so a channel that is not there is a
    /// no-op rather than a panic on the one thread that must not have one.
    #[test]
    fn a_channel_that_is_not_there_is_ignored() {
        let peaks = Peaks::new(2);
        peaks.observe(7, 1.0);
        assert_eq!(peaks.take(), vec![0.0, 0.0]);
    }

    /// A device with fewer channels than the meter can hold draws that many
    /// bars, rather than a row of dead ones.
    #[test]
    fn only_the_channels_asked_for_are_reported() {
        let peaks = Peaks::new(16);
        peaks.observe(0, 0.5);
        peaks.observe(1, 0.25);
        assert_eq!(peaks.take_channels(2), vec![0.5, 0.25]);
    }

    /// Closing a device must not leave its last level on the meter, where it
    /// would sit for as long as nothing replaced it.
    #[test]
    fn clearing_leaves_nothing_to_report() {
        let peaks = Peaks::new(2);
        peaks.observe(0, 0.9);
        peaks.clear();
        assert_eq!(peaks.take(), vec![0.0, 0.0]);
    }
}
