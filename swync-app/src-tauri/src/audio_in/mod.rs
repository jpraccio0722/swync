//! Live audio in: what a microphone or an interface is sending, as something
//! a program can name.
//!
//! The shape here is the recorder's, run backwards. A recording is the audio
//! callback handing samples to a thread that may block; an input is a callback
//! that may block handing samples to the audio callback. Both meet in a fixed
//! ring of `AtomicU32` allocated once at startup, and neither side ever waits
//! for the other — see [`crate::recorder`], which explains the ordering, since
//! it is the same single-producer, single-consumer pair either way round.
//!
//! What is different, and what most of this file is about, is that the two
//! ends are two *devices*. An input stream and an output stream have separate
//! clocks even when they agree on a rate, so the writer and the reader drift:
//! over a minute one of them delivers a few frames more than the other asked
//! for. There is no arrangement of buffers that makes that not happen, so the
//! ring is written to absorb it instead —
//!
//! - The reader keeps a **lead**: it renders silence until the ring holds a
//!   callback's worth from each side, and only then starts reading. Reading
//!   the instant a sample arrives would underrun on the very next block, since
//!   the two callbacks are not in step and never will be.
//! - Falling behind is **silence, and it primes again**. The alternative is
//!   waiting on the audio thread, which is a gap in the output rather than in
//!   the input, and everyone in the room hears that one.
//! - Getting ahead is **thrown away**. Latency that is allowed to grow is a
//!   monitor path that drifts further behind the room all evening, which is
//!   worse than a discontinuity nobody can point at.
//!
//! Both are counted and reported, because a machine that does either one
//! constantly is a machine whose buffer sizes want looking at, and nothing
//! else in the app would say so.
//!
//! ## Why the graph reads a block rather than the ring
//!
//! `input(0)` may be written twice in a program, and both must hear the same
//! thing. A node that pulled from the ring itself would consume a frame the
//! other one then could not have. So the ring is drained exactly once per
//! rendered block, by the audio callback, into a block every [`InputNode`]
//! reads by index — which is also why a voice in the sequencer hears the same
//! input as the persistent graph, at the same moment.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

use cpal::traits::DeviceTrait;
use cpal::{FromSample, SizedSample};
use fundsp::prelude64::*;

use crate::devices::{self, DeviceInfo};
use crate::engine::MAX_SAMPLE_RATE;
use crate::meter::Peaks;

#[cfg(test)]
mod tests;

/// How many channels of a device are reachable, and so the ring's stride.
///
/// The stride is fixed rather than the open device's channel count because the
/// ring is allocated before any device is chosen and outlives every one that
/// is: a stride that changed with the device would mean re-laying-out a buffer
/// the audio callback is holding a pointer into. Sixteen covers every
/// interface anyone plugs into a laptop, and the memory it costs is a
/// megabyte — cheaper than telling somebody with an eight-in interface that
/// only two of them can be named.
pub const MAX_CHANNELS: usize = 16;

/// How much audio the ring holds. Sized for the stall rather than the average,
/// exactly as the recorder's is: the lead the reader actually keeps is a
/// fortieth of this, and the rest is room for a callback that was late.
const RING_SECONDS: f64 = 0.1;

/// The ring's length in frames. At the fastest rate a device can open at, this
/// is [`RING_SECONDS`]; at the rate most of them do open at, four times that.
const RING_FRAMES: usize = (MAX_SAMPLE_RATE * RING_SECONDS) as usize;

/// The smallest lead the reader will keep, in frames, before either side has
/// said how big its buffers are. One fundsp block: enough that the first
/// rendered block after a device opens is not immediately an underrun.
const MIN_LEAD: usize = MAX_BUFFER_SIZE;


/// The ring, the block the graph reads, and the counters that say how the two
/// devices are getting on.
///
/// One of these exists per process — see [`bus`] — because there is one audio
/// input, and because a graph node has to be able to find it from
/// [`crate::swync_graph::realizer::realize`], which is a pure function of the
/// IR and is called from three threads.
pub struct AudioIn {
    /// Interleaved by [`MAX_CHANNELS`] whatever the device has, as
    /// `f32::to_bits`. Its length never changes.
    slots: Vec<AtomicU32>,
    /// Frames ever written, counted rather than wrapped. The position in the
    /// ring is this modulo its length.
    written: AtomicU64,
    /// Frames ever read, by the same counting.
    taken: AtomicU64,
    /// How many channels the open device has, and zero when none is open —
    /// which is what makes `input(0)` silence rather than a failure on a
    /// machine that has never chosen a device.
    channels: AtomicUsize,
    /// Frames per callback, as each side last saw. The lead is derived from
    /// them rather than fixed, because a device opened with 2048-frame buffers
    /// needs eight times the lead of one opened with 256 and would otherwise
    /// underrun on every block for ever.
    in_block: AtomicUsize,
    out_block: AtomicUsize,
    /// Whether the reader is still waiting for the ring to reach its lead.
    /// Only the reader touches it.
    priming: AtomicBool,
    /// Frames the reader had to invent because they had not arrived.
    late: AtomicU64,
    /// Frames thrown away — by the writer when the ring was full, and by the
    /// reader when the lead had grown past what it is for.
    dropped: AtomicU64,
    /// What the title bar's `in` meter draws. Written by the input callback
    /// once per block, not once per sample.
    peaks: Peaks,
    /// The frames the graph is reading right now, filled by [`AudioIn::take`]
    /// before each block is rendered. Same stride as the ring.
    block: Vec<AtomicU32>,
}

impl AudioIn {
    fn new() -> AudioIn {
        AudioIn {
            slots: (0..RING_FRAMES * MAX_CHANNELS).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            taken: AtomicU64::new(0),
            channels: AtomicUsize::new(0),
            in_block: AtomicUsize::new(0),
            out_block: AtomicUsize::new(0),
            priming: AtomicBool::new(true),
            late: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            peaks: Peaks::new(MAX_CHANNELS),
            block: (0..MAX_BUFFER_SIZE * MAX_CHANNELS).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    /// How many channels are arriving. Zero when no device is open.
    pub fn channels(&self) -> usize {
        self.channels.load(Ordering::Acquire)
    }

    /// A device has opened. Called with no stream running, so resetting the
    /// indices races with nobody.
    ///
    /// Reachable from the crate's tests rather than only from this module: the
    /// claim worth pinning is that a *realized graph* carries the input, and
    /// the realizer is where that test belongs.
    pub(crate) fn opened(&self, channels: usize) {
        self.written.store(0, Ordering::Relaxed);
        self.taken.store(0, Ordering::Relaxed);
        self.late.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        self.priming.store(true, Ordering::Relaxed);
        self.hush();
        self.channels.store(std::cmp::min(channels, MAX_CHANNELS), Ordering::Release);
    }

    /// It has closed, or is about to. The block is cleared as well as the
    /// count: a graph still holding `input(1)` must go quiet rather than hold
    /// the last frame it was handed, which is a DC offset for as long as the
    /// program runs.
    pub(crate) fn closed(&self) {
        self.channels.store(0, Ordering::Release);
        self.hush();
        self.peaks.clear();
    }

    /// Zero the whole block, every channel of it.
    fn hush(&self) {
        for slot in &self.block {
            slot.store(0, Ordering::Relaxed);
        }
    }

    /// How far ahead of itself the reader tries to stay: one callback from
    /// each device, which is the most either can be late by without something
    /// having actually gone wrong.
    fn lead(&self) -> u64 {
        let both = self.in_block.load(Ordering::Relaxed) + self.out_block.load(Ordering::Relaxed);
        std::cmp::max(both, MIN_LEAD) as u64
    }

    /// Take one callback's worth of input, from the input callback.
    ///
    /// Everything here is bounded and none of it can block or allocate. The
    /// peak is folded into the copy rather than walked separately, so the
    /// meter costs one comparison per sample and one atomic per channel.
    pub fn push<T>(&self, data: &[T], channels: usize)
    where
        f32: FromSample<T>,
        T: SizedSample,
    {
        let live = self.channels();
        if live == 0 || channels == 0 {
            return;
        }

        let frames = data.len() / channels;
        self.in_block.store(frames, Ordering::Relaxed);

        let written = self.written.load(Ordering::Relaxed);
        let room = RING_FRAMES as u64 - (written - self.taken.load(Ordering::Acquire));
        if room < frames as u64 {
            // The reader has stopped taking — an output device being switched
            // underneath us, or one that has stalled. Dropping is the same
            // choice the recorder makes and for the same reason: this callback
            // has a deadline too, and the samples are worth less than it.
            self.dropped.fetch_add(frames as u64, Ordering::Relaxed);
            return;
        }

        // Channels past what the ring can hold are not read at all. Nothing
        // can name them, so copying them would be work for nobody.
        let copied = std::cmp::min(live, channels);
        let mut peaks = [0.0f32; MAX_CHANNELS];
        for i in 0..frames {
            let at = ((written + i as u64) % RING_FRAMES as u64) as usize * MAX_CHANNELS;
            for c in 0..copied {
                // Spelled out rather than `f32::from_sample`, which is the
                // same conversion reached through a `Sample` trait that
                // fundsp's prelude has a name of its own for.
                let sample = <f32 as FromSample<T>>::from_sample_(data[i * channels + c]);
                self.slots[at + c].store(sample.to_bits(), Ordering::Relaxed);
                peaks[c] = f32::max(peaks[c], sample.abs());
            }
        }

        for c in 0..copied {
            self.peaks.observe(c, peaks[c]);
        }

        // The frames become visible to the reader here, and not before.
        self.written.store(written + frames as u64, Ordering::Release);
    }

    /// Fill the block the graph is about to read, from the audio callback.
    ///
    /// Called once per rendered block, before the graph runs, so that every
    /// `input()` in the program — and every voice the sequencer is holding
    /// open — reads the same frames.
    pub fn take(&self, frames: usize) {
        let frames = std::cmp::min(frames, MAX_BUFFER_SIZE);
        let live = self.channels();
        if live == 0 {
            // Nothing is open. The block was cleared when the last device
            // closed and nothing has written to it since, so this is already
            // silence and there is nothing to do — which is the state the app
            // is in for all of its life on a machine that never uses input.
            return;
        }
        self.out_block.store(frames, Ordering::Relaxed);

        let taken = self.taken.load(Ordering::Relaxed);
        let written = self.written.load(Ordering::Acquire);
        let available = written - taken;
        let lead = self.lead();

        if self.priming.load(Ordering::Relaxed) {
            if available < lead {
                self.silence(0, frames, live);
                return;
            }
            self.priming.store(false, Ordering::Relaxed);
        }

        if available < frames as u64 {
            // Under. Play what did arrive, then silence, and start filling
            // again — a lead that has been spent has to be earned back or the
            // next block is short too.
            let short = available as usize;
            self.copy(taken, short, live);
            self.silence(short, frames, live);
            self.taken.store(written, Ordering::Release);
            self.late.fetch_add(frames as u64 - available, Ordering::Relaxed);
            self.priming.store(true, Ordering::Relaxed);
            return;
        }

        // Over. Twice the lead rather than the lead itself, so ordinary
        // jitter — which crosses the mark constantly — does not throw a frame
        // away every block. What this catches is drift, and a writer catching
        // up after a stall.
        let want = lead + frames as u64;
        let mut taken = taken;
        if available > want * 2 {
            let skip = available - want;
            taken += skip;
            self.dropped.fetch_add(skip, Ordering::Relaxed);
        }

        self.copy(taken, frames, live);
        self.taken.store(taken + frames as u64, Ordering::Release);
    }

    /// Ring to block, `frames` frames from `from`.
    fn copy(&self, from: u64, frames: usize, live: usize) {
        for i in 0..frames {
            let at = ((from + i as u64) % RING_FRAMES as u64) as usize * MAX_CHANNELS;
            let to = i * MAX_CHANNELS;
            for c in 0..live {
                self.block[to + c].store(self.slots[at + c].load(Ordering::Relaxed), Ordering::Relaxed);
            }
        }
    }

    /// Silence in the block, from one frame up to another. Only the live
    /// channels: the rest were cleared when the device opened and nothing
    /// writes to them.
    fn silence(&self, from: usize, to: usize, live: usize) {
        for i in from..to {
            for c in 0..live {
                self.block[i * MAX_CHANNELS + c].store(0, Ordering::Relaxed);
            }
        }
    }

    /// One channel of one frame of the block, for a graph node.
    ///
    /// A channel the open device does not have is silence rather than a
    /// refusal: which channels exist is a fact about what is plugged in this
    /// evening, and a program should not stop compiling because an interface
    /// was swapped for a smaller one.
    #[inline]
    pub fn at(&self, channel: usize, i: usize) -> f32 {
        if channel >= MAX_CHANNELS || i >= MAX_BUFFER_SIZE {
            return 0.0;
        }
        f32::from_bits(self.block[i * MAX_CHANNELS + channel].load(Ordering::Relaxed))
    }

    /// What the `in` meter draws: a peak per live channel since it last
    /// looked. A device with two channels open reports two, not sixteen.
    pub fn levels(&self) -> Vec<f32> {
        self.peaks.take_channels(self.channels())
    }

    /// Frames the reader had to invent, and frames thrown away. Both are zero
    /// on a machine whose two devices are keeping up with each other.
    pub fn slippage(&self) -> (u64, u64) {
        (self.late.load(Ordering::Relaxed), self.dropped.load(Ordering::Relaxed))
    }
}

/// The one input bus in the process.
///
/// A `OnceLock` rather than something threaded through the realizer, because
/// what would be threaded is the same value every time: there is one audio
/// input, `realize` is a pure function of the IR called from three threads and
/// a dozen tests, and giving all of them a parameter to carry would say
/// nothing except that this is shared. Built on first use, so a test that
/// realizes a graph with an `input` in it gets a bus with no device open —
/// which is silence, and is exactly what that test should hear.
pub fn bus() -> Arc<AudioIn> {
    static BUS: OnceLock<Arc<AudioIn>> = OnceLock::new();
    BUS.get_or_init(|| Arc::new(AudioIn::new())).clone()
}

/// Hold the process's one bus for the duration of a test.
///
/// [`bus`] is a singleton — deliberately, since there is one audio input — and
/// the suite runs in threads. Two tests that both had opinions about whether a
/// device was open would each be right half the time. Every test that opens or
/// reads the shared bus takes this first; tests with a bus of their own, which
/// is most of them, need nothing.
#[cfg(test)]
pub(crate) fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// One channel of the live input, as a graph node.
///
/// - No inputs.
/// - Output 0: what arrived on that channel this block.
///
/// Stateless, like [`crate::swync_graph::sample_reader::SampleReader`] and for
/// the same reason: the scheduler builds one of these per note, on its own
/// thread, and there is nothing here to reset or to get out of step.
#[derive(Clone)]
pub struct InputNode {
    bus: Arc<AudioIn>,
    channel: usize,
}

impl InputNode {
    pub fn new(channel: usize) -> InputNode {
        InputNode { bus: bus(), channel }
    }
}

impl AudioNode for InputNode {
    // Chosen from the far end of fundsp's own range, beside `SampleReader`.
    const ID: u64 = 201;
    type Inputs = typenum::U0;
    type Outputs = typenum::U1;

    #[inline]
    fn tick(&mut self, _input: &Frame<f32, Self::Inputs>) -> Frame<f32, Self::Outputs> {
        [self.bus.at(self.channel, 0)].into()
    }

    /// The block path, which is the one the engine actually uses. Frame `i`
    /// here is frame `i` of the block the callback just drained, so two
    /// `input(0)`s in one program are sample-for-sample the same signal.
    fn process(&mut self, size: usize, _input: &BufferRef, output: &mut BufferMut) {
        for i in 0..size {
            output.set_f32(0, i, self.bus.at(self.channel, i));
        }
    }

    fn route(&mut self, input: &SignalFrame, _frequency: f64) -> SignalFrame {
        // Whatever is coming down the wire, which is unrelated to anything in
        // the graph — a generator, like `noise`.
        Routing::Generator(0.0).route(input, self.outputs())
    }
}

/// How the input is getting on, beside what it is: the levels the title bar's
/// meter draws, and the two counts that say whether the devices are keeping
/// step with each other.
///
/// Answered by one command polled ten times a second, so it is deliberately
/// small — what is *open* is `audio_devices`, which changes when somebody
/// changes it rather than continuously.
#[derive(Clone, Debug, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InputLevels {
    /// Peak per live channel since the last ask, and empty when input is off.
    /// Its length is how many channels the open device has.
    pub levels: Vec<f32>,
    /// Frames the input arrived too late to be heard, and frames thrown away
    /// to stop the delay growing. Both stay at zero on a machine that is
    /// keeping up; either one climbing is a buffer size worth looking at.
    pub late: u64,
    pub dropped: u64,
}

/// What the input thread is asked to do. Both carry a reply, because both can
/// fail in ways the person who asked is waiting to be told about — a device
/// that has been unplugged, one another app has taken exclusively, or a
/// microphone permission that was refused.
enum Command {
    /// `reply` is optional because startup does not wait to be told — see
    /// [`Input::request`].
    Open { device: String, sample_rate: u32, reply: Option<Sender<Result<Opened, String>>> },
    Close { reply: Sender<()> },
}

/// A device that is now open.
#[derive(Clone, Debug, PartialEq)]
pub struct Opened {
    device: DeviceInfo,
    channels: usize,
    sample_rate: f64,
}


/// The input side of the app, as everything outside this module sees it.
///
/// The stream itself lives on its own thread and never leaves it: a
/// `cpal::Stream` is not `Send`, so the thread that built it is the only one
/// that may drop it, and closing a device is dropping it. Every method here is
/// therefore a message, and — where there is any point waiting — a bounded
/// wait for the answer.
///
/// What is open is written by that thread rather than by whoever asked, which
/// is what makes the bounded wait honest: a caller that gives up waiting has
/// given up on the *answer*, not on the device, and [`status`](Self::status)
/// still tells the truth about it a moment later.
pub struct Input {
    commands: Sender<Command>,
    open: Arc<Mutex<Option<Opened>>>,
    bus: Arc<AudioIn>,
}

impl Input {
    /// Spawn the input thread. Call once, at startup — nothing is opened here,
    /// and nothing is until a device is chosen.
    pub fn start() -> Input {
        let (commands, requests) = channel();
        let open = Arc::new(Mutex::new(None));
        let thread = open.clone();
        std::thread::spawn(move || run(requests, thread));
        Input { commands, open, bus: bus() }
    }

    /// Listen to a device, by the id it is remembered under, at a rate.
    ///
    /// The rate is the output stream's, not a preference: the two streams feed
    /// one graph, and a graph cannot be rendered at two rates at once. A
    /// device that cannot run at it is refused, and said so, rather than
    /// opened and quietly resampled — which would be a pitch error with
    /// nothing anywhere to point at its cause.
    pub fn open(&self, device: &str, sample_rate: f64) -> Result<(), String> {
        let (reply, answer) = channel();
        self.request(device, sample_rate, Some(reply))?;

        match answer.recv_timeout(devices::ANSWER_TIMEOUT) {
            Ok(opened) => opened.map(|_| ()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(
                "that audio input has not started yet. It is still being opened, and the \
                 meter will show it if it comes up — but if it is waiting on a microphone \
                 permission, granting that and choosing the device again is what is left \
                 to do."
                    .to_string(),
            ),
            Err(_) => Err("the audio input thread has gone".to_string()),
        }
    }

    /// Ask for a device and do not wait to hear how it went.
    ///
    /// What startup uses. Waiting there would put however long a device takes
    /// to refuse in front of the window appearing, and a remembered input is
    /// the one thing here nobody has asked for yet — the panel says what is
    /// listening once there is a panel to say it in.
    pub fn request(
        &self,
        device: &str,
        sample_rate: f64,
        reply: Option<Sender<Result<Opened, String>>>,
    ) -> Result<(), String> {
        self.commands
            .send(Command::Open {
                device: device.to_string(),
                sample_rate: sample_rate.round() as u32,
                reply,
            })
            .map_err(|_| "the audio input thread has gone".to_string())
    }

    /// Stop listening. Silence for anything still naming `input`, and the
    /// device is handed back to whatever else on the machine wants it.
    pub fn close(&self) -> Result<(), String> {
        let (reply, answer) = channel();
        self.commands
            .send(Command::Close { reply })
            .map_err(|_| "the audio input thread has gone".to_string())?;
        // Dropping a stream is a device being stopped, and a device that is
        // wedged can be as slow to stop as it was to start.
        match answer.recv_timeout(devices::ANSWER_TIMEOUT) {
            Ok(()) => Ok(()),
            // Either the device itself is slow to stop, or — the case that
            // actually happens — the thread is still inside a previous open
            // that the operating system has not come back from, and this is
            // queued behind it. Both resolve on their own; neither is worth
            // holding the caller for.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(
                "the audio input has not let go of its device yet — it will as soon as \
                 the last thing asked of it finishes."
                    .to_string(),
            ),
            Err(_) => Err("the audio input thread has gone".to_string()),
        }
    }

    /// The device being listened to, if any.
    pub fn device(&self) -> Option<DeviceInfo> {
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|open| open.device.clone())
    }

    /// Reopen whatever is open at a new rate, after the output device has
    /// moved to one. Nothing to do when input is off, which is the usual case.
    ///
    /// A failure here leaves input closed and says why: the alternative is a
    /// device left running at a rate the graph is no longer rendering at,
    /// which is an input transposed by the ratio between the two.
    pub fn follow_rate(&self, sample_rate: f64) -> Result<(), String> {
        let Some(device) = self.device() else {
            return Ok(());
        };
        self.close()?;
        self.open(&device.id, sample_rate)
    }

    pub fn levels(&self) -> InputLevels {
        let (late, dropped) = self.bus.slippage();
        InputLevels { levels: self.bus.levels(), late, dropped }
    }
}

/// The input thread. Owns the stream, and is the only thread that ever does.
///
/// It also owns what `status` reports, and that is the point: a caller that
/// stopped waiting for an answer has not stopped the open, and this is what
/// makes the panel right about it either way.
fn run(requests: Receiver<Command>, open: Arc<Mutex<Option<Opened>>>) {
    let bus = bus();
    // Dropped when it is replaced or closed, which is how a device is let go.
    let mut stream: Option<cpal::Stream> = None;

    for request in requests {
        match request {
            Command::Open { device, sample_rate, reply } => {
                // Closed before the new one is opened, rather than after. Some
                // hosts refuse a second stream on a device that is already
                // open, and reopening the same device is the ordinary case —
                // it is what following the output's rate does.
                drop(stream.take());
                bus.closed();
                *open.lock().unwrap_or_else(|e| e.into_inner()) = None;

                let outcome = locate(&device, sample_rate).and_then(
                    |(device, config, opened)| {
                        // Before the stream exists, so the ring is already
                        // expecting the first callback rather than throwing it
                        // away — and, more to the point, so that no frame can
                        // arrive between the ring being reset and the channel
                        // count that admits frames being published.
                        bus.opened(opened.channels);
                        match listen(&device, &config, bus.clone()) {
                            Ok(new) => {
                                stream = Some(new);
                                Ok(opened)
                            }
                            Err(e) => {
                                bus.closed();
                                Err(e)
                            }
                        }
                    },
                );

                if let Ok(opened) = &outcome {
                    *open.lock().unwrap_or_else(|e| e.into_inner()) = Some(opened.clone());
                }
                if let Some(reply) = reply {
                    let _ = reply.send(outcome);
                } else if let Err(e) = outcome {
                    // Nobody is waiting, so this is the only place it can be
                    // said. The remembered device that could not be opened at
                    // startup is the case, and the panel shows it as well.
                    eprintln!("audio input: {e}");
                }
            }
            Command::Close { reply } => {
                drop(stream.take());
                bus.closed();
                *open.lock().unwrap_or_else(|e| e.into_inner()) = None;
                let _ = reply.send(());
            }
        }
    }
}

/// Find a device by name and settle what opening it would mean, without
/// opening anything. Everything that can refuse for a reason a person can act
/// on — no such device, not at this rate, a format nothing can read — refuses
/// here, while the previous device is already closed and nothing is half-open.
fn locate(
    id: &str,
    sample_rate: u32,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig, Opened), String> {
    let device = devices::input(id)?;
    let info = devices::describe(&device)
        .ok_or_else(|| "that audio input went away while it was being opened".to_string())?;

    let config = input_config(&device, sample_rate, &info.name)?;
    if !matches!(
        config.sample_format(),
        cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
    ) {
        return Err(format!(
            "\"{}\" sends {}, which this cannot read",
            info.name,
            config.sample_format()
        ));
    }

    let opened = Opened {
        device: info,
        channels: std::cmp::min(config.channels() as usize, MAX_CHANNELS),
        sample_rate: sample_rate as f64,
    };
    Ok((device, config, opened))
}

/// A configuration for this device at exactly this rate, or a refusal that
/// says what to do about it.
///
/// The device's own default is preferred when it already runs at the rate,
/// since that is what the rest of the machine is set up for. Otherwise the
/// widest thing it offers that covers the rate — more channels than a program
/// names costs nothing, and fewer cannot be got back.
fn input_config(
    device: &cpal::Device,
    sample_rate: u32,
    name: &str,
) -> Result<cpal::SupportedStreamConfig, String> {
    let default = device
        .default_input_config()
        .map_err(|e| format!("could not read what \"{name}\" supports: {e}"))?;
    if default.sample_rate() == sample_rate {
        return Ok(default);
    }

    device
        .supported_input_configs()
        .map_err(|e| format!("could not read what \"{name}\" supports: {e}"))?
        .filter(|range| {
            range.min_sample_rate() <= sample_rate && sample_rate <= range.max_sample_rate()
        })
        .max_by_key(|range| range.channels())
        .map(|range| range.with_sample_rate(sample_rate))
        .ok_or_else(|| {
            format!(
                "\"{name}\" cannot run at {sample_rate} Hz, which is the rate the output \
                 device opened at. Both streams feed one graph and it can only be rendered \
                 at one rate, so set the two devices to the same rate — in Audio MIDI Setup \
                 on a Mac — or choose an output device that runs at {} Hz.",
                default.sample_rate()
            )
        })
}

/// Build and start the stream. The callback's whole part is one copy into the
/// ring; everything that could have decided anything has decided it by now.
fn listen(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    bus: Arc<AudioIn>,
) -> Result<cpal::Stream, String> {
    let channels = config.channels() as usize;
    let format = config.sample_format();
    let config: cpal::StreamConfig = config.clone().into();
    match format {
        cpal::SampleFormat::I16 => build::<i16>(device, &config, bus, channels),
        cpal::SampleFormat::U16 => build::<u16>(device, &config, bus, channels),
        // Everything else was refused by `locate`, so f32 is the only case
        // left and taking it as the fallback keeps this exhaustive without a
        // branch that could only be reached by a bug.
        _ => build::<f32>(device, &config, bus, channels),
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    bus: Arc<AudioIn>,
    channels: usize,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    use cpal::traits::StreamTrait;

    let stream = device
        .build_input_stream(
            config.clone(),
            move |data: &[T], _| bus.push(data, channels),
            |e| eprintln!("audio input stream error: {e}"),
            Some(devices::START_TIMEOUT),
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

