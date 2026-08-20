//! `load("break.wav")` — an audio file, named in a program and read as a buffer.
//!
//! Two things have to be true at once. A program names a file by writing a path,
//! which only means anything relative to the file the path is written in — the
//! same rule `use` follows. And a voice is lowered per note on the scheduler
//! thread, which must never touch a disk.
//!
//! So loading happens once, at eval, and nowhere else. Every `load` in the
//! program is found by walking the syntax — including the ones inside `fn`
//! bodies, which are not lowered at eval time and would otherwise go unseen —
//! and each is resolved and decoded before anything is lowered. What comes out
//! is a [`Samples`]: the buffers the program asked for, under the paths it wrote
//! them as. The lowerer reads that map and never learns what a filesystem is.
//!
//! Decoded files are kept in a [`Cache`] that outlives the eval, so re-running a
//! program with a hundred-megabyte break in it costs one decode rather than one
//! per keystroke.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fundsp::wave::Wave;

use crate::parser::parser::{Expr, SwyncItem};

/// The builtin that names an audio file. Intercepted syntactically, like
/// `play`: the path has to be a literal for it to be findable by the walk
/// below, and a path assembled at runtime could not be loaded before the
/// program that assembles it runs.
pub const LOAD: &str = "load";

/// What an audio file is called, as far as this app is concerned — lowercase,
/// without the dot.
///
/// Nothing here is consulted before decoding: `load` hands any path it is given
/// to symphonia, which probes the file itself, so a `.wav` that is really an
/// AIFF still plays. This is the other question — *which rows in the project
/// tree are worth offering a play button on* — and it is answered here, beside
/// the decoder, rather than in a list in the frontend that could quietly come
/// to disagree with what can actually be read. It is served by the
/// `sample_extensions` command.
///
/// It is a hand-written list either way, since symphonia has no way to be asked
/// what it was built with. Adding one that cannot be decoded costs a button
/// that reports why when pressed; leaving one out costs a file with no button,
/// which nothing else in the app would explain.
pub const EXTENSIONS: &[&str] = &[
    "wav", "wave", "aif", "aiff", "aifc", "caf", "flac", "mp3", "m4a", "mp4", "aac", "ogg", "oga",
];

/// The buffers a program asked for, keyed by the path as it was written.
///
/// The key is the source text rather than the resolved path because that is
/// what the lowerer has in hand: two spellings of one file load twice into the
/// [`Cache`] and hit it the second time, which costs a hash lookup and keeps
/// this map a plain mirror of what the program says.
#[derive(Clone, Default)]
pub struct Samples {
    by_path: HashMap<String, Arc<Wave>>,
}

impl Samples {
    /// The buffer a `load` names, or `None` if this set was built without it.
    pub fn get(&self, written: &str) -> Option<Arc<Wave>> {
        self.by_path.get(written).cloned()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    /// Find every `load` in a program, resolve each against `dir`, and decode
    /// it — through `cache`, so a file already read is not read again.
    ///
    /// `dir` is the folder the program's own file sits in, exactly as a `use`
    /// resolves. `None` is a file that has never been saved: it has no folder,
    /// so a relative path has nothing to be relative to and is refused with
    /// that as the reason.
    pub fn load(
        items: &[SwyncItem],
        dir: Option<&Path>,
        cache: &Cache,
    ) -> Result<Samples, String> {
        let mut by_path = HashMap::new();
        for written in paths_in(items) {
            if by_path.contains_key(&written) {
                continue;
            }
            let wave = cache.read(&resolve(&written, dir)?)?;
            by_path.insert(written, wave);
        }
        Ok(Samples { by_path })
    }

    /// Build a set directly, for tests and for callers that have the buffers
    /// already.
    #[cfg(test)]
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, Arc<Wave>)>) -> Samples {
        Samples { by_path: pairs.into_iter().collect() }
    }
}

/// Where a written path points.
///
/// An absolute path is taken as written — `Path::join` already does this, and
/// it is worth being able to point at a sample library that lives nowhere near
/// the project. Anything else hangs off the folder the program is in.
fn resolve(written: &str, dir: Option<&Path>) -> Result<PathBuf, String> {
    if written.is_empty() {
        return Err(format!("{LOAD}: the path is empty"));
    }
    let path = Path::new(written);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    match dir {
        Some(dir) => Ok(dir.join(path)),
        None => Err(format!(
            "{LOAD}: '{written}' is relative to the file it is written in, and this \
             file has not been saved anywhere yet — save it, or write an absolute path"
        )),
    }
}

/// Files decoded so far, kept for the life of the app.
///
/// Held behind a mutex rather than handed out per eval because an eval is a
/// keystroke: the same break is named by every re-run of the same program, and
/// decoding it each time would stall the UI thread for as long as the file is
/// long.
///
/// Nothing is ever evicted. A live-coding session names a handful of files and
/// keeps them; an eviction policy would be a guess at which one is next.
#[derive(Default)]
pub struct Cache {
    by_file: Mutex<HashMap<PathBuf, Arc<Wave>>>,
}

impl Cache {
    /// The decoded file, from memory if it has been read before.
    ///
    /// The decode happens outside the lock. Holding it across a hundred
    /// megabytes of symphonia would serialize every other eval behind this one,
    /// and the cost of two threads racing to read the same file is that one of
    /// them did redundant work — not that anything is wrong afterwards.
    pub fn read(&self, path: &Path) -> Result<Arc<Wave>, String> {
        if let Some(wave) = self.peek(path) {
            return Ok(wave);
        }

        let wave = Wave::load(path).map_err(|e| match path.try_exists() {
            Ok(false) => format!("{LOAD}: no file at {}", path.display()),
            _ => format!("{LOAD}: could not read {}: {e}", path.display()),
        })?;
        if wave.channels() == 0 || wave.is_empty() {
            return Err(format!("{LOAD}: {} holds no audio", path.display()));
        }

        let wave = Arc::new(wave);
        if let Ok(mut by_file) = self.by_file.lock() {
            // Whoever got here first wins, so every reader of one path shares
            // one buffer however many raced for it.
            return Ok(by_file.entry(path.to_path_buf()).or_insert(wave).clone());
        }
        Ok(wave)
    }

    fn peek(&self, path: &Path) -> Option<Arc<Wave>> {
        self.by_file.lock().ok()?.get(path).cloned()
    }
}

/// Every path a program hands to `load`, in the order they are written.
///
/// A syntax walk rather than a lowering pass, because an instrument's body is
/// not lowered until the scheduler builds a voice from it — by which time the
/// file has to be in memory already. That reasoning belongs to more than this
/// one question now, so the walk itself lives in [`crate::parser::walk`] and
/// what is left here is only what `load` means. Duplicates come through; the
/// caller drops them.
///
/// Visible to the import tests, which have the same question in the other
/// direction: what an expanded program will go looking for.
pub(crate) fn paths_in(items: &[SwyncItem]) -> Vec<String> {
    let mut found = Vec::new();
    crate::parser::walk::calls_in(items, &mut |func, args| {
        if func.0 == LOAD {
            if let [arg] = args {
                if let Expr::Str(path) = &arg.value {
                    found.push(path.clone());
                }
            }
        }
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parser::parse;

    fn paths(src: &str) -> Vec<String> {
        paths_in(&parse(src.to_string()).expect("parse failed"))
    }

    #[test]
    fn a_top_level_load_is_found() {
        assert_eq!(paths("let b = load(\"amen.wav\")\n"), vec!["amen.wav"]);
    }

    /// The case the walk exists for: an instrument body is never lowered at
    /// eval time, so nothing but a syntax walk would see this one.
    #[test]
    fn a_load_inside_an_instrument_is_found() {
        let src = "fn hit(n) = sample(load(\"kick.wav\"), ramp(n))\n";
        assert_eq!(paths(src), vec!["kick.wav"]);
    }

    #[test]
    fn loads_are_found_through_every_kind_of_nesting() {
        let src = "\
fn f(n) = {
  let a = load(\"in_block.wav\")
  if n { sample(load(\"in_if.wav\"), 0) } else { sample(a, 0) }
}
let xs = [load(\"in_list.wav\")]
for i in 1..=2 { sample(load(\"in_for.wav\"), 0) }
sample(load(\"in_chain.wav\"), 0) >> lowpass(400, 1)
";
        let found = paths(src);
        for expected in
            ["in_block.wav", "in_if.wav", "in_list.wav", "in_for.wav", "in_chain.wav"]
        {
            assert!(found.iter().any(|p| p == expected), "missed {expected} in {found:?}");
        }
    }

    /// Only `load`'s own argument is a path. A string somewhere else is not
    /// this module's business — the lowerer refuses it.
    #[test]
    fn a_string_that_is_not_a_load_is_not_a_path() {
        assert!(paths("let x = \"amen.wav\"\n").is_empty());
    }

    #[test]
    fn a_relative_path_hangs_off_the_files_folder() {
        let dir = PathBuf::from("/songs/one");
        assert_eq!(
            resolve("kits/amen.wav", Some(&dir)).unwrap(),
            PathBuf::from("/songs/one/kits/amen.wav")
        );
    }

    #[test]
    fn an_absolute_path_is_taken_as_written() {
        let dir = PathBuf::from("/songs/one");
        assert_eq!(
            resolve("/library/amen.wav", Some(&dir)).unwrap(),
            PathBuf::from("/library/amen.wav")
        );
        // And needs no folder to be relative to.
        assert!(resolve("/library/amen.wav", None).is_ok());
    }

    /// An unsaved file has no folder, which is a thing to say plainly rather
    /// than to resolve against the process's working directory — which is
    /// wherever the app happened to be launched from.
    #[test]
    fn a_relative_path_with_no_folder_says_so() {
        let err = resolve("amen.wav", None).unwrap_err();
        assert!(err.contains("has not been saved"), "got: {err}");
    }

    #[test]
    fn an_empty_path_is_refused() {
        assert!(resolve("", Some(Path::new("/songs"))).is_err());
    }

    // ---- the cache, against real files ----

    /// A short wave on disk, in a fresh temp folder.
    fn a_wave_on_disk(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("swync-samples-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(name);
        let mut wave = Wave::new(1, 44100.0);
        for i in 0..1000 {
            wave.push((i as f32) / 1000.0 * 2.0 - 1.0);
        }
        wave.save_wav16(&path).expect("save");
        (dir, path)
    }

    #[test]
    fn a_file_is_decoded_and_then_remembered() {
        let (dir, path) = a_wave_on_disk("cached.wav");
        let cache = Cache::default();

        let first = cache.read(&path).expect("should load");
        let second = cache.read(&path).expect("should load");

        assert_eq!(first.length(), 1000);
        assert!(Arc::ptr_eq(&first, &second), "the second read should be the first buffer");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_says_which_one() {
        let cache = Cache::default();
        // `Wave` has no `Debug`, so `unwrap_err` is unavailable here.
        let err = match cache.read(Path::new("/nowhere/at/all/nope.wav")) {
            Err(e) => e,
            Ok(_) => panic!("expected an error for a file that is not there"),
        };
        assert!(err.contains("no file at"), "got: {err}");
        assert!(err.contains("nope.wav"), "got: {err}");
    }

    /// The whole path, end to end: source text in, buffer out.
    #[test]
    fn a_program_loads_the_file_it_names() {
        let (dir, _) = a_wave_on_disk("prog.wav");
        let items = parse("let b = load(\"prog.wav\")\n".to_string()).unwrap();

        let samples = Samples::load(&items, Some(&dir), &Cache::default()).expect("should load");
        assert_eq!(samples.get("prog.wav").expect("the buffer").length(), 1000);
        assert!(samples.get("other.wav").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A program that names no file needs no folder and reads no disk.
    #[test]
    fn a_program_with_no_loads_needs_nothing() {
        let items = parse("sin(220)\n".to_string()).unwrap();
        let samples = Samples::load(&items, None, &Cache::default()).expect("should be fine");
        assert!(samples.is_empty());
    }

    /// One file named twice is one buffer, not two.
    #[test]
    fn the_same_path_twice_loads_once() {
        let (dir, _) = a_wave_on_disk("twice.wav");
        let src = "let a = load(\"twice.wav\")\nlet b = load(\"twice.wav\")\n";
        let items = parse(src.to_string()).unwrap();

        let samples = Samples::load(&items, Some(&dir), &Cache::default()).expect("should load");
        assert_eq!(samples.by_path.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
