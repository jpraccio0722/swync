//! Recording a performance: what comes out of the speakers, into a file.
//!
//! The tap is at the very end of the signal path — after the graph, after the
//! sequencer, after the master gain — so what is recorded is what was heard,
//! including every eval made during the take. There is no offline render here
//! and there could not be: a performance is played, and half of what it is is
//! *when* things were changed.
//!
//! That puts the recorder on the audio callback's side of the fence, and the
//! callback has a deadline. It may not allocate, it may not lock, and it may
//! not touch a disk — the crackle that a missed deadline becomes is exactly
//! what the block-rendering path in `engine.rs` exists to avoid, and a recorder
//! that reintroduced it would make every recording a recording of itself going
//! wrong. So the two halves are split:
//!
//! - The [`Tap`] is a ring of samples the callback writes into. It is allocated
//!   once at startup, sized in seconds of audio, and the callback's whole part
//!   in a recording is copying a block into it and moving one atomic index.
//! - A writer thread drains the ring every 25 ms and writes to the file. It is
//!   allowed to block, because nothing is waiting on it.
//!
//! If the writer thread ever falls four seconds behind — a disk stalling, the
//! machine swapping — the callback drops the block rather than waiting, counts
//! it, and the count is reported when the recording stops. A gap in a file is
//! bad; a gap in the *output*, which is what waiting would cause, is worse and
//! is heard by everyone in the room.

pub mod wav;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fundsp::buffer::BufferRef;

pub use wav::Format;

/// The engine's output is stereo, and so is every recording of it.
const CHANNELS: u16 = 2;

/// How much audio the tap holds before the writer thread has to have taken it.
///
/// Four seconds is far more than a 25 ms drain needs; it is sized for the
/// stall rather than the average — a filesystem that pauses for a second under
/// load costs the recording nothing, where a buffer sized to the mean would
/// lose a second of the take.
const RING_SECONDS: f64 = 4.0;

/// How often the writer thread wakes to drain the tap. The scheduler's own
/// pass runs at the same rate, for the same reason: it is short against
/// anything a person notices and long against the cost of waking up.
const DRAIN_INTERVAL: Duration = Duration::from_millis(25);

/// How often the file's header is brought up to date. See
/// [`wav::Writer::checkpoint`] — it is what makes a recording that ends in a
/// crash still playable.
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(1);

/// The audio callback's end of a recording: a fixed ring of samples, and the
/// two indices that say what is in it.
///
/// Samples are held as `AtomicU32` bit patterns rather than behind a lock or a
/// pointer the compiler has to be promised things about. The stores are
/// relaxed and cost nothing measurable — a hundred thousand a second against
/// the millions of floating-point operations the graph is already doing — and
/// what they buy is a shared buffer with no `unsafe` in it at all.
///
/// The ordering is the ordinary single-producer, single-consumer one: the
/// callback publishes samples by releasing `written`, the writer thread
/// acquires it before reading them, and the same pair the other way round for
/// `taken`. Neither side ever waits for the other.
pub struct Tap {
    /// Interleaved samples, as `f32::to_bits`. Its length is the ring's
    /// capacity and never changes.
    slots: Vec<AtomicU32>,
    /// Samples ever written, counted rather than wrapped — the position in the
    /// ring is this modulo its length, and a monotonic count cannot be
    /// mistaken for an empty ring the way a wrapped pair of indices can.
    written: AtomicU64,
    /// Samples ever drained, by the same counting.
    taken: AtomicU64,
    /// Whether the callback should be filling the ring at all. This is also
    /// how a recording is stopped: the writer thread watches it.
    recording: AtomicBool,
    /// Frames the callback had nowhere to put. Reported when the recording
    /// stops, because a file with a gap in it should say so.
    dropped: AtomicU64,
    /// The rate the device opened at, which is the rate the file is written
    /// at: a recording is what was rendered, not a resampling of it.
    sample_rate: f64,
}

impl Tap {
    pub fn new(sample_rate: f64) -> Tap {
        let frames = (sample_rate * RING_SECONDS).round() as usize;
        Tap {
            slots: (0..frames * CHANNELS as usize).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            taken: AtomicU64::new(0),
            recording: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
            sample_rate,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Take one rendered block, from the audio callback.
    ///
    /// Every branch in here is cheap and none of them can block. When nothing
    /// is being recorded this is one relaxed load and a return, which is what
    /// it does for all but a few minutes of the app's life.
    pub fn push(&self, block: &BufferRef, frames: usize) {
        if !self.recording.load(Ordering::Relaxed) {
            return;
        }

        let capacity = self.slots.len() as u64;
        // Only this thread writes `written`, so a relaxed load of it is this
        // thread's own value; `taken` is the other thread's and is acquired.
        let written = self.written.load(Ordering::Relaxed);
        let room = capacity - (written - self.taken.load(Ordering::Acquire));

        let samples = frames as u64 * CHANNELS as u64;
        if room < samples {
            // The writer thread is four seconds behind. Dropping the block
            // keeps the callback inside its deadline, which is the one thing
            // here that cannot be given up: the alternative is heard live.
            self.dropped.fetch_add(frames as u64, Ordering::Relaxed);
            return;
        }

        for i in 0..frames {
            let at = (written + i as u64 * CHANNELS as u64) % capacity;
            self.slots[at as usize].store(block.at_f32(0, i).to_bits(), Ordering::Relaxed);
            self.slots[((at + 1) % capacity) as usize]
                .store(block.at_f32(1, i).to_bits(), Ordering::Relaxed);
        }

        // The samples are only visible to the writer thread once this lands,
        // which is why it is the last thing done and why it is a release.
        self.written.store(written + samples, Ordering::Release);
    }

    /// Start filling the ring, from empty.
    ///
    /// Only ever called with no writer thread running and nothing being
    /// pushed, so resetting the indices races with nobody.
    pub(crate) fn arm(&self) {
        self.written.store(0, Ordering::Relaxed);
        self.taken.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        self.recording.store(true, Ordering::Release);
    }

    /// Stop filling it. The writer thread reads this too, and finishes the
    /// file when it sees it.
    pub(crate) fn disarm(&self) {
        self.recording.store(false, Ordering::Release);
    }

    pub(crate) fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Acquire)
    }

    /// Move everything the callback has published into `into`, from the writer
    /// thread. Returns how many samples that was.
    pub(crate) fn drain(&self, into: &mut Vec<f32>) -> usize {
        let taken = self.taken.load(Ordering::Relaxed);
        let written = self.written.load(Ordering::Acquire);
        let capacity = self.slots.len() as u64;

        for n in taken..written {
            let bits = self.slots[(n % capacity) as usize].load(Ordering::Relaxed);
            into.push(f32::from_bits(bits));
        }

        self.taken.store(written, Ordering::Release);
        (written - taken) as usize
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// What a finished recording was.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Finished {
    /// Where it was written.
    pub path: String,
    /// How long it is.
    pub seconds: f64,
    /// Frames the writer thread could not keep up with, which is a gap in the
    /// file. Zero on any machine that was keeping up, which is all of them
    /// unless something went badly wrong.
    pub dropped: u64,
}

/// What the editor shows while a recording is running — and how it learns that
/// one has stopped without being asked to.
#[derive(Clone, Debug, serde::Serialize)]
pub struct RecordingState {
    pub recording: bool,
    /// The file being written, while one is.
    pub path: Option<String>,
    /// How long the take has been running.
    pub seconds: f64,
    pub dropped: u64,
    /// Why it stopped on its own, when it did. A disk filling up is the
    /// obvious way: nothing asked for it, so there is no call for it to be the
    /// answer to, and this is where it waits to be collected.
    pub failure: Option<String>,
}

impl RecordingState {
    fn idle() -> RecordingState {
        RecordingState {
            recording: false,
            path: None,
            seconds: 0.0,
            dropped: 0,
            failure: None,
        }
    }
}

/// A recording in progress.
struct Session {
    path: PathBuf,
    /// When it started, which is what the elapsed time is counted from. Wall
    /// time rather than frames written: it is the take's length as the person
    /// playing it experiences it, and the two only differ if frames were
    /// dropped — which is reported separately rather than folded into a clock.
    started: Instant,
    thread: JoinHandle<Result<Finished, String>>,
}

/// The recorder, as the commands see it.
pub struct Recorder {
    tap: Arc<Tap>,
    /// The take that is running, if any. A `Mutex` and not an atomic because
    /// starting one is several things at once — opening a file, arming the
    /// tap, spawning the thread — and two clicks arriving together must not
    /// interleave them.
    active: Mutex<Option<Session>>,
}

impl Recorder {
    pub fn new(tap: Arc<Tap>) -> Recorder {
        Recorder { tap, active: Mutex::new(None) }
    }

    /// Begin recording to `path`, which must not exist — see [`unique_path`],
    /// which is where the name comes from.
    ///
    /// The file is opened here rather than on the writer thread so that a
    /// folder that cannot be written to is an answer to the click that asked,
    /// rather than a failure arriving a moment later from nowhere.
    pub fn start(&self, path: &Path, format: Format) -> Result<String, String> {
        let mut active = self.active.lock().map_err(|_| "the recorder is poisoned")?;
        if let Some(session) = active.as_ref() {
            if !session.thread.is_finished() {
                return Err(format!(
                    "already recording to {}",
                    session.path.display()
                ));
            }
            // One that stopped on its own. Whatever went wrong has been
            // reported by `state` by now, or is about to be lost — either way
            // it must not stand in the way of starting another take.
            *active = None;
        }

        let writer = wav::Writer::create(path, format, self.tap.sample_rate(), CHANNELS)
            .map_err(|e| format!("could not start recording to {}: {e}", path.display()))?;

        // After the file is open: arming the tap fills a ring nobody would be
        // draining if the line above had failed.
        self.tap.arm();

        let tap = self.tap.clone();
        let where_to = path.to_path_buf();
        let thread = std::thread::spawn(move || write_until_stopped(tap, writer, where_to));

        *active = Some(Session {
            path: path.to_path_buf(),
            started: Instant::now(),
            thread,
        });
        Ok(path.display().to_string())
    }

    /// Stop, and wait for the file to be closed.
    ///
    /// Waiting is the point: the recording is not a recording until its header
    /// has been filled in, and the editor is about to tell somebody where the
    /// file is. It costs a drain interval or two.
    pub fn stop(&self) -> Result<Finished, String> {
        let mut active = self.active.lock().map_err(|_| "the recorder is poisoned")?;
        let session = active.take().ok_or("nothing is being recorded")?;

        self.tap.disarm();
        session
            .thread
            .join()
            .map_err(|_| "the recording thread failed".to_string())?
    }

    /// Where the recording has got to — and, if it has stopped by itself,
    /// why.
    ///
    /// This is the only route a failure on the writer thread has back to the
    /// editor, which is why it collects the finished thread rather than only
    /// reading a flag: nothing asked the disk to fill up, so there is no
    /// command's answer for it to travel in.
    pub fn state(&self) -> RecordingState {
        let mut active = match self.active.lock() {
            Ok(active) => active,
            Err(_) => return RecordingState::idle(),
        };

        let Some(session) = active.as_ref() else {
            return RecordingState::idle();
        };

        if !session.thread.is_finished() {
            return RecordingState {
                recording: true,
                path: Some(session.path.display().to_string()),
                seconds: session.started.elapsed().as_secs_f64(),
                dropped: self.tap.dropped(),
                failure: None,
            };
        }

        // It ended without being asked to, which only happens when writing
        // failed. Collect it, so the next take can start.
        let session = active.take().expect("checked just above");
        let failure = match session.thread.join() {
            Ok(Err(e)) => Some(e),
            Err(_) => Some("the recording thread failed".to_string()),
            // A clean finish nobody asked for should not be reachable — the
            // thread only returns `Ok` on `disarm`, and `disarm` is `stop`,
            // which takes the session before joining. Reported rather than
            // asserted: a recording is not worth a panic.
            Ok(Ok(_)) => None,
        };

        RecordingState {
            recording: false,
            path: Some(session.path.display().to_string()),
            seconds: session.started.elapsed().as_secs_f64(),
            dropped: self.tap.dropped(),
            failure,
        }
    }
}

/// The writer thread: drain, write, repeat, until the tap says stop.
fn write_until_stopped(
    tap: Arc<Tap>,
    mut writer: wav::Writer,
    path: PathBuf,
) -> Result<Finished, String> {
    let mut buffer: Vec<f32> = Vec::new();
    let mut since_checkpoint = Instant::now();

    let fail = |e: std::io::Error| format!("recording to {} failed: {e}", path.display());

    while tap.is_recording() {
        std::thread::sleep(DRAIN_INTERVAL);

        buffer.clear();
        tap.drain(&mut buffer);
        if !buffer.is_empty() {
            writer.write(&buffer).map_err(fail)?;
        }

        if since_checkpoint.elapsed() >= CHECKPOINT_INTERVAL {
            writer.checkpoint().map_err(fail)?;
            since_checkpoint = Instant::now();
        }
    }

    // One more pass after the stop. The callback may have been half-way
    // through a block when the flag went down — it finishes that block, and
    // this is the sleep that lets it land before the file is closed.
    std::thread::sleep(DRAIN_INTERVAL);
    buffer.clear();
    tap.drain(&mut buffer);
    if !buffer.is_empty() {
        writer.write(&buffer).map_err(fail)?;
    }

    // The length of the file, which is what was recorded — not how long the
    // take ran, which is what the editor's clock was showing.
    let seconds = writer.frames() as f64 / tap.sample_rate();
    writer.finish().map_err(fail)?;

    Ok(Finished {
        path: path.display().to_string(),
        seconds,
        dropped: tap.dropped(),
    })
}

/// What a recording is called: the piece, then when it was played.
///
/// A timestamp rather than a take number because the file is a record of a
/// performance, and the question asked of it later is "which evening was
/// that" — and because a number would have to be found by counting what is
/// already in the folder, which is a different answer depending on what else
/// has been moved in or out of it.
///
/// A collision is still possible — two takes inside one second, or a file that
/// happens to be named this already — and is never resolved by overwriting.
/// That is the rule the rest of the app keeps around files, and a recording is
/// the last thing that should break it: what would be lost is a performance
/// that cannot be played again.
pub fn unique_path(dir: &Path, stem: &str, format: Format) -> Result<PathBuf, String> {
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
    let base = format!("{} {stamp}", sanitized(stem));
    let extension = format.extension();

    for n in 1..1000 {
        let name = if n == 1 {
            format!("{base}.{extension}")
        } else {
            format!("{base} ({n}).{extension}")
        };
        let path = dir.join(name);
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(format!(
        "there are already a thousand recordings called '{base}' in {} — \
         nothing has been overwritten, but something has gone wrong",
        dir.display()
    ))
}

/// A project's name, made safe to be part of a file's.
///
/// A project is named by whoever opened it and may say anything at all,
/// including things a filesystem reads as structure. Everything a path could
/// take for a separator becomes a dash, and what is left of a name that was
/// nothing but those is the fallback.
fn sanitized(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "recording".to_string()
    } else {
        cleaned
    }
}

/// One entry in the list of formats the editor's settings offer.
#[derive(serde::Serialize)]
pub struct FormatInfo {
    /// What `Settings` and the dropdown call it, which is the serialized name
    /// of the [`Format`] itself.
    pub id: Format,
    pub label: String,
    pub extension: String,
    /// The one line under the name in the settings panel.
    pub detail: String,
}

/// Every format a recording can be written as.
///
/// Served to the frontend rather than written out again in TypeScript, for the
/// same reason the language's builtins are: a dropdown offering a format the
/// recorder cannot write is a promise the app breaks after the take, which is
/// the worst possible moment to find out.
pub fn formats() -> Vec<FormatInfo> {
    [
        (
            Format::Wav16,
            "WAV — 16-bit",
            "What every editor, player and phone can read. Anything the master \
             pushed above unity is clipped off.",
        ),
        (
            Format::Wav32,
            "WAV — 32-bit float",
            "Exactly what the engine rendered, headroom and all. Twice the size, \
             and what to record if the take is going to be mixed.",
        ),
    ]
    .into_iter()
    .map(|(id, label, detail)| FormatInfo {
        id,
        label: label.to_string(),
        extension: id.extension().to_string(),
        detail: detail.to_string(),
    })
    .collect()
}

#[cfg(test)]
mod tests;
