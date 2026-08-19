//! What a program means when it names a port.

use crate::midi::ports::{find, Match, PortInfo, Selector};

/// The list a real desk looks like: two interfaces whose names share a word,
/// a virtual bus, and a synth. Enough that every rule below has something to
/// get wrong.
fn desk() -> Vec<PortInfo> {
    ["IAC Driver Bus 1", "Deluge MIDI 1", "Scarlett 2i2 USB MIDI 1", "Deluge MIDI 2"]
        .iter()
        .enumerate()
        .map(|(number, name)| PortInfo { number, name: name.to_string() })
        .collect()
}

fn name(selector: &str) -> Selector {
    Selector::Name(selector.to_string())
}

#[test]
fn a_name_matches_any_part_of_a_ports_name() {
    assert_eq!(find(&desk(), &name("scarlett")), Match::One(desk()[2].clone()));
}

#[test]
fn a_name_is_matched_without_regard_to_case() {
    assert_eq!(find(&desk(), &name("SCARLETT")), find(&desk(), &name("scarlett")));
}

#[test]
fn a_name_matching_several_ports_takes_the_first_and_names_the_rest() {
    let Match::Ambiguous(port, others) = find(&desk(), &name("deluge")) else {
        panic!("two ports are called Deluge, so this cannot be unambiguous");
    };
    assert_eq!(port, desk()[1]);
    assert_eq!(others, vec!["Deluge MIDI 2".to_string()]);
}

#[test]
fn a_name_specific_enough_to_pick_one_of_two_similar_ports_is_unambiguous() {
    assert_eq!(find(&desk(), &name("deluge midi 2")), Match::One(desk()[3].clone()));
}

#[test]
fn a_name_matching_nothing_is_missing_rather_than_a_failure() {
    assert_eq!(find(&desk(), &name("prophet")), Match::Missing);
}

#[test]
fn a_number_indexes_the_list_the_platform_reports() {
    assert_eq!(find(&desk(), &Selector::Number(1)), Match::One(desk()[1].clone()));
}

#[test]
fn a_number_past_the_end_of_the_list_is_missing() {
    assert_eq!(find(&desk(), &Selector::Number(9)), Match::Missing);
}

#[test]
fn nothing_matches_when_the_machine_has_no_midi_ports_at_all() {
    assert_eq!(find(&[], &name("deluge")), Match::Missing);
    assert_eq!(find(&[], &Selector::Number(0)), Match::Missing);
}

/// The empty name is what a half-typed `midiout("")` is, and it is a substring
/// of everything. Taking the first port is the same rule as any other
/// ambiguous name — what matters is that it does not panic or take the last.
#[test]
fn an_empty_name_is_ambiguous_rather_than_special() {
    let Match::Ambiguous(port, others) = find(&desk(), &name("")) else {
        panic!("every port contains the empty string");
    };
    assert_eq!(port, desk()[0]);
    assert_eq!(others.len(), 3);
}

/// A name is quoted where a number is not, so that the sentence a diagnostic
/// builds around it says which of the two was written.
#[test]
fn a_selector_says_whether_it_was_written_as_a_name_or_a_number() {
    assert_eq!(name("deluge").to_string(), "\"deluge\"");
    assert_eq!(Selector::Number(3).to_string(), "3");
}

/// What this machine has, for when something is not working and the question
/// is whether the platform is answering at all.
///
/// Ignored, like the loopback below and for the same reason: the suite has to
/// pass on a machine with no MIDI on it, and both of these ask the platform
/// rather than a fixture.
///
///     cargo test what_this_machine_actually_reports -- --ignored --nocapture
#[test]
#[ignore = "talks to the platform; the suite must pass on a machine with no MIDI"]
fn what_this_machine_actually_reports() {
    println!("outputs: {:?}", crate::midi::ports::outputs());
    println!("inputs:  {:?}", crate::midi::ports::inputs());
}

/// The one thing no other test here can show: that a note decided by the
/// scheduler actually leaves the machine, as the right bytes, at about the
/// right time.
///
/// Everything else about MIDI out is tested against a fixture — `Player`
/// queues and releases without a port, `Note::from_event` converts without a
/// wire — because a test suite that needed hardware would not run. This is the
/// seam those cannot cover, so it exists and is ignored:
///
///     cargo test a_note_reaches_a_real_port -- --ignored --nocapture
///
/// It needs a **loopback port**, which is a virtual MIDI bus wired to itself —
/// the IAC Driver on a Mac (enable it in Audio MIDI Setup), loopMIDI on
/// Windows. Skipped rather than failed when there is none: a machine without
/// one has not told us anything about the code.
#[test]
#[ignore = "needs a loopback MIDI port; run it by name when the wire is in doubt"]
fn a_note_reaches_a_real_port() {
    use std::sync::mpsc::channel;
    use crate::midi::out::{start, Destination, Note};
    use crate::scheduler::clock::Clock;
    use midir::{Ignore, MidiInput};

    const LOOPBACK: &str = "IAC Driver Bus 1";

    let mut input = MidiInput::new("swync test").expect("a MIDI client");
    input.ignore(Ignore::None);
    let Some(port) = input.ports().into_iter().find(|p| {
        input.port_name(p).is_ok_and(|n| n == LOOPBACK)
    }) else {
        println!("no {LOOPBACK} on this machine — nothing was tested");
        return;
    };

    let (tx, rx) = channel();
    let _listening = input
        .connect(&port, "swync test", move |_, bytes, _| { let _ = tx.send(bytes.to_vec()); }, ())
        .expect("the loopback should open");

    // The audio callback, which is what this whole thread is chasing. It has
    // to be here rather than left at a standstill: the anchor is corrected
    // *towards* the audio clock, so a clock frozen at zero pulls the
    // prediction back as fast as wall time pushes it forward and it settles
    // short of ever reaching a note due later on. That is not a bug in a
    // running app — a stopped audio clock is a stopped app — but it is the
    // first thing anybody writing a test here will hit.
    let clock = Clock::new(44100.0);
    let ticking = clock.clone();
    std::thread::spawn(move || {
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            ticking.advance(44100 / 200);
        }
    });

    let out = start(clock);
    out.play(vec![Note {
        destination: Destination {
            // Named the way a program would name it — part of it, in the
            // wrong case — so that what is exercised is the matching too.
            selector: Selector::Name("iac driver bus 1".to_string()),
            channel: 1,
        },
        channel: 1,
        note: 60,
        velocity: 100,
        on_secs: 0.05,
        off_secs: 0.15,
    }]);

    let mut got: Vec<Vec<u8>> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while got.len() < 2 && std::time::Instant::now() < deadline {
        if let Ok(bytes) = rx.recv_timeout(std::time::Duration::from_millis(100)) {
            got.push(bytes);
        }
    }

    println!("received: {got:?}");
    assert_eq!(got.len(), 2, "expected a note on and a note off, got {got:?}");
    assert_eq!(got[0], vec![0x90, 60, 100], "note on, channel 1");
    assert_eq!(got[1], vec![0x80, 60, 0], "note off, channel 1");
}


/// The input half's one thing no fixture can show: that a message put on a
/// real wire reaches the bus, as the right value, through the real callback.
///
/// The mirror of `a_note_reaches_a_real_port`, and ignored for the same
/// reason — everything else about MIDI in is tested against `receive` called
/// directly, because a suite that needed hardware would not run.
///
///     cargo test the_bus_hears_a_real_port -- --ignored --nocapture
///
/// Needs the same loopback bus, and here it is doing both jobs at once: the
/// test opens it as an *output*, sends, and the bus reads it back as an
/// *input*. Skipped rather than failed when there is none.
#[test]
#[ignore = "needs a loopback MIDI port; run it by name when the wire is in doubt"]
fn the_bus_hears_a_real_port() {
    use std::time::{Duration, Instant};
    use crate::midi::input::{bus, exclusive, slot_for};
    use midir::MidiOutput;

    const LOOPBACK: &str = "IAC Driver Bus 1";
    let _held = exclusive();

    let midi = MidiOutput::new("swync test").expect("a MIDI client");
    let Some(port) = midi.ports().into_iter().find(|p| {
        midi.port_name(p).is_ok_and(|n| n == LOOPBACK)
    }) else {
        println!("no {LOOPBACK} on this machine — nothing was tested");
        return;
    };
    let mut sending = midi.connect(&port, "swync test").expect("the loopback should open");

    // The real path: intern the port, then open it exactly as an eval does.
    let slot = slot_for(&Selector::Name("iac driver bus 1".into())).expect("a slot");
    let missing = crate::midi::input::ensure_open();
    assert!(missing.is_empty(), "the loopback should have opened: {missing:?}");

    // Controller 74 hard over, on channel 1.
    sending.send(&[0xB0, 74, 127]).expect("send");
    // And a note, which takes the other road entirely.
    sending.send(&[0x90, 60, 100]).expect("send");

    // The callback is on midir's thread, so this is the one place a wait is
    // honest — everything else here is called directly.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut notes = Vec::new();
    while notes.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        notes = bus().take_notes();
    }

    println!("controller 74 read back as {}", bus().controller(slot, 1, 74));
    println!("notes: {notes:?}");
    assert_eq!(bus().controller(slot, 1, 74), 1.0, "the knob should be hard over");
    assert_eq!(notes.len(), 1, "expected one note");
    assert_eq!(notes[0].note, 60);
    assert!(notes[0].on);
    assert!((notes[0].velocity - 100.0 / 127.0).abs() < 1e-6);
}

/// Clock, both directions, through a real port: the out thread sends ticks to
/// the loopback bus and the follower reads them back and moves the transport.
///
/// The one thing no fixture can show. Everything else about clock is tested
/// against `Player::clock` and `Follow::receive` called directly, because a
/// suite that needed hardware would not run.
///
///     cargo test the_clock_goes_out_and_comes_back -- --ignored --nocapture
///
/// Needs the same loopback bus as the other two. Skipped when there is none.
#[test]
#[ignore = "needs a loopback MIDI port; run it by name when the wire is in doubt"]
fn the_clock_goes_out_and_comes_back() {
    use std::time::{Duration, Instant};
    use crate::midi::input::{exclusive, following, slot_for};
    use crate::scheduler::clock::Clock;

    const LOOPBACK: &str = "IAC Driver Bus 1";
    let _held = exclusive();

    if !crate::midi::ports::outputs().iter().any(|p| p.name == LOOPBACK) {
        println!("no {LOOPBACK} on this machine — nothing was tested");
        return;
    }

    // The transport the *sender* runs on, ticking in real time so the out
    // thread has bar time to count against — the audio callback's part, as in
    // `a_note_reaches_a_real_port`.
    let sending = Clock::with_cps(44100.0, 0.5);
    let ticking = sending.clone();
    std::thread::spawn(move || {
        // Advanced by the time that *actually* passed, not by a fixed number
        // of frames per sleep. `thread::sleep` overshoots by a millisecond or
        // two, so a fixed advance makes audio time run slow — and since ticks
        // are placed on bar time and measured back in wall time, that reads as
        // a slower tempo at the far end. Measured: 82 bpm for a clock sent at
        // 120. A real audio callback advances by frames it really rendered,
        // which is what this now imitates.
        let mut last = Instant::now();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            std::thread::sleep(Duration::from_millis(2));
            let now = Instant::now();
            ticking.advance((now.duration_since(last).as_secs_f64() * 44100.0) as u64);
            last = now;
        }
    });

    // And a separate transport for the follower, so that what is being tested
    // is one clock reaching the other rather than a value being shared.
    let receiving = Clock::with_cps(44100.0, 0.1);
    following().drives(receiving.clone());
    let slot = slot_for(&Selector::Name(LOOPBACK.to_string())).expect("a slot");
    following().follow(Some((LOOPBACK, slot)));
    let missing = crate::midi::input::ensure_open();
    assert!(missing.is_empty(), "the loopback should have opened: {missing:?}");

    let out = crate::midi::out::start(sending);
    out.clock_to(vec![Selector::Name(LOOPBACK.to_string())]);
    out.transport(true);

    // Long enough for the tempo window to fill several times over.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && following().ticks() < 200 {
        std::thread::sleep(Duration::from_millis(20));
    }
    out.transport(false);

    println!("ticks received: {}", following().ticks());
    println!("tempo followed: {:.2} bpm (sent at 120)", receiving.bpm());
    assert!(following().ticks() > 100, "ticks should have arrived");
    assert!(
        (receiving.bpm() - 120.0).abs() < 6.0,
        "the followed tempo should be the sent one, got {}", receiving.bpm()
    );
}
