//! Editing the project from the tree: new folders, renames, moves, deletes.
//!
//! Everything here is one filesystem call with the checks around it that the
//! panel cannot make for itself. The checks are the point: a tree you can drag
//! things around in is a tree you can drop a folder into its own child, or
//! rename a file over one that was already there, and the difference between
//! those being refused and being carried out is somebody's work.
//!
//! Two rules hold across all of it.
//!
//! Nothing is ever overwritten. A move onto an existing path is refused rather
//! than merged or replaced, because the tree gives no way to see what you were
//! about to lose and no way to get it back. A copy keeps the same rule by the
//! other route — it takes a free name beside the one it collided with — because
//! pasting a file back into the folder it came from is the ordinary case for a
//! copy and refusing it would make the command useless where it is most used.
//!
//! Nothing is ever destroyed. A delete goes to the platform's trash, so the way
//! back is the one every other application on the machine has taught the user
//! already. The editor has no undo that reaches the filesystem, and inventing
//! one is worse than using the one that exists.
//!
//! [`reroute`] is the part that is not filesystem bookkeeping: a `use` names a
//! file by the route to it, so moving files means rewriting the programs that
//! import them.

pub mod reroute;
pub mod search;

use std::path::{Path, PathBuf};

use crate::imports::EXTENSION;
use reroute::Unfollowed;

/// Folders a walk never goes into, on top of the hidden ones every walk skips.
///
/// These two because a walk that reads them costs more than everything else put
/// together — and finds only files nobody wrote.
const SKIPPED: [&str; 2] = ["node_modules", "target"];

/// A ceiling on how much of a filesystem one walk will read.
///
/// A project is a folder somebody chose, and nothing stops that folder being a
/// home directory. Search and the import survey both stop here rather than
/// hanging: a truncated answer arrives, which is the honest one.
pub const MAX_FILES: usize = 20_000;

/// Every file under a folder, ignoring the ones above.
///
/// Symlinked folders are listed but never descended into. A link is the one way
/// a tree of folders can contain itself, and a walk that followed them could
/// run until the ceiling above stopped it, having read the same files a hundred
/// times on the way.
pub fn walk(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut folders = vec![root.to_path_buf()];

    while let Some(folder) = folders.pop() {
        let Ok(listing) = std::fs::read_dir(&folder) else {
            continue; // unreadable, which is not this walk's problem to report
        };
        for entry in listing.flatten() {
            if files.len() >= MAX_FILES {
                return files;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else { continue };

            // Hidden either way: a search that turned up a line in a file the
            // tree does not show is a result nobody can open from where they
            // are standing.
            if name.starts_with('.') {
                continue;
            }

            if kind.is_dir() {
                if SKIPPED.contains(&name.as_ref()) {
                    continue;
                }
                folders.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
        }
    }

    files
}

/// Make a folder, for the tree's New Folder.
///
/// Intermediate folders are made too, so typing `lib/drums` into the row makes
/// both — the same thing the shell does, and the same thing the name obviously
/// means.
#[tauri::command]
pub fn create_dir(path: String) -> Result<String, String> {
    let path = PathBuf::from(path);
    if path.exists() {
        return Err(format!("{} already exists", name_of(&path)));
    }
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// Make an empty file, for the tree's New File.
///
/// Refused rather than truncating, which is the whole difference between this
/// and `save_file`: one is a new file, the other is a file you have open.
#[tauri::command]
pub fn create_file(path: String) -> Result<String, String> {
    let path = PathBuf::from(path);
    if path.exists() {
        return Err(format!("{} already exists", name_of(&path)));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, "").map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// A move or a copy that happened, and what it cost.
///
/// One type for both because the two answer the same two questions — where the
/// thing landed, and which `use` paths the landing broke — and the panel does
/// the same thing with either answer.
#[derive(serde::Serialize, Debug)]
pub struct Moved {
    /// Where the file or folder ended up.
    pub path: String,
    /// Imports that could not be made to follow it. Empty for almost every
    /// move; when it is not, the editor shows these in the problems panel,
    /// because the move has already happened and the programs are already
    /// wrong.
    pub unfollowed: Vec<Unfollowed>,
}

/// Rename or move a file or folder.
///
/// One command for both, because on a filesystem they are one act — a rename is
/// a move that stays in the folder it started in — and because both break the
/// same thing: a `use` elsewhere in the project that named this file by the
/// route to it. See [`reroute`] for what is done about that.
///
/// `root` is the project folder, and bounds the search for programs to correct.
/// Without one nothing is corrected: a file being dragged around outside any
/// project has no set of importers to look through.
#[tauri::command]
pub fn move_path(from: String, to: String, root: Option<String>) -> Result<Moved, String> {
    let (from, to) = (PathBuf::from(from), PathBuf::from(to));

    if !from.exists() {
        return Err(format!("{} is no longer there", name_of(&from)));
    }
    // A folder cannot be put inside itself, and the filesystem's own answer to
    // being asked is an errno nobody should have to read.
    if from.is_dir() && to.starts_with(&from) {
        return Err(format!(
            "{} cannot be moved inside itself",
            name_of(&from)
        ));
    }
    // `exists` is not enough on a case-insensitive filesystem, where renaming
    // `drums.swync` to `Drums.swync` asks about a path that is already this
    // very file. That is a rename people make, and refusing it would be wrong.
    if to.exists() && !same_file(&from, &to) {
        return Err(format!("{} already exists", name_of(&to)));
    }

    // Only what a `use` could possibly have named. A dragged sample or image is
    // not something any program imports, and a survey for it would read every
    // file in the project to find that out.
    let follow = from.is_dir() || from.extension().is_some_and(|e| e == EXTENSION);
    let surveyed = match (&root, follow) {
        (Some(root), true) => reroute::survey(&walk(Path::new(root))),
        _ => Vec::new(),
    };

    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&from, &to).map_err(|e| e.to_string())?;

    Ok(Moved {
        path: to.display().to_string(),
        unfollowed: reroute::follow(&surveyed, &from, &to),
    })
}

/// Copy a file or folder, for the tree's Paste.
///
/// `to` is where the paste asked for it, and is the name it gets if that name
/// is free. If it is not, the copy takes the next free one beside it —
/// `drums copy.swync`, then `drums copy 2.swync` — rather than being refused
/// the way a move onto an occupied path is. Pasting into the folder something
/// was copied from is what a copy is usually for, and there is no other name
/// for the panel to have offered.
///
/// The copy's own imports are then corrected, which is the part that is not
/// filesystem bookkeeping. `use lib::drums` in a file pasted one folder up
/// resolves somewhere else, or nowhere; rewriting it to the route from where
/// the copy now sits is what makes the copy mean what the original meant. Only
/// the copy is surveyed, so nothing else in the project is touched — the file
/// every other program imported is still exactly where it was.
#[tauri::command]
pub fn copy_path(from: String, to: String) -> Result<Moved, String> {
    let (from, to) = (PathBuf::from(from), PathBuf::from(to));

    if !from.exists() {
        return Err(format!("{} is no longer there", name_of(&from)));
    }
    // The same refusal a move makes, and for a worse reason: a copy that walked
    // into its own destination would copy what it had just written, forever.
    if from.is_dir() && to.starts_with(&from) {
        return Err(format!("{} cannot be copied inside itself", name_of(&from)));
    }

    // Before anything is written, since afterwards the copy is on the disk and
    // a survey could not tell it from the original.
    let sources = if from.is_dir() {
        walk(&from)
    } else {
        vec![from.clone()]
    };
    let surveyed = reroute::survey(&sources);

    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let landed = unused(&to);

    if from.is_dir() {
        copy_tree(&from, &landed).map_err(|e| e.to_string())?;
    } else {
        std::fs::copy(&from, &landed).map_err(|e| e.to_string())?;
    }

    Ok(Moved {
        path: landed.display().to_string(),
        unfollowed: reroute::follow(&surveyed, &from, &landed),
    })
}

/// The path asked for, or the first free name beside it.
///
/// `drums.swync` becomes `drums copy.swync` and then `drums copy 2.swync`, and
/// a folder the same without an extension to keep. The word goes before the
/// extension rather than after it: `drums.swync copy` is a file the language no
/// longer recognises, which is a copy of the wrong thing.
fn unused(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }

    let dir = path.parent().unwrap_or(Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    for nth in 1.. {
        let suffix = if nth == 1 {
            " copy".to_string()
        } else {
            format!(" copy {nth}")
        };
        let candidate = dir.join(format!("{stem}{suffix}{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("the loop returns on the first free name")
}

/// Copy a folder and everything under it.
///
/// Symlinks are skipped rather than followed or recreated. Following one copies
/// a tree that is not inside this folder and may be this folder; recreating one
/// copies a path that means nothing once the copy is somewhere else. Skipping
/// is the only one of the three that cannot quietly do something enormous, and
/// it is what [`walk`] already does for the same reason.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let landing = to.join(entry.file_name());

        if kind.is_symlink() {
            continue;
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &landing)?;
        } else {
            std::fs::copy(entry.path(), &landing)?;
        }
    }
    Ok(())
}

/// Move a file or folder to the platform's trash.
///
/// Not `remove_file`. The tree has no undo, and a folder deleted from it would
/// take everything under it with no way back — so this hands the job to the
/// thing the machine already has for exactly this, and the way back is the one
/// the user already knows.
#[tauri::command]
pub fn delete_path(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err(format!("{} is no longer there", name_of(&path)));
    }
    trash::delete(&path).map_err(|e| e.to_string())
}

/// Whether two paths name the same file on disk.
///
/// Asked by canonicalising both, which resolves the case a case-insensitive
/// filesystem folded and the links either path went through.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// What to call a path in a message: its own name, since the person reading it
/// is looking at a tree that shows exactly that.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests;
