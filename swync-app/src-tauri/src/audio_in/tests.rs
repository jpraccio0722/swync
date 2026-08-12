//! What the ring has to do, pinned without a device.
//!
//! Every test here makes its own [`AudioIn`] rather than taking the process's
//! one from [`bus`], because the suite runs in threads and a shared ring would
//! be several tests pushing into each other's audio.

use super::*;

/// One channel's worth of what a device would send: frame `i` of channel `c`
/// is `i + c/10`, so a sample read back says exactly which frame and which
/// channel it came from.
fn block(frames: usize, channels: usize, from: usize) -> Vec<f32> {
    let mut data = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        for c in 0..channels {
            data.push((from + i) as f32 + c as f32 / 10.0);
        }
    }
    data
}

/// The frames the graph would read out of one rendered block.
fn heard(bus: &AudioIn, channel: usize, frames: usize) -> Vec<f32> {
    (0..frames).map(|i| bus.at(channel, i)).collect()
}

/// Fill past the lead the reader waits for, so the next `take` is a read
/// rather than more priming. Answers with how many frames went in.
fn prime(bus: &AudioIn, channels: usize, frames: usize) -> usize {
    for n in 0..4 {
        bus.push(&block(frames, channels, n * frames), channels);
    }
    4 * frames
}

/// The state the app is in on every machine that never opens an input, and the
/// one this must be cheapest in: the callback asks for a block and there is
/// nothing to do.
#[test]
fn nothing_is_heard_until_a_device_is_open() {
    let bus = AudioIn::new();
    assert_eq!(bus.channels(), 0);

    bus.take(64);
    assert_eq!(heard(&bus, 0, 64), vec![0.0; 64]);
    assert_eq!(bus.slippage(), (0, 0), "silence is not a failure to keep up");
}

#[test]
fn what_the_input_callback_pushed_is_what_the_graph_reads() {
    let bus = AudioIn::new();
    bus.opened(2);
    prime(&bus, 2, 64);

    bus.take(64);
    // The oldest frames first: a ring is a queue, not a snapshot.
    assert_eq!(heard(&bus, 0, 64), block(64, 1, 0));
    assert_eq!(
        heard(&bus, 1, 8),
        (0..8).map(|i| i as f32 + 0.1).collect::<Vec<_>>(),
        "the second channel is the second channel"
    );

    // And the next block carries on from where that one stopped, rather than
    // starting over or skipping.
    bus.take(64);
    assert_eq!(heard(&bus, 0, 64), block(64, 1, 64));
}

/// The whole reason the ring is drained into a block rather than read from
/// directly: `input(0)` may be written twice in one program, and a program in
/// which the two disagreed would be one where neither could be trusted.
#[test]
fn every_reader_of_a_block_hears_the_same_frames() {
    let bus = AudioIn::new();
    bus.opened(1);
    prime(&bus, 1, 64);
    bus.take(64);

    let first = heard(&bus, 0, 64);
    let second = heard(&bus, 0, 64);
    assert_eq!(first, second);
    assert_ne!(first, vec![0.0; 64], "and it is the input, not silence twice");
}

/// A device that has not delivered yet must not be the last frame held. Held
/// samples are a DC offset, which is a click at best and a silent speaker
/// excursion at worst — and it would be held for as long as the input stalled.
#[test]
fn a_reader_that_runs_dry_hears_silence_rather_than_the_last_frame() {
    let bus = AudioIn::new();
    bus.opened(1);
    prime(&bus, 1, 64);
    bus.take(64);

    // Nothing more arrives, and the ring is emptied.
    for _ in 0..8 {
        bus.take(64);
    }
    assert_eq!(heard(&bus, 0, 64), vec![0.0; 64]);

    let (late, _) = bus.slippage();
    assert!(late > 0, "frames the reader had to invent should be counted");
}

/// Having run dry, it waits for the lead again rather than reading each frame
/// the moment it lands — which would underrun on the very next block and stay
/// there, since the two devices are not in step and never will be.
#[test]
fn a_reader_that_has_run_dry_fills_up_again_before_it_reads() {
    let bus = AudioIn::new();
    bus.opened(1);
    prime(&bus, 1, 64);
    for _ in 0..8 {
        bus.take(64);
    }

    // One block back, which is less than the lead: still silence.
    bus.push(&block(64, 1, 1000), 1);
    bus.take(64);
    assert_eq!(heard(&bus, 0, 64), vec![0.0; 64]);

    // Filled past the lead, and it reads again — from the oldest frame it
    // still has, not from wherever the writer has got to.
    prime(&bus, 1, 64);
    bus.take(64);
    assert_eq!(heard(&bus, 0, 64), block(64, 1, 1000));
}

/// Two devices at a nominal 48 kHz do not agree on what a second is, so one of
/// them delivers more than the other takes. Left alone the delay would grow
/// all evening; this is the catching up, and it is counted.
#[test]
fn a_writer_that_gets_ahead_is_caught_up_rather_than_left_to_drift() {
    let bus = AudioIn::new();
    bus.opened(1);

    // Far more than the reader has asked for — a stall on the output side that
    // has just ended, or an hour of a fast input clock.
    for n in 0..40 {
        bus.push(&block(64, 1, n * 64), 1);
    }
    bus.take(64);

    let (_, dropped) = bus.slippage();
    assert!(dropped > 0, "the backlog should be thrown away, not queued");

    // What it hears is the recent past, not the beginning of the backlog.
    let first = bus.at(0, 0);
    assert!(first > 2000.0, "should have skipped forward, heard frame {first}");

    // And having caught up it stays caught up: one block in, one block out.
    for n in 40..48 {
        bus.push(&block(64, 1, n * 64), 1);
        bus.take(64);
    }
    let settled = bus.slippage().1;
    for n in 48..56 {
        bus.push(&block(64, 1, n * 64), 1);
        bus.take(64);
    }
    assert_eq!(bus.slippage().1, settled, "a steady stream should drop nothing");
}

/// A ring nobody is draining is the output stream having gone away mid-switch.
/// The input callback has a deadline of its own, so it drops rather than waits.
#[test]
fn a_ring_nobody_is_draining_drops_rather_than_blocking() {
    let bus = AudioIn::new();
    bus.opened(1);

    for n in 0..(RING_FRAMES / 64 + 8) {
        bus.push(&block(64, 1, n * 64), 1);
    }
    assert!(bus.slippage().1 > 0, "a full ring should drop and count");
}

/// Which channels exist is a fact about what is plugged in tonight. A program
/// written against an eight-in interface must still run on the laptop it is
/// edited on, and what it hears there is nothing.
#[test]
fn a_channel_the_device_does_not_have_is_silence() {
    let bus = AudioIn::new();
    bus.opened(2);
    prime(&bus, 2, 64);
    bus.take(64);

    assert_ne!(bus.at(1, 0), 0.0, "the channels it does have still sound");
    assert_eq!(heard(&bus, 5, 64), vec![0.0; 64]);
    assert_eq!(bus.at(MAX_CHANNELS, 0), 0.0, "and past the end is not a panic");
    assert_eq!(bus.at(0, MAX_BUFFER_SIZE), 0.0);
}

/// Switching device, or turning input off, has to silence what is still naming
/// it. Whatever was in the block would otherwise be held for as long as the
/// program ran, which is a DC offset rather than a memory.
#[test]
fn closing_a_device_silences_what_was_still_naming_it() {
    let bus = AudioIn::new();
    bus.opened(2);
    prime(&bus, 2, 64);
    bus.take(64);
    // Frame 5 rather than frame 0: the ramp these tests push starts at zero,
    // and "the first frame is silent" would be true either way.
    assert_ne!(bus.at(0, 5), 0.0);

    bus.closed();
    assert_eq!(bus.channels(), 0);
    assert_eq!(heard(&bus, 0, 64), vec![0.0; 64]);

    // And a block rendered afterwards is still silence rather than the ring's
    // leftovers, which are frames from a device that is no longer listening.
    bus.take(64);
    assert_eq!(heard(&bus, 0, 64), vec![0.0; 64]);
}

/// The meter is a peak since the last look, not a sample at the moment of it:
/// a poll twice a second that read the waveform where it happened to be would
/// show a silent meter on a signal that is clipping.
#[test]
fn the_meter_holds_the_loudest_thing_since_it_was_last_read() {
    let bus = AudioIn::new();
    bus.opened(2);

    let mut quiet = vec![0.1f32; 128];
    quiet[64] = -0.8; // one loud sample, in one channel, and negative
    bus.push(&quiet, 2);

    let levels = bus.levels();
    assert_eq!(levels.len(), 2, "one per channel of the open device");
    assert!((levels[0] - 0.8).abs() < 1e-6, "a peak is a magnitude: {levels:?}");
    assert!((levels[1] - 0.1).abs() < 1e-6, "channels are metered apart: {levels:?}");

    // Reading resets it, so the next drawing is about the next interval.
    assert_eq!(bus.levels(), vec![0.0, 0.0]);
}

#[test]
fn the_node_is_a_generator_with_one_output() {
    let node = InputNode { bus: Arc::new(AudioIn::new()), channel: 0 };
    assert_eq!(node.inputs(), 0);
    assert_eq!(node.outputs(), 1);
}

/// The node is the block, by index. Both paths through it agree, because the
/// engine renders with `process` and a good deal of fundsp still ticks.
#[test]
fn the_node_reads_its_own_channel_of_the_block() {
    let bus = Arc::new(AudioIn::new());
    bus.opened(2);
    prime(&bus, 2, 64);
    bus.take(64);

    let mut node = InputNode { bus: bus.clone(), channel: 1 };
    let mut out = BufferVec::new(1);
    node.process(64, &BufferRef::empty(), &mut out.buffer_mut());
    for i in 0..64 {
        let want = i as f32 + 0.1;
        assert!(
            (out.buffer_ref().at_f32(0, i) - want).abs() < 1e-6,
            "frame {i} was {}",
            out.buffer_ref().at_f32(0, i)
        );
    }

    let ticked: Frame<f32, typenum::U1> = node.tick(&Frame::default());
    assert!((ticked[0] - 0.1).abs() < 1e-6, "ticking reads the same first frame");
}

/// Stateless, so the scheduler can build one per note on its own thread —
/// the same property `SampleReader` has and for the same reason.
#[test]
fn reading_the_input_holds_no_state() {
    let bus = Arc::new(AudioIn::new());
    bus.opened(1);
    prime(&bus, 1, 64);
    bus.take(64);

    let mut one = InputNode { bus: bus.clone(), channel: 0 };
    let mut two = InputNode { bus: bus.clone(), channel: 0 };
    let first: Frame<f32, typenum::U1> = one.tick(&Frame::default());
    for _ in 0..10 {
        let _: Frame<f32, typenum::U1> = one.tick(&Frame::default());
    }
    let fresh: Frame<f32, typenum::U1> = two.tick(&Frame::default());
    assert_eq!(first[0], fresh[0]);
}

/// The bus a graph node finds is the same one the input thread fills. If these
/// were ever two objects, `input` would compile, run, meter — and be silent.
#[test]
fn every_caller_finds_the_same_bus() {
    assert!(Arc::ptr_eq(&bus(), &bus()));
    assert!(Arc::ptr_eq(&bus(), &InputNode::new(0).bus));
}

