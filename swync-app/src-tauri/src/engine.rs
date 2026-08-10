use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use fundsp::net::{Net, NodeId};
use fundsp::prelude::*;

use crate::recorder::Tap;
use crate::scheduler::clock::Clock;

/// Unity: the master control only ever attenuates what a program asked for.
pub const DEFAULT_MASTER_VOLUME: f32 = 1.0;

/// How long the master gain takes to reach a new setting. Long enough that
/// dragging a fader is smooth rather than a stairway of clicks, short enough
/// that it still feels immediate.
const MASTER_GLIDE_SECS: f64 = 0.02;

pub struct AudioEngine {
    pub net: Net,     // control-side handle; lives in tauri state
    pub slot: NodeId, // the one node programs get swapped into
    pub clock: Clock,
    /// Master output gain, read per sample by the graph's last node. Setting
    /// it is all a volume control has to do — nothing is rebuilt.
    pub master: Shared,
}

/// A stereo insert that scales what passes through it by a shared control.
///
/// The control is glided rather than read raw: a fader jumping straight from
/// one gain to the next steps the waveform, and a step is a click.
fn master_gain(volume: &Shared) -> Box<dyn AudioUnit> {
    Box::new(
        (pass() | pass())
            * ((var(volume) >> follow(MASTER_GLIDE_SECS))
                | (var(volume) >> follow(MASTER_GLIDE_SECS))),
    )
}

/// The whole signal path, rated for a device: the program slot, the sequencer's
/// backend, the mixer and the master gain.
///
/// Split out from `start` because that one opens a device and spawns a thread,
/// neither of which a test can do — while everything worth pinning about the
/// wiring, the rating included, is here.
fn wire(sample_rate: f64) -> (Net, Sequencer, NodeId, Shared) {
    let mut net = Net::new(0, 2);

    let slot = net.push(Box::new(dc((0.0, 0.0))));      // silence until first eval

    let mut seq = Sequencer::new(0, 2, ReplayMode::None);
    let seq_node = net.push(Box::new(seq.backend()));

    let mixer = net.push(Box::new((pass() | pass()) + (pass() | pass())));

    // Everything that can make a sound is mixed by now, so this is the one
    // place a master gain belongs: after the graph *and* the sequencer.
    let master = Shared::new(DEFAULT_MASTER_VOLUME);
    let master_node = net.push(master_gain(&master));

    net.connect(slot, 0, mixer, 0);
    net.connect(slot, 1, mixer, 1);
    net.connect(seq_node, 0, mixer, 2);
    net.connect(seq_node, 1, mixer, 3);

    net.connect(mixer, 0, master_node, 0);
    net.connect(mixer, 1, master_node, 1);

    net.connect_output(master_node, 0, 0);
    net.connect_output(master_node, 1, 1);

    net.set_sample_rate(sample_rate);

    // The sequencer the scheduler gets is a *separate object* from the backend
    // node sitting in the net, and the line above reaches only the latter.
    // `Sequencer::push` stamps every voice it is handed with its own rate, so
    // without this the frontend keeps fundsp's 44100 default and every pattern
    // voice runs at 44100 while the device renders it at whatever it opened —
    // on a 48 kHz device a semitone and a half sharp, with every envelope and
    // delay inside the voice short by the same ratio. The persistent graph is
    // rated correctly by `Net::crossfade`, which is what makes this audible
    // rather than merely wrong: the graph and the patterns come out in
    // different keys.
    seq.set_sample_rate(sample_rate);

    (net, seq, slot, master)
}

/// Start audio. The `Sequencer` is returned separately because the scheduler
/// thread owns it outright — nothing else touches it, so it needs no mutex.
///
/// The [`Tap`] comes back for the same kind of reason as the sequencer: it is
/// shared with the callback, and the recorder — not the engine — is what
/// arms it. It is made here because this is where the device's sample rate is
/// known, and a recording is written at whatever rate the stream opened at.
pub fn start() -> Result<(AudioEngine, Sequencer, Arc<Tap>), String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no audio output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;

    let sample_rate = config.sample_rate() as f64;
    let (mut net, seq, slot, master) = wire(sample_rate);
    let backend = net.backend();

    let clock = Clock::new(sample_rate);
    let audio_clock = clock.clone();

    let tap = Arc::new(Tap::new(sample_rate));
    let audio_tap = tap.clone();

    std::thread::spawn(move || {
        let result = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                run_stream::<f32>(&device, &config.into(), backend, audio_clock, audio_tap)
            }
            cpal::SampleFormat::I16 => {
                run_stream::<i16>(&device, &config.into(), backend, audio_clock, audio_tap)
            }
            cpal::SampleFormat::U16 => {
                run_stream::<u16>(&device, &config.into(), backend, audio_clock, audio_tap)
            }
            other => Err(format!("unsupported sample format: {other}")),
        };
        if let Err(e) = result {
            eprintln!("audio thread failed: {e}");
        }
    });

    Ok((AudioEngine { net, slot, clock, master }, seq, tap))
}

/// Fill one device buffer from the graph, a block at a time. Returns the frames
/// written, which is what the clock counts.
///
/// The block is the whole point. `process` renders a run of frames per call and
/// vectorizes across it; `get_stereo` renders one frame per call and pays the
/// graph's per-call overhead on every sample of it. Measured on a program
/// holding six delay-tapped voices open, the per-sample path costs 23% of real
/// time against 3% for this one — sevenfold, and it is spent out of the margin
/// between the callback and its deadline. What is missing when that margin runs
/// out is not quiet, it is a gap, and a gap is heard as a crackle.
///
/// fundsp renders at most `MAX_BUFFER_SIZE` frames per call, so a device buffer
/// longer than that takes several passes. The signal has to carry across them
/// unbroken: a pass that re-rendered or skipped a block would be a seam in the
/// waveform every buffer, which is the very thing this is here to avoid.
///
/// A recording is taken from the rendered block, which is the last point at
/// which the signal is still what the graph produced: after this it is
/// converted to whatever the device asked for and spread across however many
/// channels it has. That the tap is *here*, rather than anywhere earlier, is
/// what makes a recording the performance as it was heard — the graph, the
/// sequencer's voices and the master fader all together.
fn render<T>(
    backend: &mut impl AudioUnit,
    block: &mut BufferVec,
    data: &mut [T],
    channels: usize,
    tap: &Tap,
) -> u64
where
    T: SizedSample + FromSample<f32>,
{
    let frames = data.len() / channels;
    let mut done = 0;
    while done < frames {
        // Spelled out: fundsp's prelude brings a `min` of its own into scope
        // alongside `Ord`'s, and the two are ambiguous.
        let n = std::cmp::min(frames - done, MAX_BUFFER_SIZE);
        backend.process(n, &BufferRef::empty(), &mut block.buffer_mut());
        let rendered = block.buffer_ref();
        // Costs one atomic load when nothing is being recorded, which is
        // almost always. See `recorder`: it never blocks and never allocates.
        tap.push(&rendered, n);
        for i in 0..n {
            let frame = &mut data[(done + i) * channels..][..channels];
            frame[0] = T::from_sample(rendered.at_f32(0, i));
            if channels > 1 {
                frame[1] = T::from_sample(rendered.at_f32(1, i));
            }
        }
        done += n;
    }
    frames as u64
}

fn run_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut backend: impl AudioUnit + 'static,
    clock: Clock,
    tap: Arc<Tap>,
) -> Result<(), String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;

    // Rendered into a block at a time, then copied out interleaved. It is
    // allocated here rather than inside the callback, which may not allocate.
    let mut block = BufferVec::new(2);

    let stream = device
        .build_output_stream(
            config.clone(),
            move |data: &mut [T], _| {
                clock.advance(render(&mut backend, &mut block, data, channels, &tap));
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    std::thread::park(); // keep the !Send stream alive on this thread, forever
    Ok(())
}

pub fn swap_program(engine: &mut AudioEngine, program: Net) {
    let slot = engine.slot;
    engine.net.crossfade(slot, Fade::Smooth, 0.2, Box::new(program));
    engine.net.commit();
}

pub fn stop(engine: &mut AudioEngine) {
    let slot = engine.slot;
    // Must be 2-out: crossfade asserts the replacement matches the slot.
    engine.net.crossfade(slot, Fade::Smooth, 0.2, Box::new(dc((0.0, 0.0))));
    engine.net.commit();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render the insert for a while at a fixed input, and report the last
    /// output — past the glide, so this measures the setting rather than the
    /// ramp towards it.
    fn settled(unit: &mut Box<dyn AudioUnit>, input: f32) -> (f32, f32) {
        let mut out = [0.0f32; 2];
        for _ in 0..4410 {
            // 100ms at 44.1k, well past the glide
            unit.tick(&[input, input], &mut out);
        }
        (out[0], out[1])
    }

    /// `Net::connect` asserts on arity, so an insert of the wrong shape does
    /// not sound wrong — it panics the app on startup.
    #[test]
    fn the_master_is_a_stereo_insert() {
        let unit = master_gain(&Shared::new(1.0));
        assert_eq!(unit.inputs(), 2);
        assert_eq!(unit.outputs(), 2);
    }

    #[test]
    fn master_volume_scales_the_output() {
        let volume = Shared::new(DEFAULT_MASTER_VOLUME);
        let mut unit = master_gain(&volume);
        unit.set_sample_rate(44100.0);

        // The default must be unity: a program should be heard as written.
        let (l, r) = settled(&mut unit, 0.5);
        assert!((l - 0.5).abs() < 1e-3, "left was {l}");
        assert!((r - 0.5).abs() < 1e-3, "right was {r}");

        volume.set(0.25);
        let (l, _) = settled(&mut unit, 0.5);
        assert!((l - 0.125).abs() < 1e-3, "left was {l}");

        volume.set(0.0);
        let (l, r) = settled(&mut unit, 0.5);
        assert!(l.abs() < 1e-4 && r.abs() < 1e-4, "zero should silence, got ({l}, {r})");
    }

    /// Rising zero crossings, which is how many cycles of a tone went past.
    fn cycles(s: &[f32]) -> usize {
        s.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
    }

    /// A voice is stamped with the sequencer frontend's sample rate when it is
    /// pushed, and rendered at the backend's — so if the two disagree, every
    /// note in every pattern is transposed by the ratio between them. At the
    /// 48 kHz most devices open, and fundsp's 44100 default, that is a semitone
    /// and a half.
    ///
    /// A 1000 Hz voice must therefore produce 1000 cycles in a second of audio,
    /// whatever rate the device asked for.
    #[test]
    fn a_pushed_voice_runs_at_the_device_rate() {
        for rate in [44100.0, 48000.0, 96000.0] {
            let (mut net, mut seq, _, _) = wire(rate);
            let mut backend = net.backend();

            let tone = Net::wrap(Box::new(sine_hz::<f32>(1000.0) >> split::<U2>()));
            seq.push_duration(0.0, 4.0, Fade::Smooth, 0.0, 0.0, Box::new(tone));

            let s: Vec<f32> = (0..rate as usize).map(|_| backend.get_stereo().0).collect();
            let heard = cycles(&s);
            assert!(
                (heard as i64 - 1000).abs() <= 2,
                "at {rate} Hz a 1000 Hz voice was heard as {heard} Hz"
            );
        }
    }

    /// The persistent graph was always rated correctly; the point is that it
    /// still is, and that it now agrees with the voices beside it.
    #[test]
    fn the_graph_and_the_voices_agree_on_pitch() {
        const RATE: f64 = 48000.0;
        let (mut net, mut seq, slot, _) = wire(RATE);
        // The backend has to exist before an edit can be queued for it.
        let mut backend = net.backend();

        net.crossfade(slot, Fade::Smooth, 0.0,
            Box::new(sine_hz::<f32>(1000.0) >> split::<U2>()));
        net.commit();

        // One second of graph alone, then a second with the same tone as a
        // voice on top of it — equal windows, so the counts are comparable.
        let graph: Vec<f32> = (0..RATE as usize).map(|_| backend.get_stereo().0).collect();

        let tone = Net::wrap(Box::new(sine_hz::<f32>(1000.0) >> split::<U2>()));
        seq.push_duration(1.0, 4.0, Fade::Smooth, 0.0, 0.0, Box::new(tone));
        let both: Vec<f32> = (0..RATE as usize).map(|_| backend.get_stereo().0).collect();

        // Two tones of a pitch reinforce and cross zero as often as one does.
        // Two a semitone and a half apart beat, and the beat crosses far more
        // often — which is what a mistuned voice against the graph sounds like.
        assert!((cycles(&graph) as i64 - 1000).abs() <= 2, "the graph is off pitch");
        assert_eq!(cycles(&graph), cycles(&both),
            "the voice is not in tune with the graph");
    }

    /// A device buffer is longer than fundsp will render at once, so the
    /// callback fills it in several passes. The waveform has to carry across
    /// them: a pass that re-rendered or skipped a block would put a seam in
    /// every buffer, which is exactly the crackle this path exists to remove.
    #[test]
    fn a_buffer_longer_than_a_block_is_filled_without_a_seam() {
        const RATE: f64 = 48000.0;
        let mut backend = Net::wrap(Box::new(sine_hz::<f32>(1000.0) >> split::<U2>()));
        backend.set_sample_rate(RATE);
        let mut block = BufferVec::new(2);

        // 512 frames is eight of fundsp's blocks, and a common device buffer.
        let mut data = vec![0.0f32; 512 * 2];
        let mut whole: Vec<f32> = Vec::new();
        let tap = Tap::new(RATE);
        for _ in 0..40 {
            let frames = render(&mut backend, &mut block, &mut data, 2, &tap);
            assert_eq!(frames, 512, "every frame of the buffer must be accounted for");
            whole.extend(data.chunks(2).map(|f| f[0]));
        }

        // A 1000 Hz sine moves at most 0.14 per sample at 48 kHz. Anything
        // larger is a step, and a step is a seam.
        let worst = whole.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.15, "the waveform stepped by {worst}");

        // And the tone really is the tone: 40 buffers of 512 frames is
        // 20480 samples, which holds 426.7 cycles of 1000 Hz.
        let heard = cycles(&whole);
        assert!((heard as i64 - 426).abs() <= 1, "expected ~426 cycles, got {heard}");
    }

    /// Both channels are written, not just the one the mono path would fill.
    #[test]
    fn every_channel_of_every_frame_is_written() {
        let mut backend = Net::wrap(Box::new(dc((0.25, 0.75))));
        backend.set_sample_rate(44100.0);
        let mut block = BufferVec::new(2);

        // A sentinel no rendered sample can be, so anything left is a hole.
        let mut data = vec![f32::NAN; 100 * 2];
        assert_eq!(render(&mut backend, &mut block, &mut data, 2, &Tap::new(44100.0)), 100);

        for (i, frame) in data.chunks(2).enumerate() {
            assert!((frame[0] - 0.25).abs() < 1e-6, "frame {i} left was {}", frame[0]);
            assert!((frame[1] - 0.75).abs() < 1e-6, "frame {i} right was {}", frame[1]);
        }
    }

    /// A recording is meant to be the performance as it was heard, which means
    /// it is taken from the same block the device is filled from — the graph
    /// and the sequencer mixed together, with the master fader already applied.
    ///
    /// Tapped anywhere earlier and a take would disagree with the room it was
    /// played in: pulling the master down would still be a full-level file,
    /// and the sequencer's voices would be missing from it entirely.
    #[test]
    fn what_is_recorded_is_what_the_device_was_given() {
        const RATE: f64 = 48000.0;
        let (mut net, mut seq, slot, master) = wire(RATE);
        let mut backend = net.backend();

        // Something in the graph, something in the sequencer, and a master
        // that is not unity — all three are things a tap can miss.
        net.crossfade(slot, Fade::Smooth, 0.0, Box::new(dc((0.25, 0.25))));
        net.commit();
        seq.push_duration(0.0, 4.0, Fade::Smooth, 0.0, 0.0,
            Box::new(Net::wrap(Box::new(dc((0.25, 0.25))))));
        master.set(0.5);

        let tap = Tap::new(RATE);
        tap.arm();

        let mut block = BufferVec::new(2);
        let mut data = vec![0.0f32; 256 * 2];
        render(&mut backend, &mut block, &mut data, 2, &tap);

        let mut recorded = Vec::new();
        assert_eq!(tap.drain(&mut recorded), 256 * 2, "every frame should be tapped");
        assert_eq!(recorded, data, "the recording is the buffer the device was handed");

        // And it really did go through the master: past its glide the two
        // sources sum to 0.5 and the fader halves them.
        let settled = recorded[recorded.len() - 2];
        assert!((settled - 0.25).abs() < 1e-3, "the master was not in the path, got {settled}");
    }

    /// The state the app is in for all but a few minutes of its life. Nothing
    /// is kept, and the callback does no work for a recording nobody asked for.
    #[test]
    fn nothing_is_recorded_until_a_take_is_started() {
        let mut backend = Net::wrap(Box::new(dc((0.5, 0.5))));
        backend.set_sample_rate(44100.0);
        let mut block = BufferVec::new(2);
        let mut data = vec![0.0f32; 64 * 2];

        let tap = Tap::new(44100.0);
        render(&mut backend, &mut block, &mut data, 2, &tap);

        let mut recorded = Vec::new();
        assert_eq!(tap.drain(&mut recorded), 0);
    }

    /// The point of the glide: a jump in the setting must not become a jump in
    /// the signal.
    #[test]
    fn a_volume_jump_is_glided_not_stepped() {
        let volume = Shared::new(1.0);
        let mut unit = master_gain(&volume);
        unit.set_sample_rate(44100.0);
        let _ = settled(&mut unit, 1.0);

        volume.set(0.0);
        let mut out = [0.0f32; 2];
        unit.tick(&[1.0, 1.0], &mut out);
        assert!(out[0] > 0.9, "one sample later it should barely have moved, got {}", out[0]);
    }
}
