//! A WAV file, written as the audio arrives rather than at the end of it.
//!
//! fundsp can already save a `Wave`, and that is the wrong shape for this: a
//! `Wave` is the whole recording in memory, and a performance has no length
//! anybody knows in advance. Half an hour of stereo float is 350 megabytes,
//! and the first thing that would go wrong is the one thing a recorder must
//! not do — run out of room part-way through a take.
//!
//! So this writes a header with its sizes left blank, appends samples as they
//! come, and fills the sizes in afterwards. The sizes are also patched
//! periodically while recording ([`Writer::checkpoint`]), which costs two seeks
//! a second and means a file left behind by a crash is still playable up to
//! the last checkpoint instead of being a header full of zeros.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// What a recording is written as.
///
/// Both are WAV, and that is not a list waiting to be filled in. The engine
/// renders `f32` and this writes it straight out, so these two cost nothing
/// but a conversion; anything compressed would bring an encoder, a bitrate to
/// argue about and a lossy answer to "what did that sound like". What differs
/// between them is only what a sample is stored as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Two bytes a sample, clipped to ±1. Half the size, and what every piece
    /// of software that reads audio at all can read.
    #[default]
    Wav16,
    /// Four bytes a sample, written exactly as the engine rendered it —
    /// including anything above unity, which 16-bit would clip off.
    Wav32,
}

impl Format {
    /// What a file in this format is called.
    pub fn extension(self) -> &'static str {
        "wav"
    }

    /// The WAV format tag: 1 is integer PCM, 3 is IEEE float.
    fn tag(self) -> u16 {
        match self {
            Format::Wav16 => 1,
            Format::Wav32 => 3,
        }
    }

    fn bytes_per_sample(self) -> u16 {
        match self {
            Format::Wav16 => 2,
            Format::Wav32 => 4,
        }
    }

    /// How long the `fmt ` chunk's body is.
    ///
    /// A float WAV's is 18 rather than 16: the two extra bytes are a `cbSize`
    /// of zero, which the spec requires of every format that is not integer
    /// PCM. Plenty of readers accept a 16-byte one — fundsp writes it that way
    /// — but the strict ones are the DAWs a recording is most likely to be
    /// opened in, so this writes what the spec asks for.
    fn fmt_len(self) -> u32 {
        match self {
            Format::Wav16 => 16,
            Format::Wav32 => 18,
        }
    }

    /// Where the samples start, which is also how long the header is.
    fn header_len(self) -> u64 {
        // "RIFF" + size + "WAVE" + "fmt " + size + body + "data" + size
        let base = 12 + 8 + self.fmt_len() as u64 + 8;
        match self {
            Format::Wav16 => base,
            // A float WAV also carries a `fact` chunk naming the frame count,
            // for the same reason the longer `fmt ` is there.
            Format::Wav32 => base + 12,
        }
    }

    /// Where in the file the `fact` chunk's frame count sits, for the formats
    /// that have one.
    fn fact_offset(self) -> Option<u64> {
        match self {
            Format::Wav16 => None,
            Format::Wav32 => Some(12 + 8 + self.fmt_len() as u64 + 8),
        }
    }
}

/// Where the `RIFF` chunk's size sits: immediately after the tag.
const RIFF_SIZE_OFFSET: u64 = 4;

/// One sample as this format stores it, appended to `out`.
fn encode(format: Format, sample: f32, out: &mut Vec<u8>) {
    match format {
        // The same conversion fundsp's own writer makes, so a file recorded
        // here and a file `load` reads are the same arithmetic — 32767.49
        // rather than 32767 so that ±1 lands on the ends of the range rather
        // than half a step inside it.
        Format::Wav16 => {
            let scaled = (sample.clamp(-1.0, 1.0) * 32767.49).round() as i16;
            out.extend_from_slice(&scaled.to_le_bytes());
        }
        Format::Wav32 => out.extend_from_slice(&sample.to_le_bytes()),
    }
}

/// A WAV file open for writing.
pub struct Writer {
    file: BufWriter<File>,
    format: Format,
    channels: u16,
    /// Bytes of audio written so far. Every size in the header is computed
    /// from this, so it is the one count that has to be right.
    data_bytes: u64,
    /// Encoded samples on their way to the file, kept between calls so a
    /// recording allocates once rather than once a block.
    scratch: Vec<u8>,
}

impl Writer {
    /// Open a file and write its header, with the sizes left to be filled in.
    pub fn create(
        path: &Path,
        format: Format,
        sample_rate: f64,
        channels: u16,
    ) -> std::io::Result<Writer> {
        let file = File::create(path)?;
        let mut writer = Writer {
            file: BufWriter::new(file),
            format,
            channels,
            data_bytes: 0,
            scratch: Vec::new(),
        };
        writer.write_header(sample_rate)?;
        Ok(writer)
    }

    fn write_header(&mut self, sample_rate: f64) -> std::io::Result<()> {
        let format = self.format;
        let bytes = format.bytes_per_sample();
        let block_align = self.channels * bytes;
        let byte_rate = sample_rate.round() as u32 * block_align as u32;

        let mut header: Vec<u8> = Vec::with_capacity(format.header_len() as usize);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&0u32.to_le_bytes()); // patched by `checkpoint`
        header.extend_from_slice(b"WAVE");

        header.extend_from_slice(b"fmt ");
        header.extend_from_slice(&format.fmt_len().to_le_bytes());
        header.extend_from_slice(&format.tag().to_le_bytes());
        header.extend_from_slice(&self.channels.to_le_bytes());
        header.extend_from_slice(&(sample_rate.round() as u32).to_le_bytes());
        header.extend_from_slice(&byte_rate.to_le_bytes());
        header.extend_from_slice(&block_align.to_le_bytes());
        header.extend_from_slice(&(bytes * 8).to_le_bytes());
        if format.fmt_len() == 18 {
            header.extend_from_slice(&0u16.to_le_bytes()); // cbSize
        }

        if format.fact_offset().is_some() {
            header.extend_from_slice(b"fact");
            header.extend_from_slice(&4u32.to_le_bytes());
            header.extend_from_slice(&0u32.to_le_bytes()); // patched too
        }

        header.extend_from_slice(b"data");
        header.extend_from_slice(&0u32.to_le_bytes()); // and this

        debug_assert_eq!(header.len() as u64, format.header_len());
        self.file.write_all(&header)
    }

    /// The most audio a file in this format can hold.
    ///
    /// Every size in a WAV header is a `u32`, so the format cannot name a file
    /// past four gigabytes — about six hours of 16-bit stereo at 48 kHz. Going
    /// past it does not fail, it *wraps*, and what comes out is a file whose
    /// header says it is much shorter than it is. Refusing is the honest end:
    /// what has been recorded so far is closed properly and kept.
    fn capacity(&self) -> u64 {
        // riff size is everything after its own field, so the header's first
        // eight bytes do not count against it.
        u32::MAX as u64 - (self.format.header_len() - 8)
    }

    /// Append interleaved samples.
    pub fn write(&mut self, samples: &[f32]) -> std::io::Result<()> {
        let adding = samples.len() as u64 * self.format.bytes_per_sample() as u64;
        if self.data_bytes + adding > self.capacity() {
            return Err(std::io::Error::other(format!(
                "a WAV file cannot hold more than {} hours of this, and this \
                 recording has reached it — it has been saved as it stands",
                self.capacity() / (48_000 * 2 * self.format.bytes_per_sample() as u64) / 3600,
            )));
        }

        self.scratch.clear();
        self.scratch.reserve(adding as usize);
        for &sample in samples {
            encode(self.format, sample, &mut self.scratch);
        }
        self.file.write_all(&self.scratch)?;
        self.data_bytes += adding;
        Ok(())
    }

    /// Fill in the header's sizes for what has been written so far, and leave
    /// the cursor back at the end where the next samples go.
    ///
    /// Called during a recording as well as at the end of one, which is what
    /// makes an interrupted file playable — a WAV whose header still says zero
    /// is not a short recording, it is nothing at all.
    pub fn checkpoint(&mut self) -> std::io::Result<()> {
        let data = self.data_bytes as u32;
        let riff = (self.format.header_len() - 8 + self.data_bytes) as u32;

        self.file.seek(SeekFrom::Start(RIFF_SIZE_OFFSET))?;
        self.file.write_all(&riff.to_le_bytes())?;

        if let Some(offset) = self.format.fact_offset() {
            let frames = self.frames() as u32;
            self.file.seek(SeekFrom::Start(offset))?;
            self.file.write_all(&frames.to_le_bytes())?;
        }

        self.file.seek(SeekFrom::Start(self.format.header_len() - 4))?;
        self.file.write_all(&data.to_le_bytes())?;

        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    /// Frames written so far — samples divided by the channels they were
    /// interleaved from.
    pub fn frames(&self) -> u64 {
        self.data_bytes / (self.format.bytes_per_sample() as u64 * self.channels as u64)
    }

    /// Close the file: sizes filled in, buffers flushed, bytes on the disk.
    ///
    /// `sync_all` rather than a plain flush because the recording is the whole
    /// point of the exercise, and a machine that loses power a second later
    /// should still have it.
    pub fn finish(mut self) -> std::io::Result<()> {
        self.checkpoint()?;
        self.file.flush()?;
        self.file.get_ref().sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fundsp::wave::Wave;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "scree-wav-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("should create a temp folder");
        dir
    }

    /// A second of a tone, interleaved as the engine renders it.
    fn tone(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let t = i as f32 / 48_000.0;
                let left = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                [left, -left]
            })
            .collect()
    }

    /// The whole point of the module: what was handed to it is what comes back
    /// out, at the rate it was recorded at, in something everything can read.
    /// Read back with fundsp's own loader, which is symphonia — the same
    /// decoder `load` uses, so a recording can be sampled by the next program.
    #[test]
    fn a_recording_reads_back_as_the_samples_it_was_given() {
        let root = temp("round-trip");
        for (format, tolerance) in [(Format::Wav16, 1.0 / 32767.0), (Format::Wav32, 0.0)] {
            let path = root.join(format!("{format:?}.wav"));
            let mut writer =
                Writer::create(&path, format, 48_000.0, 2).expect("should open the file");

            let samples = tone(4800);
            // In blocks, because that is how a recording arrives.
            for block in samples.chunks(512) {
                writer.write(block).expect("should write");
            }
            assert_eq!(writer.frames(), 4800);
            writer.finish().expect("should close the file");

            let wave = Wave::load(&path).expect("should read back");
            assert_eq!(wave.channels(), 2, "{format:?} lost a channel");
            assert_eq!(wave.length(), 4800, "{format:?} is the wrong length");
            assert_eq!(wave.sample_rate(), 48_000.0, "{format:?} is at the wrong rate");

            for i in 0..wave.length() {
                let (want_l, want_r) = (samples[i * 2], samples[i * 2 + 1]);
                assert!(
                    (wave.at(0, i) - want_l).abs() <= tolerance,
                    "{format:?} frame {i} left was {} not {want_l}",
                    wave.at(0, i)
                );
                assert!(
                    (wave.at(1, i) - want_r).abs() <= tolerance,
                    "{format:?} frame {i} right was {} not {want_r}",
                    wave.at(1, i)
                );
            }
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The reason the header is patched as it goes: a take that ends in a
    /// crash rather than a stop is still a take, and must still play.
    #[test]
    fn a_recording_interrupted_after_a_checkpoint_is_still_playable() {
        let root = temp("interrupted");
        let path = root.join("take.wav");

        let mut writer = Writer::create(&path, Format::Wav16, 48_000.0, 2).expect("should open");
        writer.write(&tone(2400)).expect("should write");
        writer.checkpoint().expect("should patch the header");
        // More audio arrives, and then the process dies: this is never closed.
        writer.write(&tone(2400)).expect("should write");
        drop(writer);

        let wave = Wave::load(&path).expect("an interrupted file should still read");
        // What the last checkpoint accounted for, which is the guarantee. The
        // samples after it are in the file but not in its header.
        assert_eq!(wave.length(), 2400);
        std::fs::remove_dir_all(&root).ok();
    }

    /// Past four gigabytes a WAV's sizes wrap, and a wrapped header describes
    /// a file that is not the one on the disk. It has to be refused rather
    /// than written, and the refusal has to say what happened.
    #[test]
    fn a_recording_is_refused_rather_than_wrapping_the_size_a_wav_can_name() {
        let root = temp("too-long");
        let path = root.join("long.wav");
        let mut writer = Writer::create(&path, Format::Wav16, 48_000.0, 2).expect("should open");

        // Nothing is actually written: the count is moved to just short of the
        // ceiling, which is the state six hours of playing would reach.
        writer.data_bytes = writer.capacity() - 2;
        writer.write(&[0.0]).expect("the last sample that fits");
        let err = writer.write(&[0.0]).expect_err("one past it should be refused");
        assert!(err.to_string().contains("saved as it stands"), "got {err}");

        // And the file it refused to add to is still closeable.
        writer.finish().expect("should close");
        std::fs::remove_dir_all(&root).ok();
    }
}
