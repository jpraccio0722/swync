//! The checks around the filesystem calls, which are the whole of what this
//! module adds to them.
//!
//! `delete_path` is only tested for the check it makes. What it does when the
//! check passes is hand a real path to the platform's trash, and a test that
//! exercised that would be leaving rubbish in whoever ran it's Trash.

use super::*;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swync-files-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("should create a temp folder");
    dir
}

fn s(path: &Path) -> String {
    path.display().to_string()
}

#[test]
fn a_new_folder_is_made_with_whatever_it_needs_above_it() {
    let root = temp("new-folder");
    let made = create_dir(s(&root.join("lib/drums"))).expect("should create");
    assert!(Path::new(&made).is_dir());
    assert!(root.join("lib").is_dir(), "the folder above it too");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_new_file_is_empty_and_a_second_one_is_refused() {
    let root = temp("new-file");
    let path = root.join("song.swync");
    create_file(s(&path)).expect("should create");
    assert_eq!(std::fs::read_to_string(&path).expect("should read"), "");

    std::fs::write(&path, "let kick = 1\n").expect("should write");
    let err = create_file(s(&path)).expect_err("should refuse");
    assert!(err.contains("already exists"), "got {err}");
    // And the file that was there is still what it was.
    assert_eq!(
        std::fs::read_to_string(&path).expect("should read"),
        "let kick = 1\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_new_folder_over_an_existing_one_is_refused() {
    let root = temp("folder-exists");
    std::fs::create_dir(root.join("lib")).expect("should create");
    let err = create_dir(s(&root.join("lib"))).expect_err("should refuse");
    assert!(err.contains("already exists"), "got {err}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_rename_moves_the_file_and_says_where_it_went() {
    let root = temp("rename");
    std::fs::write(root.join("a.swync"), "let x = 1\n").expect("should write");

    let moved = move_path(s(&root.join("a.swync")), s(&root.join("b.swync")), None)
        .expect("should move");
    assert_eq!(moved.path, s(&root.join("b.swync")));
    assert!(!root.join("a.swync").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("b.swync")).expect("should read"),
        "let x = 1\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// The folder a move lands in is made if the name asked for one, so typing
/// `lib/song.swync` into a rename does what it plainly says.
#[test]
fn a_move_makes_the_folder_it_lands_in() {
    let root = temp("move-into-new");
    std::fs::write(root.join("song.swync"), "").expect("should write");

    move_path(s(&root.join("song.swync")), s(&root.join("lib/song.swync")), None)
        .expect("should move");
    assert!(root.join("lib/song.swync").is_file());
    std::fs::remove_dir_all(&root).ok();
}

/// The check that matters most: a move must never be a delete.
#[test]
fn a_move_onto_something_else_is_refused() {
    let root = temp("no-overwrite");
    std::fs::write(root.join("a.swync"), "mine\n").expect("should write");
    std::fs::write(root.join("b.swync"), "yours\n").expect("should write");

    let err = move_path(s(&root.join("a.swync")), s(&root.join("b.swync")), None)
        .expect_err("should refuse");
    assert!(err.contains("already exists"), "got {err}");
    assert_eq!(
        std::fs::read_to_string(root.join("b.swync")).expect("should read"),
        "yours\n"
    );
    // And the one that was being moved is still where it was.
    assert!(root.join("a.swync").is_file());
    std::fs::remove_dir_all(&root).ok();
}

/// On a case-insensitive filesystem the new name already "exists" — it is this
/// very file — and refusing would make a rename people really do impossible.
#[test]
fn changing_only_the_case_of_a_name_is_allowed() {
    let root = temp("case");
    std::fs::write(root.join("drums.swync"), "let kick = 1\n").expect("should write");

    move_path(s(&root.join("drums.swync")), s(&root.join("Drums.swync")), None)
        .expect("should rename");
    assert_eq!(
        std::fs::read_to_string(root.join("Drums.swync")).expect("should read"),
        "let kick = 1\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_folder_cannot_be_dropped_inside_itself() {
    let root = temp("into-itself");
    std::fs::create_dir_all(root.join("lib/drums")).expect("should create");

    let err = move_path(s(&root.join("lib")), s(&root.join("lib/drums/lib")), None)
        .expect_err("should refuse");
    assert!(err.contains("inside itself"), "got {err}");
    assert!(root.join("lib/drums").is_dir());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn moving_something_that_is_gone_says_so() {
    let root = temp("gone");
    let err = move_path(s(&root.join("nowhere.swync")), s(&root.join("b.swync")), None)
        .expect_err("should refuse");
    assert!(err.contains("no longer there"), "got {err}");

    let err = delete_path(s(&root.join("nowhere.swync"))).expect_err("should refuse");
    assert!(err.contains("no longer there"), "got {err}");
    std::fs::remove_dir_all(&root).ok();
}

/// The two ends of the feature meeting: a rename inside a project corrects the
/// programs that imported the file under its old name.
#[test]
fn a_move_within_a_project_corrects_its_importers() {
    let root = temp("importers");
    std::fs::write(root.join("drums.swync"), "let kick = 1\n").expect("should write");
    std::fs::write(root.join("song.swync"), "use drums::kick\nplay(kick)\n")
        .expect("should write");

    let moved = move_path(
        s(&root.join("drums.swync")),
        s(&root.join("percussion.swync")),
        Some(s(&root)),
    )
    .expect("should move");

    assert_eq!(moved.unfollowed, vec![]);
    assert_eq!(
        std::fs::read_to_string(root.join("song.swync")).expect("should read"),
        "use percussion::kick\nplay(kick)\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Without a project there is no set of importers to look through, and the
/// move is just a move.
#[test]
fn a_move_outside_a_project_corrects_nothing() {
    let root = temp("no-project");
    std::fs::write(root.join("drums.swync"), "let kick = 1\n").expect("should write");
    std::fs::write(root.join("song.swync"), "use drums::kick\n").expect("should write");

    move_path(s(&root.join("drums.swync")), s(&root.join("percussion.swync")), None)
        .expect("should move");
    assert_eq!(
        std::fs::read_to_string(root.join("song.swync")).expect("should read"),
        "use drums::kick\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// The plainest paste: somewhere the name is free, and the copy gets it.
#[test]
fn a_copy_lands_under_the_name_it_asked_for() {
    let root = temp("copy");
    std::fs::write(root.join("drums.swync"), "let kick = 1\n").expect("should write");
    std::fs::create_dir(root.join("lib")).expect("should create");

    let copied = copy_path(s(&root.join("drums.swync")), s(&root.join("lib/drums.swync")))
        .expect("should copy");

    assert_eq!(copied.path, s(&root.join("lib/drums.swync")));
    assert_eq!(
        std::fs::read_to_string(root.join("lib/drums.swync")).expect("should read"),
        "let kick = 1\n"
    );
    // And the original is still where it was, which is the whole difference
    // between this and a move.
    assert!(root.join("drums.swync").is_file());
    std::fs::remove_dir_all(&root).ok();
}

/// Pasting back into the folder it came from is what a copy is usually for,
/// so it takes a free name rather than being refused.
#[test]
fn a_copy_onto_an_occupied_name_takes_the_next_one_along() {
    let root = temp("copy-collide");
    std::fs::write(root.join("drums.swync"), "let kick = 1\n").expect("should write");

    let first = copy_path(s(&root.join("drums.swync")), s(&root.join("drums.swync")))
        .expect("should copy");
    assert_eq!(first.path, s(&root.join("drums copy.swync")));

    let second = copy_path(s(&root.join("drums.swync")), s(&root.join("drums.swync")))
        .expect("should copy");
    assert_eq!(second.path, s(&root.join("drums copy 2.swync")));

    // The extension is kept where the language can still see it, and the file
    // that was already there is untouched.
    assert_eq!(
        std::fs::read_to_string(root.join("drums.swync")).expect("should read"),
        "let kick = 1\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// A folder is copied with everything under it.
#[test]
fn a_copied_folder_brings_what_is_inside_it() {
    let root = temp("copy-folder");
    std::fs::create_dir_all(root.join("lib/deep")).expect("should create");
    std::fs::write(root.join("lib/drums.swync"), "let kick = 1\n").expect("should write");
    std::fs::write(root.join("lib/deep/bass.swync"), "let low = 1\n").expect("should write");

    let copied = copy_path(s(&root.join("lib")), s(&root.join("sounds"))).expect("should copy");

    assert_eq!(copied.path, s(&root.join("sounds")));
    assert_eq!(
        std::fs::read_to_string(root.join("sounds/deep/bass.swync")).expect("should read"),
        "let low = 1\n"
    );
    assert!(root.join("lib/drums.swync").is_file(), "the original stays");
    std::fs::remove_dir_all(&root).ok();
}

/// A copy that walked into its own destination would copy what it had just
/// written, and go on doing it.
#[test]
fn a_folder_cannot_be_copied_inside_itself() {
    let root = temp("copy-into-self");
    std::fs::create_dir(root.join("lib")).expect("should create");

    let err = copy_path(s(&root.join("lib")), s(&root.join("lib/lib")))
        .expect_err("should refuse");
    assert!(err.contains("inside itself"), "got {err}");
    std::fs::remove_dir_all(&root).ok();
}

/// A pasted file resolves its `use` paths against wherever it now sits, so the
/// copy is rewritten to reach what the original reached.
#[test]
fn a_copy_keeps_pointing_at_what_the_original_pointed_at() {
    let root = temp("copy-imports");
    std::fs::create_dir(root.join("lib")).expect("should create");
    std::fs::write(root.join("lib/drums.swync"), "let kick = 1\n").expect("should write");
    std::fs::write(root.join("lib/song.swync"), "use drums::kick\nplay(kick)\n")
        .expect("should write");

    let copied = copy_path(s(&root.join("lib/song.swync")), s(&root.join("song.swync")))
        .expect("should copy");

    assert_eq!(copied.unfollowed, vec![]);
    assert_eq!(
        std::fs::read_to_string(&copied.path).expect("should read"),
        "use lib::drums::kick\nplay(kick)\n"
    );
    // The file it was copied from is not a file anything moved, and reads
    // exactly as it did.
    assert_eq!(
        std::fs::read_to_string(root.join("lib/song.swync")).expect("should read"),
        "use drums::kick\nplay(kick)\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Two files copied together keep pointing at each other rather than back at
/// the pair they were copied from — the route between them did not change.
#[test]
fn imports_inside_a_copied_folder_stay_inside_the_copy() {
    let root = temp("copy-folder-imports");
    std::fs::create_dir(root.join("lib")).expect("should create");
    std::fs::write(root.join("lib/drums.swync"), "let kick = 1\n").expect("should write");
    std::fs::write(root.join("lib/kit.swync"), "use drums::kick\nlet k = kick\n")
        .expect("should write");

    copy_path(s(&root.join("lib")), s(&root.join("sounds"))).expect("should copy");

    assert_eq!(
        std::fs::read_to_string(root.join("sounds/kit.swync")).expect("should read"),
        "use drums::kick\nlet k = kick\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// A path only goes downward, so a copy placed where it cannot reach what it
/// imported is reported rather than guessed at — the same answer a move gives.
#[test]
fn a_copy_that_cannot_reach_its_imports_says_so() {
    let root = temp("copy-unreachable");
    std::fs::create_dir(root.join("lib")).expect("should create");
    std::fs::create_dir(root.join("other")).expect("should create");
    std::fs::write(root.join("lib/drums.swync"), "let kick = 1\n").expect("should write");
    std::fs::write(root.join("lib/song.swync"), "use drums::kick\n").expect("should write");

    // Into a folder beside the one it came from, where nothing written
    // downward can name the module it imported.
    let copied = copy_path(s(&root.join("lib/song.swync")), s(&root.join("other/song.swync")))
        .expect("should copy");

    assert_eq!(copied.unfollowed.len(), 1, "got {:?}", copied.unfollowed);
    assert!(copied.unfollowed[0].file.ends_with("song.swync"));
    // Left as written, so the copy is what the author wrote rather than a
    // rewrite that means something else.
    assert_eq!(
        std::fs::read_to_string(&copied.path).expect("should read"),
        "use drums::kick\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// A walk is what search and the import survey are both built on, so what it
/// leaves out is worth stating.
#[test]
fn a_walk_skips_hidden_and_generated_folders() {
    let root = temp("walk");
    std::fs::write(root.join("song.swync"), "").expect("should write");
    std::fs::create_dir_all(root.join(".git")).expect("should create");
    std::fs::write(root.join(".git/HEAD"), "").expect("should write");
    // A hidden file out in the open, which the tree does not show either.
    std::fs::write(root.join(".env"), "").expect("should write");
    std::fs::create_dir_all(root.join("node_modules/thing")).expect("should create");
    std::fs::write(root.join("node_modules/thing/index.js"), "").expect("should write");
    std::fs::create_dir_all(root.join("lib")).expect("should create");
    std::fs::write(root.join("lib/drums.swync"), "").expect("should write");

    let mut found: Vec<String> = walk(&root)
        .iter()
        .map(|p| p.strip_prefix(&root).expect("under root").display().to_string())
        .collect();
    found.sort();

    assert_eq!(found, vec!["lib/drums.swync".to_string(), "song.swync".to_string()]);
    std::fs::remove_dir_all(&root).ok();
}
