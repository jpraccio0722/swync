//! Keeping a `use` pointed at the file it named, after that file has moved.
//!
//! A `use` path is a route rather than a name: `use lib::drums` is
//! `lib/drums.swync` beside the file that wrote it. Renaming that file, or
//! dragging either end of the pair anywhere else in the tree, quietly changes
//! what the path means — or stops it meaning anything at all. Nothing says so
//! until the next eval, which in a live-coding editor is the worst moment
//! available for finding out.
//!
//! So the explorer corrects them. Every `use` in the project is resolved to the
//! file it names *before* the move; afterwards each one is written back out as
//! the route from the importer's new folder to that file's new folder. The edit
//! is the path itself and nothing else — the `{…}` list, the `*`, an `as` that
//! was already there, the whitespace around all of it, are the file's and are
//! left as the file has them.
//!
//! ## Why an alias appears
//!
//! `use drums` binds the module under its last segment, so every `drums::kick`
//! in the importing file depends on what the *file* is called. Rewriting the
//! path to `use percussion` would leave all of those mentions naming nothing.
//! What was renamed is a file, not the binding, so the binding is kept: the
//! import becomes `use percussion as drums` and the body is not touched. That
//! is the whole reason this is safe to do behind a drag — the program after the
//! move compiles to what it compiled to before it.
//!
//! ## What cannot be followed
//!
//! A `use` path only ever goes downward. There is no `super`, no leading `::`,
//! no way to name a folder above the one the file is in — so a module dragged
//! out of its importer's subtree is one that no path can reach, and any rewrite
//! would be a guess. Those are reported instead, along with the files that could
//! not be parsed to be checked in the first place. The move still happens: a
//! broken import is worth knowing about, and it is not worth refusing to move
//! somebody's file over.

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::imports::{module_file, EXTENSION};
use crate::parser::parser::{parse, SwyncItem, Use, UseTree};

/// A `use` that could not be made to follow its file.
///
/// Not an error — the move it describes has already happened, and happened
/// because somebody asked for it. It is the note that goes to the problems
/// panel afterwards, so a program that will refuse to run says why now rather
/// than at the next eval.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct Unfollowed {
    /// The file holding the import, at whatever path it is at now.
    pub file: String,
    /// What is wrong with it, as a sentence.
    pub detail: String,
}

/// One `use`, as both the parser and the file's text see it.
#[derive(Debug, Clone, PartialEq)]
struct Import {
    /// Byte range of the `::`-joined path within the file, so a rewrite can
    /// replace the route without touching anything written around it.
    span: Range<usize>,
    /// The file the path resolved to, before anything moved.
    target: PathBuf,
    /// The item named after the file, for `use lib::drums::kick` — where `kick`
    /// is a definition inside `lib/drums.swync` rather than a folder step.
    item: Option<String>,
    /// The name the module is bound under in this file, for a `use` that takes
    /// the whole module and gives it no `as`. `None` for every other shape:
    /// a `{…}` list, a `*` and an item import all bind names the file's own
    /// path cannot change, and an explicit `as` says what it wants to be called.
    implicit_alias: Option<String>,
}

/// Every `use` in one file, and the text they were read from.
#[derive(Debug)]
pub struct Surveyed {
    path: PathBuf,
    code: String,
    imports: Vec<Import>,
    /// Set for a file that could not be parsed, so it can be reported rather
    /// than quietly treated as a file with no imports in it.
    unreadable: Option<String>,
}

/// Resolve every `use` in the project, before a move takes any of them apart.
///
/// Reading the lot for a single rename is deliberate. The alternative is
/// resolving lazily afterwards, which cannot work: the question each import has
/// to answer is what it pointed at *before*, and by then the file it pointed at
/// is gone from where it was.
pub fn survey(files: &[PathBuf]) -> Vec<Surveyed> {
    files
        .iter()
        .filter(|path| path.extension().is_some_and(|e| e == EXTENSION))
        .filter_map(|path| {
            let code = std::fs::read_to_string(path).ok()?;
            Some(match parse(code.clone()) {
                Ok(items) => Surveyed {
                    imports: imports_in(&items, &code, path),
                    path: path.clone(),
                    code,
                    unreadable: None,
                },
                Err(diagnostic) => Surveyed {
                    path: path.clone(),
                    code,
                    imports: Vec::new(),
                    unreadable: Some(diagnostic.message),
                },
            })
        })
        .collect()
}

/// Rewrite a survey's imports for a move that has already happened.
///
/// `from` and `to` are the two ends of the one move — a file or a whole folder.
/// Everything else follows from them: which files are now somewhere new, which
/// targets are, and which of the resulting pairs can still see each other.
pub fn follow(surveyed: &[Surveyed], from: &Path, to: &Path) -> Vec<Unfollowed> {
    let mut unfollowed = Vec::new();

    for file in surveyed {
        let moved_to = remap(&file.path, from, to);
        let Some(dir) = moved_to.parent() else { continue };

        if let Some(detail) = &file.unreadable {
            // Only worth saying when this file might have had something to
            // correct: every project has a file somebody is midway through
            // typing, and a rename elsewhere should not nag about it.
            if imports_could_have_pointed_here(&file.code, from) {
                unfollowed.push(Unfollowed {
                    file: moved_to.display().to_string(),
                    detail: format!(
                        "could not check this file's imports because it does not parse: {detail}"
                    ),
                });
            }
            continue;
        }

        // Collected before any of them is applied, and applied last-first
        // below: an edit earlier in the file moves every span after it.
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();

        for import in &file.imports {
            let target = remap(&import.target, from, to);
            if target == import.target && moved_to == file.path {
                continue; // neither end of this import went anywhere
            }

            let written = &file.code[import.span.clone()];
            match route(dir, &target, import) {
                Some(replacement) if replacement != written => {
                    edits.push((import.span.clone(), replacement))
                }
                Some(_) => {}
                None => unfollowed.push(Unfollowed {
                    file: moved_to.display().to_string(),
                    detail: format!(
                        "`use {written}` named {}, which is no longer inside this file's \
                         folder — a `use` path cannot reach above the file that writes it, \
                         so this import has to be moved or rewritten by hand",
                        target.display()
                    ),
                }),
            }
        }

        if edits.is_empty() {
            continue;
        }

        edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
        let mut code = file.code.clone();
        for (span, replacement) in edits {
            code.replace_range(span, &replacement);
        }

        if let Err(e) = std::fs::write(&moved_to, code) {
            unfollowed.push(Unfollowed {
                file: moved_to.display().to_string(),
                detail: format!("could not save this file's corrected imports: {e}"),
            });
        }
    }

    unfollowed
}

/// The `use` path that reaches `target` from `dir`, written as this import
/// wants it — including the `as` that keeps a renamed module's binding.
///
/// `None` when there is no such path: the target sits outside `dir`, or one of
/// the folders on the way to it has a name that cannot be written as a path
/// segment. Both are real answers, and neither is worth a guess.
fn route(dir: &Path, target: &Path, import: &Import) -> Option<String> {
    let relative = target.strip_prefix(dir).ok()?;

    let mut segments: Vec<String> = relative
        .with_extension("")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if segments.is_empty() || !segments.iter().all(|s| is_ident(s)) {
        return None;
    }

    // Whichever of the two shapes this import had. An item import writes the
    // item last, exactly where the parser will read it back from.
    let last = segments.last().expect("checked non-empty").clone();
    if let Some(item) = &import.item {
        segments.push(item.clone());
    }

    let mut path = segments.join("::");
    // The file's name is the module's name, so a rename that changes it would
    // rename every mention in the importing file too. The `as` is what stops
    // that: the route changes, the binding does not.
    if let Some(alias) = &import.implicit_alias {
        if *alias != last {
            path.push_str(" as ");
            path.push_str(alias);
        }
    }

    Some(path)
}

/// Where a path ends up, for the one move being followed. Unchanged for
/// anything the move did not cover, which is most of a project.
fn remap(path: &Path, from: &Path, to: &Path) -> PathBuf {
    if path == from {
        return to.to_path_buf();
    }
    match path.strip_prefix(from) {
        Ok(rest) => to.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// Whether a name can be written as a segment of a `use` path.
///
/// The lexer's rule for an identifier, which is the only thing a path is made
/// of. A folder called `my drums` or `2023` is a perfectly good folder and an
/// impossible import, and this is where that is noticed.
fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A cheap guess at whether an unparseable file was importing what just moved,
/// so the report is about files somebody needs to look at.
///
/// Text search rather than resolution, because there is no parse to resolve:
/// if the moved file's stem is not written anywhere in it, no `use` in it named
/// that file however the rest of it is broken.
fn imports_could_have_pointed_here(code: &str, from: &Path) -> bool {
    let Some(stem) = from.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
        return false;
    };
    code.contains("use") && code.contains(&stem)
}

/// Every `use` in one parsed file, resolved against the disk as it stands.
fn imports_in(items: &[SwyncItem], code: &str, path: &Path) -> Vec<Import> {
    let dir = path.parent().unwrap_or(Path::new(""));
    items
        .iter()
        .filter_map(|item| match item {
            SwyncItem::Use(u) => one(u, code, dir),
            _ => None,
        })
        .collect()
}

/// One `use`, if it currently resolves to a file and its path can be found in
/// the text. An import that already names nothing is left alone: it is broken
/// before the move, and correcting it is not this code's business.
fn one(u: &Use, code: &str, dir: &Path) -> Option<Import> {
    let span = path_span(code, u.at)?;
    let (target, item) = locate(u, dir)?;

    // `use drums` and `use lib::drums`, and only those: every other shape binds
    // a name the file's own path has no say over.
    let implicit_alias = match (&u.tree, &item) {
        (UseTree::Plain(None), None) => u.path.last().map(|s| s.0.clone()),
        _ => None,
    };

    Some(Import { span, target, item, implicit_alias })
}

/// Which file a `use` names, and the item it takes from it, if any.
///
/// The same two candidates `imports::Resolver::locate` weighs, resolved the
/// same way round: the whole path as a file first, and the path without its
/// last segment second, so `use lib::drums::kick` is a module when
/// `lib/drums/kick.swync` exists and an item of `lib/drums.swync` when it does
/// not. Disagreeing with the resolver here would rewrite a path into one that
/// means something else.
fn locate(u: &Use, dir: &Path) -> Option<(PathBuf, Option<String>)> {
    let whole = module_file(dir, &u.path);
    if whole.is_file() {
        return Some((whole, None));
    }

    match u.tree {
        // Both of these have to name a file: there is nothing after the path
        // for the last segment to be the name of.
        UseTree::Names(_) | UseTree::Glob => None,
        UseTree::Plain(_) if u.path.len() >= 2 => {
            let parent = module_file(dir, &u.path[..u.path.len() - 1]);
            parent
                .is_file()
                .then(|| (parent, u.path.last().map(|s| s.0.clone())))
        }
        UseTree::Plain(_) => None,
    }
}

/// The byte range of a `use`'s path, starting from the offset of its keyword.
///
/// The parser keeps where each `use` begins and nothing else, so the path is
/// found by reading it: the keyword, the space after it, then segments joined
/// by `::` for as long as a name follows each one. That last condition is what
/// stops the scan at the `::` of a `::{…}` or a `::*`, which are not part of
/// the route and must survive the rewrite untouched.
fn path_span(code: &str, at: usize) -> Option<Range<usize>> {
    if code.get(at..at + 3) != Some("use") {
        return None;
    }

    let bytes = code.as_bytes();
    let mut i = at + 3;
    while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
        i += 1;
    }

    let start = i;
    loop {
        let segment = i;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            i += 1;
        }
        if i == segment {
            break;
        }
        let continues = bytes[i..].starts_with(b"::")
            && bytes
                .get(i + 2)
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_');
        if !continues {
            break;
        }
        i += 2;
    }

    (i > start).then_some(start..i)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project on disk, since resolving a `use` is asking the filesystem
    /// which of two candidates exists.
    struct Project(PathBuf);

    impl Project {
        fn new(name: &str) -> Project {
            let dir = std::env::temp_dir().join(format!(
                "swync-reroute-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).expect("should create a temp project");
            Project(dir)
        }

        fn write(&self, relative: &str, code: &str) -> PathBuf {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("should create a folder");
            }
            std::fs::write(&path, code).expect("should write");
            path
        }

        fn read(&self, relative: &str) -> String {
            std::fs::read_to_string(self.0.join(relative)).expect("should read")
        }

        fn files(&self) -> Vec<PathBuf> {
            fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
                for entry in std::fs::read_dir(dir).expect("should list") {
                    let path = entry.expect("should read").path();
                    if path.is_dir() {
                        walk(&path, into);
                    } else {
                        into.push(path);
                    }
                }
            }
            let mut files = Vec::new();
            walk(&self.0, &mut files);
            files
        }

        /// Move a file the way the command does, with the survey taken first.
        fn move_file(&self, from: &str, to: &str) -> Vec<Unfollowed> {
            let (from, to) = (self.0.join(from), self.0.join(to));
            let surveyed = survey(&self.files());
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).expect("should create a folder");
            }
            std::fs::rename(&from, &to).expect("should move");
            follow(&surveyed, &from, &to)
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// The plainest case, and the one behind every rename in the tree.
    #[test]
    fn a_renamed_module_is_followed() {
        let p = Project::new("rename");
        p.write("drums.swync", "let kick = 1\n");
        p.write("song.swync", "use drums::kick\nplay(kick)\n");

        assert_eq!(p.move_file("drums.swync", "percussion.swync"), vec![]);
        assert_eq!(p.read("song.swync"), "use percussion::kick\nplay(kick)\n");
    }

    /// A whole-module import binds the module under the file's name, so the
    /// rename would break every mention in the body. The alias is what keeps
    /// the program meaning what it meant.
    #[test]
    fn a_renamed_module_keeps_the_name_its_importer_calls_it() {
        let p = Project::new("alias");
        p.write("drums.swync", "let kick = 1\n");
        p.write("song.swync", "use drums\nplay(drums::kick)\n");

        assert_eq!(p.move_file("drums.swync", "percussion.swync"), vec![]);
        assert_eq!(
            p.read("song.swync"),
            "use percussion as drums\nplay(drums::kick)\n"
        );
    }

    /// One that already had an `as` says what it wants to be called, so there
    /// is nothing to preserve and no second alias to add.
    #[test]
    fn an_existing_alias_is_left_to_say_what_it_says() {
        let p = Project::new("existing-alias");
        p.write("drums.swync", "let kick = 1\n");
        p.write("song.swync", "use drums as d\nplay(d::kick)\n");

        assert_eq!(p.move_file("drums.swync", "percussion.swync"), vec![]);
        assert_eq!(p.read("song.swync"), "use percussion as d\nplay(d::kick)\n");
    }

    /// Dragging a module into a folder lengthens the path to it.
    #[test]
    fn a_module_moved_deeper_is_reached_through_the_folder() {
        let p = Project::new("deeper");
        p.write("drums.swync", "let kick = 1\n");
        p.write("song.swync", "use drums::{kick}\nplay(kick)\n");

        assert_eq!(p.move_file("drums.swync", "lib/drums.swync"), vec![]);
        assert_eq!(p.read("song.swync"), "use lib::drums::{kick}\nplay(kick)\n");
    }

    /// And out of one shortens it. The `*` is not part of the path and must
    /// come through the rewrite exactly as it was written.
    #[test]
    fn a_glob_keeps_its_star() {
        let p = Project::new("glob");
        p.write("lib/drums.swync", "let kick = 1\n");
        p.write("song.swync", "use lib::drums::*\nplay(kick)\n");

        assert_eq!(p.move_file("lib/drums.swync", "drums.swync"), vec![]);
        assert_eq!(p.read("song.swync"), "use drums::*\nplay(kick)\n");
    }

    /// The importer is what moved, which changes the route just as much.
    #[test]
    fn moving_the_importer_rewrites_its_own_paths() {
        let p = Project::new("importer");
        p.write("lib/drums.swync", "let kick = 1\n");
        p.write("song.swync", "use lib::drums::kick\nplay(kick)\n");

        assert_eq!(p.move_file("song.swync", "lib/song.swync"), vec![]);
        assert_eq!(p.read("lib/song.swync"), "use drums::kick\nplay(kick)\n");
    }

    /// A folder dragged somewhere else takes its files with it, and every
    /// import of any of them follows.
    #[test]
    fn moving_a_folder_follows_the_files_inside_it() {
        let p = Project::new("folder");
        p.write("lib/drums.swync", "let kick = 1\n");
        p.write("song.swync", "use lib::drums::kick\nplay(kick)\n");

        assert_eq!(p.move_file("lib", "sounds/lib"), vec![]);
        assert_eq!(
            p.read("song.swync"),
            "use sounds::lib::drums::kick\nplay(kick)\n"
        );
    }

    /// Two files inside a moved folder keep pointing at each other: the route
    /// between them did not change, so neither does their text.
    #[test]
    fn imports_within_a_moved_folder_are_left_alone() {
        let p = Project::new("within");
        p.write("lib/drums.swync", "let kick = 1\n");
        p.write("lib/kit.swync", "use drums::kick\nlet k = kick\n");

        assert_eq!(p.move_file("lib", "sounds"), vec![]);
        assert_eq!(p.read("sounds/kit.swync"), "use drums::kick\nlet k = kick\n");
    }

    /// A path only goes downward, so this one cannot be written at all. The
    /// move still happens — it is what was asked for — and the import that can
    /// no longer be expressed is reported instead of guessed at.
    #[test]
    fn a_module_moved_out_of_reach_is_reported() {
        let p = Project::new("unreachable");
        p.write("lib/drums.swync", "let kick = 1\n");
        p.write("lib/kit.swync", "use drums::kick\nlet k = kick\n");

        let unfollowed = p.move_file("lib/drums.swync", "drums.swync");
        assert_eq!(unfollowed.len(), 1, "got {unfollowed:?}");
        assert!(unfollowed[0].file.ends_with("kit.swync"));
        assert!(
            unfollowed[0].detail.contains("use drums::kick"),
            "should quote the import, got {}",
            unfollowed[0].detail
        );
        // Left exactly as it was, so what is on disk is still what the author
        // wrote rather than a rewrite that means something else.
        assert_eq!(p.read("lib/kit.swync"), "use drums::kick\nlet k = kick\n");
    }

    /// A folder name that is not an identifier cannot appear in a path.
    #[test]
    fn a_folder_that_cannot_be_written_as_a_path_is_reported() {
        let p = Project::new("bad-name");
        p.write("drums.swync", "let kick = 1\n");
        p.write("song.swync", "use drums::kick\nplay(kick)\n");

        let unfollowed = p.move_file("drums.swync", "my drums/drums.swync");
        assert_eq!(unfollowed.len(), 1, "got {unfollowed:?}");
        assert_eq!(p.read("song.swync"), "use drums::kick\nplay(kick)\n");
    }

    /// An import of something else entirely is not touched, whatever else in
    /// the project is being dragged around.
    #[test]
    fn unrelated_imports_are_not_rewritten() {
        let p = Project::new("unrelated");
        p.write("drums.swync", "let kick = 1\n");
        p.write("bass.swync", "let low = 1\n");
        p.write("song.swync", "use bass::low\nuse drums::kick\nplay(kick)\n");

        assert_eq!(p.move_file("drums.swync", "percussion.swync"), vec![]);
        assert_eq!(
            p.read("song.swync"),
            "use bass::low\nuse percussion::kick\nplay(kick)\n"
        );
    }

    /// Several edits in one file, applied so that the earlier ones do not move
    /// the spans of the later ones out from under them.
    #[test]
    fn several_imports_in_one_file_are_all_corrected() {
        let p = Project::new("several");
        p.write("lib/drums.swync", "let kick = 1\n");
        p.write("lib/bass.swync", "let low = 1\n");
        p.write(
            "song.swync",
            "use lib::drums::kick\nuse lib::bass::low\nplay(kick)\n",
        );

        assert_eq!(p.move_file("lib", "sounds"), vec![]);
        assert_eq!(
            p.read("song.swync"),
            "use sounds::drums::kick\nuse sounds::bass::low\nplay(kick)\n"
        );
    }

    /// A file nobody can parse is a file whose imports nobody can check, and
    /// saying so is better than moving a module out from under it in silence.
    #[test]
    fn a_file_that_does_not_parse_is_reported_rather_than_edited() {
        let p = Project::new("broken");
        p.write("drums.swync", "let kick = 1\n");
        p.write("song.swync", "use drums::kick\nfn oops(\n");

        let unfollowed = p.move_file("drums.swync", "percussion.swync");
        assert_eq!(unfollowed.len(), 1, "got {unfollowed:?}");
        assert!(unfollowed[0].detail.contains("does not parse"));
        assert_eq!(p.read("song.swync"), "use drums::kick\nfn oops(\n");
    }

    /// And one that never mentioned the moved file does not get a note about
    /// it: every project has a file somebody is halfway through typing.
    #[test]
    fn an_unparseable_file_with_nothing_to_do_with_the_move_is_quiet() {
        let p = Project::new("quiet");
        p.write("drums.swync", "let kick = 1\n");
        p.write("scratch.swync", "fn oops(\n");

        assert_eq!(p.move_file("drums.swync", "percussion.swync"), vec![]);
    }

    #[test]
    fn the_span_covers_the_path_and_stops_before_what_follows_it() {
        let cases = [
            ("use drums", "drums"),
            ("use lib::drums", "lib::drums"),
            ("use lib::drums::kick", "lib::drums::kick"),
            ("use drums::{kick, snare}", "drums"),
            ("use drums::*", "drums"),
            ("use drums as d", "drums"),
            ("use  drums", "drums"),
        ];
        for (code, expected) in cases {
            let span = path_span(code, 0).unwrap_or_else(|| panic!("no span in {code}"));
            assert_eq!(&code[span], expected, "in {code}");
        }
    }

    #[test]
    fn a_use_that_is_not_the_first_thing_in_the_file_is_still_found() {
        let code = "let x = 1\nuse drums::*\n";
        let at = code.find("use").expect("there is a use");
        let span = path_span(code, at).expect("should find the path");
        assert_eq!(&code[span], "drums");
    }
}
