//! The ring between the audio callback and the disk, and the take that runs
//! over it.
//!
//! What is worth pinning here is everything the two threads agree on without
//! talking: that nothing is kept when nothing is recording, that what goes in
//! comes out in order, that a writer thread which falls behind loses audio
//! rather than the callback's deadline, and that a file with a name already on
//! the disk is never the file that gets written.

use super::*;

use fundsp::buffer::BufferVec;
use fundsp::MAX_BUFFER_SIZE;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "scree-recorder-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("should create a temp folder");
    dir
}

/// Play `frames` frames into the tap, as the callback would: in blocks of at
/// most `MAX_BUFFER_SIZE`, which is all a rendered block ever holds.
///
/// The left channel counts up from `from`, so a drained buffer can be checked
/// for order and for holes, and the right is its negative, so the two cannot
/// be swapped without the test noticing.
fn play(tap: &Tap, from: usize, frames: usize) {
    let mut buffer = BufferVec::new(2);
    let mut done = 0;
    while done < frames {
        let n = std::cmp::min(frames - done, MAX_BUFFER_SIZE);
        {
            let mut writing = buffer.buffer_mut();
            for i in 0..n {
                writing.set_f32(0, i, (from + done + i) as f32);
                writing.set_f32(1, i, -((from + done + i) as f32));
            }
        }
        tap.push(&buffer.buffer_ref(), n);
        done += n;
    }
}

/// The app spends almost all of its life here, and it must cost nothing and
/// keep nothing: a tap nobody has armed is a branch in the callback and no
/// more.
#[test]
fn a_tap_that_is_not_recording_keeps_nothing() {
    let tap = Tap::new(48_000.0);
    play(&tap, 0, 64);

    let mut drained = Vec::new();
    assert_eq!(tap.drain(&mut drained), 0);
    assert!(drained.is_empty());
}

#[test]
fn every_frame_pushed_comes_out_in_the_order_it_was_played() {
    let tap = Tap::new(48_000.0);
    tap.arm();

    // Several blocks, drained between two of them — the ring has to carry
    // across a drain as well as across its own wrap.
    play(&tap, 0, 100);
    play(&tap, 100, 100);

    let mut drained = Vec::new();
    assert_eq!(tap.drain(&mut drained), 400, "200 stereo frames is 400 samples");

    play(&tap, 200, 50);
    assert_eq!(tap.drain(&mut drained), 100);

    assert_eq!(drained.len(), 500);
    for i in 0..250 {
        assert_eq!(drained[i * 2], i as f32, "frame {i} left");
        assert_eq!(drained[i * 2 + 1], -(i as f32), "frame {i} right");
    }
    assert_eq!(tap.dropped(), 0);
}

/// The ring is four seconds long and the writer thread drains it forty times a
/// second, so this is a stall — a disk pausing, or the machine swapping. The
/// callback must come back either way: what it would be waiting for is a file,
/// and what it would be holding up is the sound in the room.
#[test]
fn a_writer_thread_that_falls_behind_loses_audio_rather_than_the_deadline() {
    // A ring of two hundred frames, so the test does not have to play four
    // seconds of audio to fill one.
    let tap = Tap::new(50.0);
    let capacity = tap.slots.len() / 2; // in frames
    tap.arm();

    play(&tap, 0, capacity);
    // Nothing has drained it, so there is nowhere for this to go.
    play(&tap, capacity, 64);
    assert_eq!(tap.dropped(), 64, "the block that did not fit should be counted");

    // And what was already in the ring is intact: a drop is a gap at the end,
    // never a tear in the middle.
    let mut drained = Vec::new();
    assert_eq!(tap.drain(&mut drained), capacity * 2);
    for i in 0..capacity {
        assert_eq!(drained[i * 2], i as f32, "frame {i} should be untouched");
    }

    // With room again, recording carries on.
    play(&tap, 0, 32);
    drained.clear();
    assert_eq!(tap.drain(&mut drained), 64);
}

/// Arming is what a second take does, and it must not begin with the tail of
/// the first one — or with its dropped-frame count.
#[test]
fn a_new_take_starts_from_an_empty_ring() {
    let tap = Tap::new(4_800.0);
    tap.arm();
    play(&tap, 0, 100);
    tap.disarm();

    tap.arm();
    let mut drained = Vec::new();
    assert_eq!(tap.drain(&mut drained), 0, "the last take should not be in this one");
    assert_eq!(tap.dropped(), 0);
}

/// The whole path, as a recording actually runs: a tap, a writer thread, a
/// file, and something that can read it back afterwards.
#[test]
fn a_take_holds_what_was_played_into_it_and_closes_a_readable_file() {
    let root = temp("take");
    let tap = Arc::new(Tap::new(48_000.0));
    let recorder = Recorder::new(tap.clone());

    let path = unique_path(&root, "a piece", Format::Wav32).expect("should name a file");
    recorder.start(&path, Format::Wav32).expect("should start");

    // Half a second of audio, pushed in blocks the way the callback would.
    let frames = 24_000;
    for start in (0..frames).step_by(512) {
        let count = std::cmp::min(512, frames - start);
        play(&tap, start, count);
        // Faster than real time, but not so fast that the ring — four seconds
        // of it — could overflow before the thread's next pass.
        std::thread::sleep(Duration::from_millis(1));
    }

    let state = recorder.state();
    assert!(state.recording, "it should still be running");
    assert_eq!(state.path.as_deref(), Some(path.display().to_string().as_str()));

    let finished = recorder.stop().expect("should stop");
    assert_eq!(finished.dropped, 0, "nothing should have been dropped");
    assert!(
        (finished.seconds - 0.5).abs() < 0.01,
        "half a second of audio should be half a second long, got {}",
        finished.seconds
    );

    let wave = fundsp::wave::Wave::load(&path).expect("should read the recording back");
    assert_eq!(wave.channels(), 2);
    assert_eq!(wave.length(), frames, "every frame pushed should be in the file");
    for i in 0..wave.length() {
        assert_eq!(wave.at(0, i), i as f32, "frame {i} left");
        assert_eq!(wave.at(1, i), -(i as f32), "frame {i} right");
    }

    // And the recorder is idle again, ready for another take.
    assert!(!recorder.state().recording);
    assert!(!tap.is_recording(), "the callback should have stopped filling the ring");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_second_take_cannot_be_started_over_the_one_that_is_running() {
    let root = temp("two-takes");
    let tap = Arc::new(Tap::new(48_000.0));
    let recorder = Recorder::new(tap);

    let first = unique_path(&root, "piece", Format::Wav16).expect("should name a file");
    recorder.start(&first, Format::Wav16).expect("should start");

    let second = root.join("another.wav");
    let err = recorder.start(&second, Format::Wav16).expect_err("should refuse");
    assert!(err.contains("already recording"), "got {err}");
    assert!(!second.exists(), "the file it refused should not have been made");

    recorder.stop().expect("should stop");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn stopping_when_nothing_is_recording_says_so() {
    let recorder = Recorder::new(Arc::new(Tap::new(48_000.0)));
    let err = recorder.stop().expect_err("should refuse");
    assert!(err.contains("nothing is being recorded"), "got {err}");
}

/// A folder that cannot be written to is an answer to the click that asked for
/// the recording, not a failure that turns up later — and it leaves the
/// recorder idle rather than half-armed.
#[test]
fn a_take_that_cannot_open_its_file_fails_at_once_and_records_nothing() {
    let root = temp("unwritable");
    let tap = Arc::new(Tap::new(48_000.0));
    let recorder = Recorder::new(tap.clone());

    // A folder that is not there, which is the state of a saved output folder
    // that has since been moved or unmounted.
    let path = root.join("gone").join("take.wav");
    let err = recorder.start(&path, Format::Wav16).expect_err("should refuse");
    assert!(err.contains("could not start recording"), "got {err}");

    assert!(!tap.is_recording(), "the tap should not have been armed");
    assert!(!recorder.state().recording);

    std::fs::remove_dir_all(&root).ok();
}

/// Two takes inside one second have the same timestamp, and the second one
/// must not be the end of the first.
#[test]
fn a_recording_is_never_written_over_one_that_is_already_there() {
    let root = temp("collision");

    let first = unique_path(&root, "piece", Format::Wav16).expect("should name a file");
    std::fs::write(&first, b"a take").expect("should write");

    let second = unique_path(&root, "piece", Format::Wav16).expect("should name a second");
    assert_ne!(first, second);
    assert!(
        second.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains("(2)")),
        "the second take should be numbered, got {second:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&first).expect("should read"),
        "a take",
        "the first take should be untouched"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// A project is named by whoever opened it, and the name goes into a path.
#[test]
fn a_project_name_that_would_be_a_path_is_made_into_a_file_name() {
    assert_eq!(sanitized("kick/snare"), "kick-snare");
    assert_eq!(sanitized("  spaced  "), "spaced");
    // Nothing that could still be read as a route out of the folder: the
    // separators are gone and it cannot begin with a dot.
    assert_eq!(sanitized("../../etc"), "-..-etc");
    assert_eq!(sanitized(""), "recording");
    assert_eq!(sanitized("..."), "recording");
    // The ordinary case is left exactly as it was written.
    assert_eq!(sanitized("Night Piece 3"), "Night Piece 3");
}

/// The dropdown is built from this, so a format missing from it is a format
/// nobody can choose — and one in it that the writer cannot open is a promise
/// broken after the take.
#[test]
fn every_format_the_recorder_can_write_is_offered_and_openable() {
    let root = temp("formats");
    let offered = formats();
    assert_eq!(offered.len(), 2, "both WAV formats should be offered");

    for info in offered {
        assert!(!info.label.is_empty());
        assert!(!info.detail.is_empty());
        let path = root.join(format!("check.{}", info.extension));
        std::fs::remove_file(&path).ok();
        wav::Writer::create(&path, info.id, 48_000.0, 2)
            .expect("every offered format should open")
            .finish()
            .expect("and close");
    }

    std::fs::remove_dir_all(&root).ok();
}
