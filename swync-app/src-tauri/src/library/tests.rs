//! Packs, against real files.
//!
//! There is no map-of-files fake here as there is for imports, because what is
//! being tested *is* the filesystem work: what a zip is allowed to write, where
//! it lands, and what is left behind when it is refused. Every test builds its
//! own pack in a temp folder and installs it into another.

use std::io::Write;

use super::*;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swync-library-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("should make a temp folder");
    dir
}

/// A pack built entry by entry, so a test can write one that install must
/// refuse as easily as one it must accept.
fn pack(at: &Path, manifest: &str, entries: &[(&str, &str)]) -> PathBuf {
    let path = at.join(format!("pack.{EXTENSION}"));
    let file = std::fs::File::create(&path).expect("should create a pack");
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default();

    if !manifest.is_empty() {
        zip.start_file(MANIFEST, options).expect("should start");
        zip.write_all(manifest.as_bytes()).expect("should write");
    }
    for (name, body) in entries {
        zip.start_file(*name, options).expect("should start");
        zip.write_all(body.as_bytes()).expect("should write");
    }
    zip.finish().expect("should finish");
    path
}

const KIT: &str = r#"{ "name": "kit", "version": "1.0.0" }"#;

/// The ordinary pack: one module of its own name, and a folder of more.
fn a_kit(at: &Path) -> PathBuf {
    pack(
        at,
        KIT,
        &[
            ("root/kit.swync", "use kit::drums::*\nfn hit(f) = kick(f)\n"),
            ("root/kit/drums.swync", "fn kick(f) = sin(f)\n"),
            ("root/kit/samples/909.wav", "not really audio"),
        ],
    )
}

fn installed(store: &Path) -> Vec<String> {
    list(store, Source::Global).into_iter().map(|l| l.name).collect()
}

// ---- installing ----

/// What a pack brings lands where a `use` will look for it, and the store can
/// say what it holds.
#[test]
fn a_pack_unpacks_into_the_store() {
    let from = temp("install-from");
    let store = temp("install-store");

    let outcome = install(&a_kit(&from), &store, Source::Global, false).expect("should install");
    match outcome {
        Outcome::Installed(lib) => {
            assert_eq!(lib.name, "kit");
            assert_eq!(lib.version, "1.0.0");
            assert_eq!(lib.source, Source::Global);
        }
        other => panic!("expected an install, got {other:?}"),
    }

    assert!(store.join("kit.swync").is_file());
    assert!(store.join("kit/drums.swync").is_file());
    // The samples come too, or the instruments are silent.
    assert!(store.join("kit/samples/909.wav").is_file());
    assert_eq!(installed(&store), vec!["kit"]);

    std::fs::remove_dir_all(&from).ok();
    std::fs::remove_dir_all(&store).ok();
}

/// The rule the format rests on: a pack writes inside its own name and nowhere
/// else, so no library can ever reach another one's files.
#[test]
fn a_pack_may_not_write_outside_its_own_name() {
    let from = temp("greedy-from");
    let store = temp("greedy-store");

    let greedy = pack(
        &from,
        KIT,
        &[
            ("root/kit.swync", "fn hit(f) = sin(f)\n"),
            ("root/drums.swync", "fn kick(f) = sin(f)\n"),
        ],
    );

    let err = install(&greedy, &store, Source::Global, false).expect_err("should refuse");
    assert!(err.contains("drums.swync"), "the message should name it, got {err}");

    // And nothing of it survives the refusal — not even the part that was fine.
    assert!(!store.join("kit.swync").exists());
    assert!(installed(&store).is_empty());
    assert_eq!(std::fs::read_dir(&store).expect("should read").count(), 0);

    std::fs::remove_dir_all(&from).ok();
    std::fs::remove_dir_all(&store).ok();
}

/// A zip entry that climbs out of the archive is the same act by another name.
#[test]
fn a_pack_may_not_climb_out_of_itself() {
    let from = temp("climb-from");
    let store = temp("climb-store");

    let escaping = pack(&from, KIT, &[("root/../../elsewhere.swync", "fn x() = 1\n")]);

    let err = install(&escaping, &store, Source::Global, false).expect_err("should refuse");
    assert!(
        err.contains("not a path a pack may write") || err.contains("outside"),
        "unexpected message: {err}"
    );
    assert!(!from.parent().unwrap_or(&from).join("elsewhere.swync").exists());

    std::fs::remove_dir_all(&from).ok();
    std::fs::remove_dir_all(&store).ok();
}

/// Anything that is not a pack says so, rather than failing somewhere further
/// in where the message would be about zips.
#[test]
fn a_file_that_is_not_a_pack_is_named_as_one() {
    let from = temp("nonsense");
    let store = temp("nonsense-store");

    let not_a_pack = from.join("song.swyncpack");
    std::fs::write(&not_a_pack, "sin(440)\n").expect("should write");

    let err = install(&not_a_pack, &store, Source::Global, false).expect_err("should refuse");
    assert!(err.contains("not a library pack"), "unexpected message: {err}");

    // And a zip with no manifest in it is a different message, about that.
    let no_manifest = pack(&from, "", &[("root/kit.swync", "fn hit(f) = sin(f)\n")]);
    let err = install(&no_manifest, &store, Source::Global, false).expect_err("should refuse");
    assert!(err.contains(MANIFEST), "unexpected message: {err}");

    std::fs::remove_dir_all(&from).ok();
    std::fs::remove_dir_all(&store).ok();
}

/// A name a `use` could not write is a library nobody could import.
#[test]
fn a_name_that_is_not_an_identifier_is_refused() {
    let from = temp("badname-from");
    let store = temp("badname-store");

    let bad = pack(
        &from,
        r#"{ "name": "909 kit", "version": "1.0" }"#,
        &[("root/909 kit.swync", "fn hit(f) = sin(f)\n")],
    );

    let err = install(&bad, &store, Source::Global, false).expect_err("should refuse");
    assert!(err.contains("909 kit"), "the message should name it, got {err}");

    std::fs::remove_dir_all(&from).ok();
    std::fs::remove_dir_all(&store).ok();
}

// ---- installing over something ----

/// Installing over a library that is already there asks first, and has written
/// nothing by the time it asks.
#[test]
fn a_second_install_of_one_name_asks_before_replacing() {
    let from = temp("conflict-from");
    let store = temp("conflict-store");

    install(&a_kit(&from), &store, Source::Global, false).expect("should install");

    let newer = pack(
        &from,
        r#"{ "name": "kit", "version": "2.0.0" }"#,
        &[("root/kit.swync", "fn hit(f) = saw(f)\n")],
    );

    match install(&newer, &store, Source::Global, false).expect("should answer") {
        Outcome::Conflict { name, installed, incoming } => {
            assert_eq!(name, "kit");
            assert_eq!(installed, "1.0.0");
            assert_eq!(incoming, "2.0.0");
        }
        other => panic!("expected a conflict, got {other:?}"),
    }

    // Untouched: the old one is still the one a `use` would find.
    assert_eq!(
        std::fs::read_to_string(store.join("kit.swync")).expect("should read"),
        "use kit::drums::*\nfn hit(f) = kick(f)\n"
    );

    std::fs::remove_dir_all(&from).ok();
    std::fs::remove_dir_all(&store).ok();
}

/// And replacing leaves one library, not two halves of two.
#[test]
fn replacing_leaves_exactly_one_copy() {
    let from = temp("replace-from");
    let store = temp("replace-store");

    install(&a_kit(&from), &store, Source::Global, false).expect("should install");

    let newer = pack(
        &from,
        r#"{ "name": "kit", "version": "2.0.0" }"#,
        &[("root/kit.swync", "fn hit(f) = saw(f)\n")],
    );
    install(&newer, &store, Source::Global, true).expect("should replace");

    assert_eq!(installed(&store), vec!["kit"]);
    assert_eq!(list(&store, Source::Global)[0].version, "2.0.0");
    assert_eq!(
        std::fs::read_to_string(store.join("kit.swync")).expect("should read"),
        "fn hit(f) = saw(f)\n"
    );
    // The old folder went with the old library: what is installed is what the
    // new pack said, and nothing of what the old one said.
    assert!(!store.join("kit").exists());

    std::fs::remove_dir_all(&from).ok();
    std::fs::remove_dir_all(&store).ok();
}

// ---- removing and vendoring ----

/// Removing takes the modules, the samples and the bookkeeping.
#[test]
fn removing_takes_everything_the_pack_brought() {
    let from = temp("remove-from");
    let store = temp("remove-store");

    install(&a_kit(&from), &store, Source::Global, false).expect("should install");
    remove("kit", &store).expect("should remove");

    assert!(installed(&store).is_empty());
    assert!(!store.join("kit.swync").exists());
    assert!(!store.join("kit").exists());
    assert_eq!(std::fs::read_dir(&store).expect("should read").count(), 0);

    assert!(remove("kit", &store).is_err(), "removing twice should say so");

    std::fs::remove_dir_all(&from).ok();
    std::fs::remove_dir_all(&store).ok();
}

/// Vendoring makes the project's copy its own: the global one can go, and the
/// project still has everything the pack brought.
#[test]
fn a_vendored_library_stands_on_its_own() {
    let from = temp("vendor-from");
    let global = temp("vendor-global");
    let project = temp("vendor-project");
    let store = project_store(&project);

    install(&a_kit(&from), &global, Source::Global, false).expect("should install");
    let copied = vendor("kit", &global, &store).expect("should vendor");
    assert_eq!(copied.source, Source::Project);
    assert_eq!(copied.version, "1.0.0");

    remove("kit", &global).expect("should remove");

    assert_eq!(installed(&store), vec!["kit"]);
    assert!(store.join("kit/drums.swync").is_file());
    assert!(store.join("kit/samples/909.wav").is_file());

    std::fs::remove_dir_all(&from).ok();
    std::fs::remove_dir_all(&global).ok();
    std::fs::remove_dir_all(&project).ok();
}

// ---- exporting ----

/// The round trip is the claim: a folder packed here installs from there, with
/// every file it started with.
#[test]
fn a_packed_folder_installs_as_it_was() {
    let source = temp("export-source");
    let out = temp("export-out");
    let store = temp("export-store");

    std::fs::create_dir_all(source.join("kit/samples")).expect("should make");
    std::fs::write(source.join("kit.swync"), "use kit::drums::*\n").expect("should write");
    std::fs::write(source.join("kit/drums.swync"), "fn kick(f) = sin(f)\n").expect("should write");
    std::fs::write(source.join("kit/samples/909.wav"), "not really audio").expect("should write");
    // The machine's clutter is not part of the library.
    std::fs::write(source.join("kit/.DS_Store"), "clutter").expect("should write");

    let manifest = Manifest {
        name: "kit".into(),
        version: "3.1.0".into(),
        author: Some("Somebody".into()),
        description: None,
    };
    let dest = out.join(format!("kit.{EXTENSION}"));
    export(&source, &manifest, &dest).expect("should export");

    match install(&dest, &store, Source::Global, false).expect("should install") {
        Outcome::Installed(lib) => {
            assert_eq!(lib.version, "3.1.0");
            assert_eq!(lib.author.as_deref(), Some("Somebody"));
        }
        other => panic!("expected an install, got {other:?}"),
    }

    assert_eq!(
        std::fs::read_to_string(store.join("kit/drums.swync")).expect("should read"),
        "fn kick(f) = sin(f)\n"
    );
    assert!(store.join("kit/samples/909.wav").is_file());
    assert!(!store.join("kit/.DS_Store").exists());

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&out).ok();
    std::fs::remove_dir_all(&store).ok();
}

/// A folder with nothing of that name in it is not a library, and says so
/// rather than writing an empty pack.
#[test]
fn exporting_a_name_the_folder_does_not_hold_is_refused() {
    let source = temp("export-empty");
    let out = temp("export-empty-out");
    std::fs::write(source.join("song.swync"), "sin(440)\n").expect("should write");

    let manifest = Manifest {
        name: "kit".into(),
        version: "1.0".into(),
        author: None,
        description: None,
    };
    let dest = out.join(format!("kit.{EXTENSION}"));
    let err = export(&source, &manifest, &dest).expect_err("should refuse");
    assert!(err.contains("kit.swync"), "unexpected message: {err}");
    assert!(!dest.exists(), "a refused export should leave no file");

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&out).ok();
}

// ---- the shape the editor reads ----

/// The editor tells the two answers apart by `kind`, and reads a conflict's
/// versions straight off it. Both are checked here because a rename on this
/// side would otherwise be found by a dialog that never appears.
#[test]
fn an_outcome_serializes_to_the_shape_the_editor_expects() {
    let from = temp("shape-from");
    let store = temp("shape-store");

    let installed = install(&a_kit(&from), &store, Source::Global, false).expect("should install");
    let json = serde_json::to_value(&installed).expect("should serialize");
    assert_eq!(json["kind"], "installed");
    // The library's own fields sit alongside the tag, which is what the panel
    // lists from.
    assert_eq!(json["name"], "kit");
    assert_eq!(json["source"], "global");

    let newer = pack(&from, r#"{ "name": "kit", "version": "2.0.0" }"#, &[("root/kit.swync", "")]);
    let conflict = install(&newer, &store, Source::Global, false).expect("should answer");
    let json = serde_json::to_value(&conflict).expect("should serialize");
    assert_eq!(json["kind"], "conflict");
    assert_eq!(json["installed"], "1.0.0");
    assert_eq!(json["incoming"], "2.0.0");

    std::fs::remove_dir_all(&from).ok();
    std::fs::remove_dir_all(&store).ok();
}

/// The first export needs no answers: the file is written, seeded from the
/// project's name, and the second export reads what the first left.
#[test]
fn a_project_manifest_seeds_itself_and_then_stays_put() {
    let root = temp("manifest");

    let first = project_manifest(&root, "Nocturne").expect("should seed");
    assert_eq!(first.name, "Nocturne");
    assert!(root.join(PROJECT_MANIFEST).is_file());

    std::fs::write(
        root.join(PROJECT_MANIFEST),
        r#"{ "name": "kit", "version": "2.0.0" }"#,
    )
    .expect("should write");
    let second = project_manifest(&root, "Nocturne").expect("should read");
    assert_eq!(second.name, "kit");
    assert_eq!(second.version, "2.0.0");

    std::fs::remove_dir_all(&root).ok();
}

// ---- what a pack may not promise ----

fn a_manifest() -> Manifest {
    Manifest { name: "kit".into(), version: "1.0".into(), author: None, description: None }
}

/// Exporting a folder whose modules reach outside it refuses, because the
/// alternative is a pack that installs cleanly and then cannot find its own
/// sounds on somebody else's machine.
fn export_of(files: &[(&str, &str)], at: &str) -> Result<PathBuf, String> {
    let source = temp(at);
    let out = temp(&format!("{at}-out"));
    for (path, body) in files {
        let full = source.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("should make");
        }
        std::fs::write(full, body).expect("should write");
    }
    let result = export(&source, &a_manifest(), &out.join(format!("kit.{EXTENSION}")));
    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&out).ok();
    result
}

/// An absolute path is a path on the author's machine and nowhere else.
#[test]
fn an_absolute_sample_path_will_not_pack() {
    let err = export_of(
        &[("kit/hit.swync", "fn hit(f) = sample(load(\"/Users/me/909.wav\"), ramp())\n")],
        "audit-absolute",
    )
    .expect_err("should refuse");
    assert!(err.contains("909.wav"), "unexpected message: {err}");
    assert!(err.contains("this machine"), "unexpected message: {err}");
}

/// So is one that climbs out of the library.
#[test]
fn a_sample_path_out_of_the_library_will_not_pack() {
    let err = export_of(
        &[("kit/hit.swync", "fn hit(f) = sample(load(\"../breaks/amen.wav\"), ramp())\n")],
        "audit-escape",
    )
    .expect_err("should refuse");
    assert!(err.contains("outside the library"), "unexpected message: {err}");
}

/// And a `use` at the top of the library that names something the library does
/// not carry: it would resolve to whatever else is installed, or to nothing.
#[test]
fn an_import_of_something_outside_the_library_will_not_pack() {
    let err = export_of(
        &[("kit.swync", "use helpers::*\nfn hit(f) = sin(f)\n")],
        "audit-import",
    )
    .expect_err("should refuse");
    assert!(err.contains("helpers"), "unexpected message: {err}");
}

/// The shapes that do survive the move: the library's own files, named from
/// where they will be.
#[test]
fn a_library_that_names_only_its_own_files_packs() {
    export_of(
        &[
            ("kit.swync", "use kit::drums::*\nfn hit(f) = kick(f)\n"),
            ("kit/drums.swync", "fn kick(f) = sample(load(\"samples/909.wav\"), ramp())\n"),
            ("kit/samples/909.wav", "not really audio"),
        ],
        "audit-clean",
    )
    .expect("should pack");
}

/// A path from the top module reaches into the library's own folder — which is
/// the one way out of the store root that is still inside.
#[test]
fn the_top_module_may_name_its_own_folder() {
    export_of(
        &[
            ("kit.swync", "fn hit(f) = sample(load(\"kit/samples/909.wav\"), ramp())\n"),
            ("kit/samples/909.wav", "not really audio"),
        ],
        "audit-own-folder",
    )
    .expect("should pack");
}

// ---- the whole trip ----

/// A real one-channel wav, short enough to write by hand.
///
/// It has to decode, because the point of the test below is that the file the
/// program names is found and read — and a file of the right name holding the
/// wrong bytes would pass a test about paths while failing the thing the paths
/// are for.
fn a_wav() -> Vec<u8> {
    let samples: Vec<i16> = (0..64).map(|i| (i * 512 - 16384) as i16).collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();

    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend(((36 + data.len()) as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes()); // chunk size
    wav.extend(1u16.to_le_bytes()); // PCM
    wav.extend(1u16.to_le_bytes()); // mono
    wav.extend(44100u32.to_le_bytes());
    wav.extend((44100u32 * 2).to_le_bytes()); // bytes per second
    wav.extend(2u16.to_le_bytes()); // block align
    wav.extend(16u16.to_le_bytes()); // bits per sample
    wav.extend(b"data");
    wav.extend((data.len() as u32).to_le_bytes());
    wav.extend(data);
    wav
}

/// Export a library, install it, and play something out of it.
///
/// Everything the feature claims, in the order somebody would do it — and the
/// only test that joins them: a pack that installs but whose samples cannot be
/// found afterwards passes every test above this one, because each of those
/// asks about one folder and this asks about the trip between two.
#[test]
fn a_library_can_be_packed_installed_and_played() {
    let source = temp("trip-source");
    let out = temp("trip-out");
    let store = temp("trip-store");
    let project = temp("trip-project");

    // A library whose instrument reads a sample it carries, named from where
    // it is written rather than from wherever it ends up being played.
    std::fs::create_dir_all(source.join("kit/samples")).expect("should make");
    std::fs::write(
        source.join("kit.swync"),
        "use kit::drums::*\nfn hit(f, decay = 0.25) = kick(f) * perc(0.001, decay)\n",
    )
    .expect("should write");
    std::fs::write(
        source.join("kit/drums.swync"),
        "fn kick(f) = sample(load(\"samples/909.wav\"), ramp(f))\n",
    )
    .expect("should write");
    std::fs::write(source.join("kit/samples/909.wav"), a_wav()).expect("should write");

    let manifest = Manifest {
        name: "kit".into(),
        version: "1.0.0".into(),
        author: None,
        description: None,
    };
    let dest = out.join(format!("kit.{EXTENSION}"));
    export(&source, &manifest, &dest).expect("should export");

    // The author's folder is gone; everything from here is the recipient's.
    std::fs::remove_dir_all(&source).expect("should remove");
    install(&dest, &store, Source::Global, false).expect("should install");

    // A project with nothing of its own in it, and a song that reaches for the
    // library by name.
    let song = project.join("song.swync");
    let code = "use kit\nplay([50, 60], kit::hit, pan: [-1, 1])\n";
    std::fs::write(&song, code).expect("should write");

    let mut ws = crate::imports::Workspace::new(
        Some(song.display().to_string()),
        Some(project.display().to_string()),
    );
    ws.set_libraries(vec![store.clone()]);

    let items = crate::imports::expand(
        crate::parser::parser::parse(code.to_string()).expect("should parse"),
        code,
        &ws,
    )
    .unwrap_or_else(|e| panic!("should expand: {e}"));

    // The sample is found — from inside the store, which is the whole claim.
    let cache = crate::samples::Cache::default();
    let samples = crate::samples::Samples::load(&items, ws.dir().as_deref(), &cache)
        .unwrap_or_else(|e| panic!("should load: {e}"));

    let lowered = crate::lowerer::lower::lower_with_samples(&items, samples.clone())
        .unwrap_or_else(|e| panic!("should lower: {e}"));
    assert_eq!(lowered.bindings.len(), 1);
    assert_eq!(lowered.bindings[0].instrument, "kit::hit");

    // And the scheduler can build the voice that binding names, which is the
    // last thing between this and a sound.
    let instruments =
        crate::scheduler::voice::Instruments::from_program(&items).with_samples(samples);
    crate::scheduler::voice::build_voice(
        &instruments, "kit::hit", 50.0, &[], 0.5,
        crate::lowerer::lower::DEFAULT_BEAT_SECS, Default::default(),
    )
        .unwrap_or_else(|e| panic!("should build a voice: {e}"));

    std::fs::remove_dir_all(&out).ok();
    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&project).ok();
}
