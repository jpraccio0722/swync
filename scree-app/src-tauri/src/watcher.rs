//! Watching the open project's folder, so the tree does not have to be asked.
//!
//! The panel used to carry a refresh button, and it was there because nothing
//! here watched a filesystem: a file made by another program — a sample
//! dropped in from the Finder, a branch checked out, a module written by a
//! script — was invisible until somebody thought to press it. That is exactly
//! the case where you do not know to press it.
//!
//! So the folder is watched instead. What comes back is not a list of what
//! changed but a single "something did": the tree re-reads the folders it is
//! showing and asks the disk what is in them, which is the same thing it did
//! for the button and the only version that cannot drift from what is really
//! there.
//!
//! Two filters keep that from firing constantly, and both are the tree's own
//! rules rather than new ones:
//!
//! - **Hidden paths are ignored**, exactly as `list_dir` skips them. A `.git`
//!   during a commit writes hundreds of files, and not one of them is a row.
//! - **Content changes are ignored.** The tree shows names, so a file being
//!   written is not news — which is what most of these events are, since this
//!   app writes `patterns.scree` and `scree-project.json` as you work.
//!
//! Everything else refreshes, including events the platform could not classify.
//! The costs are lopsided: an extra refresh is a directory listing nobody
//! notices, and a missed one is the bug this module exists to remove.

use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Mutex;
use std::time::Duration;

use notify::event::{EventKind, ModifyKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

/// The event the editor listens for. It carries nothing: what changed is a
/// question the tree answers by reading, and one event per settled burst is
/// all the frontend can usefully act on anyway.
pub const PROJECT_CHANGED: &str = "project-changed";

/// How long a burst of events has to go quiet before it is reported.
///
/// Filesystems do not deliver one event per thing a person did — a save is
/// several, a checkout or an unzip is thousands. Waiting out the quiet means
/// the tree reads once at the end of that rather than once per file, and 200ms
/// is far below the point where a person notices a folder appearing late.
const SETTLE: Duration = Duration::from_millis(200);

/// The watch on whichever project is open.
///
/// Held as an `Option` because no project is a real state — a first run has
/// none — and because dropping the watcher is how a watch is ended: it closes
/// the channel, which is what stops the thread reading it. Opening another
/// project is therefore just replacing what is in here.
#[derive(Default)]
pub struct Watch {
    inner: Mutex<Option<RecommendedWatcher>>,
}

impl Watch {
    /// Watch `root`, or nothing at all, replacing whatever was watched before.
    ///
    /// `changed` is called on the watcher's own thread, once per settled burst.
    pub fn set<F>(&self, root: Option<&Path>, changed: F) -> Result<(), String>
    where
        F: Fn() + Send + 'static,
    {
        let mut inner = self.inner.lock().map_err(|_| "the watcher is poisoned")?;

        // Before anything else: the old watcher must go even if the new one
        // cannot be made, or a folder nobody is looking at any more would keep
        // asking the tree to read itself.
        *inner = None;

        let Some(root) = root else { return Ok(()) };

        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            // A closed channel is the watch having been replaced, which is not
            // a failure — the thread that was reading this has already gone.
            let _ = tx.send(event);
        })
        .map_err(|e| format!("could not watch {}: {e}", root.display()))?;

        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| format!("could not watch {}: {e}", root.display()))?;

        let watched = root.to_path_buf();
        std::thread::spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut worth_reading = matters(&first, &watched);

                // Drain the rest of the burst. Anything in it can be the one
                // event worth reporting, so the whole burst is judged rather
                // than the event that happened to arrive first.
                loop {
                    match rx.recv_timeout(SETTLE) {
                        Ok(next) => worth_reading |= matters(&next, &watched),
                        Err(RecvTimeoutError::Timeout) => break,
                        // The watch was replaced or the app is closing. There
                        // is nothing left to tell, and nobody to tell it to.
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }

                if worth_reading {
                    changed();
                }
            }
        });

        *inner = Some(watcher);
        Ok(())
    }
}

/// Whether an event is one the tree would draw differently for.
fn matters(event: &notify::Result<notify::Event>, root: &Path) -> bool {
    let event = match event {
        Ok(event) => event,
        // The backend itself failing — a queue overflowing, most likely, which
        // means events were lost. Precisely the moment to read again rather
        // than to trust what is on screen.
        Err(_) => return true,
    };

    if matches!(
        event.kind,
        EventKind::Access(_)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Metadata(_))
    ) {
        return false;
    }

    // A burst inside `.git` is not a burst in the project as the tree sees it.
    // An event with no paths at all is one nothing can be ruled out about, so
    // `any` on an empty list answering false is wrong here — hence the check.
    event.paths.is_empty() || event.paths.iter().any(|path| !is_hidden_below(path, root))
}

/// Whether a path is inside something hidden, judged from `root` down.
///
/// From `root` down, and not from the filesystem's, because the project itself
/// may well sit inside a hidden folder — a checkout under `~/.local`, or the
/// app's own library store. Judged from the top, every file in such a project
/// would be hidden and the watch would report nothing at all.
fn is_hidden_below(path: &Path, root: &Path) -> bool {
    let below: PathBuf = match path.strip_prefix(root) {
        Ok(rest) => rest.to_path_buf(),
        // Outside the folder being watched, which a rename out of it can be.
        // Not hidden, and worth reading: what left is a row that has gone.
        Err(_) => return false,
    };

    below.components().any(|component| match component {
        Component::Normal(name) => {
            name.to_str().is_some_and(|name| name.starts_with('.'))
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind, RenameMode};
    use notify::Event;

    fn event(kind: EventKind, paths: &[&str]) -> notify::Result<Event> {
        Ok(Event {
            kind,
            paths: paths.iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        })
    }

    const ROOT: &str = "/piece";

    /// The three things that put a row in the tree, or take one out.
    #[test]
    fn a_file_appearing_moving_or_going_is_worth_reading_again() {
        let root = Path::new(ROOT);
        assert!(matters(
            &event(EventKind::Create(CreateKind::File), &["/piece/kick.scree"]),
            root
        ));
        assert!(matters(
            &event(EventKind::Remove(RemoveKind::File), &["/piece/kick.scree"]),
            root
        ));
        assert!(matters(
            &event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &["/piece/kick.scree", "/piece/lib/kick.scree"]
            ),
            root
        ));
        // What the platform could not classify. Read again: an extra listing
        // costs nothing and a missed file is the whole bug.
        assert!(matters(&event(EventKind::Any, &["/piece/kick.scree"]), root));
        assert!(matters(&event(EventKind::Modify(ModifyKind::Any), &["/piece/x"]), root));
    }

    /// Most of what a filesystem reports is a file being written, and this app
    /// writes two of its own as you work. The tree shows names, not contents.
    #[test]
    fn a_file_being_written_is_not_worth_reading_again() {
        let root = Path::new(ROOT);
        assert!(!matters(
            &event(
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                &["/piece/patterns.scree"]
            ),
            root
        ));
        assert!(!matters(
            &event(
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)),
                &["/piece/song.scree"]
            ),
            root
        ));
    }

    /// The tree does not draw hidden files, so nothing that happens to one is
    /// news — and a commit is hundreds of them.
    #[test]
    fn a_burst_inside_a_hidden_folder_is_ignored() {
        let root = Path::new(ROOT);
        assert!(!matters(
            &event(EventKind::Create(CreateKind::File), &["/piece/.git/index.lock"]),
            root
        ));
        assert!(!matters(
            &event(EventKind::Create(CreateKind::File), &["/piece/.DS_Store"]),
            root
        ));
        // One real path among hidden ones still counts: bursts are mixed.
        assert!(matters(
            &event(
                EventKind::Create(CreateKind::File),
                &["/piece/.git/index.lock", "/piece/new.scree"]
            ),
            root
        ));
    }

    /// A project can perfectly well live inside a hidden folder, and judging
    /// from the filesystem's root rather than the project's would make every
    /// file in one invisible.
    #[test]
    fn a_project_inside_a_hidden_folder_is_still_watched() {
        let root = Path::new("/home/someone/.local/piece");
        assert!(matters(
            &event(
                EventKind::Create(CreateKind::File),
                &["/home/someone/.local/piece/kick.scree"]
            ),
            root
        ));
        assert!(!matters(
            &event(
                EventKind::Create(CreateKind::File),
                &["/home/someone/.local/piece/.git/HEAD"]
            ),
            root
        ));
    }

    /// A rename out of the project reports a path that is not below it. What
    /// left is a row that has gone, so it is read again.
    #[test]
    fn something_moved_out_of_the_project_is_worth_reading_again() {
        assert!(matters(
            &event(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                &["/somewhere/else/kick.scree"]
            ),
            Path::new(ROOT)
        ));
    }

    /// Ending a watch must not depend on a project being open next, or closing
    /// one would leave the last folder being read forever.
    #[test]
    fn watching_nothing_ends_the_watch_that_was_running() {
        let watch = Watch::default();
        let root = std::env::temp_dir().join(format!("scree-watch-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("should create a temp folder");

        watch.set(Some(&root), || {}).expect("should watch");
        assert!(watch.inner.lock().expect("should lock").is_some());

        watch.set(None, || {}).expect("should stop watching");
        assert!(watch.inner.lock().expect("should lock").is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    /// The whole path, on a real folder: a file appears and the editor is
    /// told, once, after the burst has settled.
    #[test]
    fn a_file_made_outside_the_app_reaches_the_editor() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let root = std::env::temp_dir().join(format!(
            "scree-watch-live-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).expect("should create a temp folder");

        let told = Arc::new(AtomicUsize::new(0));
        let count = told.clone();
        let watch = Watch::default();
        watch
            .set(Some(&root), move || {
                count.fetch_add(1, Ordering::SeqCst);
            })
            .expect("should watch");

        // Something else makes a file — the Finder, a script, another editor.
        std::fs::write(root.join("dropped-in.scree"), "let x = 1\n").expect("should write");

        // Long enough for the platform to notice and for the burst to settle.
        for _ in 0..40 {
            if told.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(
            told.load(Ordering::SeqCst),
            1,
            "one file should be reported exactly once"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
