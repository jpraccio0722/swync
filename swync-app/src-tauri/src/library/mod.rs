//! Sound libraries — modules somebody else wrote, installed once and used
//! everywhere.
//!
//! A project is a folder, and until now that was the edge of the world: a `use`
//! reached files beside the one that wrote it and nowhere else, so there was no
//! way to hand somebody a set of instruments and no way to receive one. This is
//! that way. A **pack** is a zip with a name on it; installing one unpacks it
//! into a **store**, and a store is nothing but another folder a `use` may
//! resolve in.
//!
//! That last sentence is the whole design. `use kit::kick` already means
//! `kit/kick.swync` beside the file — [`crate::imports::module_file`] says so —
//! and an installed library is that same shape in a different folder, so
//! nothing about importing, renaming or lowering has to learn a new idea. The
//! only new thing is *where else* to look, and in what order:
//!
//! 1. beside the file, as it always was,
//! 2. the project's own copies, in `.swync/libraries/`,
//! 3. the ones installed on this machine.
//!
//! A file in the project always wins. That is the safe direction — installing
//! something can never change what a project that already worked means.
//!
//! ## What a pack looks like
//!
//! ```text
//! manifest.json      { "name": "kit", "version": "1.2.0" }
//! root/
//!   kit.swync        the module itself, and/or
//!   kit/             a folder of them
//!     kick.swync
//!     samples/909/kick.wav
//! ```
//!
//! Everything under `root/` is copied into the store as it stands, and
//! **everything under `root/` must be inside the pack's own name**. One pack
//! owns one top-level name, checked on the way in — which is what makes
//! installing safe without any version resolution behind it: two libraries can
//! never write the same file, so there is never a question of which one won.
//!
//! The manifest is kept beside what it installed, as `<name>.library.json`. It
//! is not a `.swync`, so it can never be imported, and it is how a store
//! answers what is in it.

#[cfg(test)]
mod tests;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::imports::EXTENSION as MODULE_EXTENSION;
use crate::parser::parser::SwyncItem;

/// The extension a pack carries.
pub const EXTENSION: &str = "swyncpack";

/// The manifest, inside a pack.
const MANIFEST: &str = "manifest.json";

/// The folder inside a pack whose contents become the library.
const ROOT: &str = "root";

/// What an installed library's manifest is called, in a store: the library's
/// name and this. Not a module extension, so nothing can import it.
const MANIFEST_SUFFIX: &str = ".library.json";

/// The folder libraries live in, under whatever holds a store.
pub const STORE: &str = "libraries";

/// The folder a project keeps its own copies under. Hidden, because it is not
/// part of the piece and the tree has no reason to show it.
pub const PROJECT_DIR: &str = ".swync";

/// What a project says it is when it is packed, beside `swync-project.json`
/// and read the same way.
///
/// A file rather than a dialog because a library is exported more than once —
/// that is what a version is for — and neither its name nor what it holds
/// should have to be retyped, or be able to differ between two exports of one
/// folder.
pub const PROJECT_MANIFEST: &str = "swync-library.json";

/// As much as will be unpacked out of one pack, in bytes.
///
/// A sample library is allowed to be large — this is a limit on a zip that
/// claims to be one and is not, rather than on anything anybody would send.
const MAX_UNPACKED: u64 = 4 * 1024 * 1024 * 1024;

/// What a pack says it is.
///
/// Only the name is required, and only because it is the name a `use` writes:
/// everything else here is for a human reading a list.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Which store a library was found in.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// Installed on this machine, for every project.
    Global,
    /// Copied into this project, and travelling with it.
    Project,
}

/// One installed library, as the panel lists it.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub source: Source,
    /// Absolute, so the panel can reveal it without knowing where stores are.
    pub path: String,
}

/// What installing did, or would have done.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Outcome {
    Installed(Library),
    /// Something of this name is already there. Not an error: the editor asks,
    /// and asks again with `replace` if the answer is yes. Nothing has been
    /// written by the time this comes back.
    Conflict {
        name: String,
        installed: String,
        incoming: String,
    },
}

/// Where installed libraries sit, under a folder that holds a store.
pub fn store_in(dir: &Path) -> PathBuf {
    dir.join(STORE)
}

/// A project's own store, which travels with the project.
pub fn project_store(root: &Path) -> PathBuf {
    root.join(PROJECT_DIR).join(STORE)
}

/// Where a library's manifest is written, in a store.
fn manifest_path(store: &Path, name: &str) -> PathBuf {
    store.join(format!("{name}{MANIFEST_SUFFIX}"))
}

/// The two things a library may own in a store: a module file of its own name,
/// and a folder of them. A pack may bring either, or both.
fn owned(store: &Path, name: &str) -> [PathBuf; 2] {
    [
        store.join(name).with_extension(MODULE_EXTENSION),
        store.join(name),
    ]
}

/// Whether a name may be written in a `use`.
///
/// The lexer's rule for an identifier, because that is what this becomes: a
/// library nobody can name is a library nobody can import.
fn usable_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Every library installed in a store, by name.
///
/// A store that is not there is a store with nothing in it — which is every
/// store until something is installed, and a project until one is vendored.
pub fn list(store: &Path, source: Source) -> Vec<Library> {
    let Ok(entries) = std::fs::read_dir(store) else {
        return Vec::new();
    };

    let mut found: Vec<Library> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.strip_suffix(MANIFEST_SUFFIX)?;
            let text = std::fs::read_to_string(&path).ok()?;
            let manifest: Manifest = serde_json::from_str(&text).ok()?;
            Some(Library {
                // What the store calls it, not what the file inside says: the
                // filename is what a `use` resolves against, and a hand-edited
                // manifest must not be able to disagree with the disk.
                name: name.to_string(),
                version: manifest.version,
                author: manifest.author,
                description: manifest.description,
                source,
                path: store.join(name).display().to_string(),
            })
        })
        .collect();

    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Read a pack's manifest without unpacking anything else.
pub fn manifest_of(pack: &Path) -> Result<Manifest, String> {
    let mut archive = open(pack)?;
    let mut text = String::new();
    archive
        .by_name(MANIFEST)
        .map_err(|_| format!("{} is not a library: it has no {MANIFEST}", name_of(pack)))?
        .read_to_string(&mut text)
        .map_err(|e| format!("could not read {MANIFEST}: {e}"))?;

    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|e| format!("could not read {MANIFEST}: {e}"))?;

    if !usable_name(&manifest.name) {
        return Err(format!(
            "`{}` cannot be the name of a library: a `use` writes it, so it has to \
             start with a letter and hold only letters, digits and underscores",
            manifest.name
        ));
    }

    Ok(manifest)
}

/// Install a pack into a store.
///
/// Nothing is written until every entry in the pack has been checked, and what
/// is written is staged beside the store and moved into place — so a pack that
/// turns out to be malformed halfway through leaves the store as it was, rather
/// than half a library that a `use` would then find.
pub fn install(
    pack: &Path,
    store: &Path,
    source: Source,
    replace: bool,
) -> Result<Outcome, String> {
    let manifest = manifest_of(pack)?;
    let name = manifest.name.clone();

    let existing = list(store, source).into_iter().find(|lib| lib.name == name);
    if let Some(existing) = existing {
        if !replace {
            return Ok(Outcome::Conflict {
                name,
                installed: existing.version,
                incoming: manifest.version,
            });
        }
    }

    let staged = stage(pack, &name, store)?;
    let result = (|| {
        for path in owned(store, &name) {
            discard(&path).map_err(|e| {
                format!("could not replace the library already at {}: {e}", path.display())
            })?;
        }

        let entries = std::fs::read_dir(&staged)
            .map_err(|e| format!("could not read what was unpacked: {e}"))?;
        for entry in entries.flatten() {
            let to = store.join(entry.file_name());
            std::fs::rename(entry.path(), &to)
                .map_err(|e| format!("could not install into {}: {e}", to.display()))?;
        }

        let text = serde_json::to_string_pretty(&manifest)
            .map_err(|e| format!("could not write the manifest: {e}"))?;
        std::fs::write(manifest_path(store, &name), text + "\n")
            .map_err(|e| format!("could not write the manifest: {e}"))
    })();

    std::fs::remove_dir_all(&staged).ok();
    result?;

    let installed = list(store, source)
        .into_iter()
        .find(|lib| lib.name == name)
        .ok_or_else(|| format!("`{name}` did not install"))?;

    Ok(Outcome::Installed(installed))
}

/// Unpack a pack's `root/` into a folder beside the store, checking every
/// entry as it goes.
///
/// The checks are all the same check: a pack may only write inside its own
/// name. An entry outside `root/`, one that climbs out of the archive with
/// `..`, one under somebody else's name — all of them are the pack reaching
/// somewhere it was not invited, and all of them stop the install.
fn stage(pack: &Path, name: &str, store: &Path) -> Result<PathBuf, String> {
    let mut archive = open(pack)?;

    let staged = store.join(format!(".staging-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&staged).ok();
    std::fs::create_dir_all(&staged)
        .map_err(|e| format!("could not make room to install into: {e}"))?;

    let unpack = (|| {
        let mut unpacked: u64 = 0;

        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| format!("could not read the pack: {e}"))?;

            // `enclosed_name` is the crate's own answer to an entry that
            // climbs out of the archive: anything absolute, or with a `..` in
            // it, has no enclosed name at all.
            let Some(path) = file.enclosed_name() else {
                return Err(format!("`{}` is not a path a pack may write", file.name()));
            };

            let Ok(inside) = path.strip_prefix(ROOT) else {
                // The manifest is the one thing outside `root/`, and it has
                // been read already.
                if path == Path::new(MANIFEST) {
                    continue;
                }
                return Err(format!(
                    "`{}` is outside `{ROOT}/` — everything a pack installs goes there",
                    path.display()
                ));
            };

            if inside.as_os_str().is_empty() {
                continue;
            }
            owns(name, inside)?;

            let to = staged.join(inside);
            if file.is_dir() {
                std::fs::create_dir_all(&to)
                    .map_err(|e| format!("could not unpack {}: {e}", inside.display()))?;
                continue;
            }

            unpacked = unpacked.saturating_add(file.size());
            if unpacked > MAX_UNPACKED {
                return Err(format!(
                    "`{name}` unpacks to more than {} GB, which is more than a library is",
                    MAX_UNPACKED / (1024 * 1024 * 1024)
                ));
            }

            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("could not unpack {}: {e}", inside.display()))?;
            }
            // Written rather than linked: this only ever creates plain files,
            // so nothing in a pack can become a link out of the store.
            let mut out = std::fs::File::create(&to)
                .map_err(|e| format!("could not unpack {}: {e}", inside.display()))?;
            std::io::copy(&mut file, &mut out)
                .map_err(|e| format!("could not unpack {}: {e}", inside.display()))?;
        }

        Ok(())
    })();

    if let Err(e) = unpack {
        std::fs::remove_dir_all(&staged).ok();
        return Err(e);
    }

    Ok(staged)
}

/// Whether a path inside `root/` is one this library may write.
///
/// `kit.swync` or anything under `kit/`, and nothing else. This is the check
/// the whole format rests on: it is why installing a library can never touch
/// another one, and why there is no version resolution here to get wrong.
fn owns(name: &str, inside: &Path) -> Result<(), String> {
    let head = inside
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or_default();

    let module = format!("{name}.{MODULE_EXTENSION}");
    if head == name || head == module {
        return Ok(());
    }

    Err(format!(
        "`{}` is not `{name}`'s to install — a pack writes `{module}` and `{name}/`, \
         and nothing else",
        inside.display()
    ))
}

/// Uninstall a library: its modules, its samples, and the manifest that said
/// it was there.
pub fn remove(name: &str, store: &Path) -> Result<(), String> {
    let mut gone = false;
    for path in owned(store, name).into_iter().chain([manifest_path(store, name)]) {
        if path.exists() {
            discard(&path)?;
            gone = true;
        }
    }

    if !gone {
        return Err(format!("`{name}` is not installed"));
    }
    Ok(())
}

/// Delete a path in a store, if it is there.
///
/// Straight out, and not to the trash — which is the opposite of what
/// [`crate::files::delete_path`] does, on purpose. That deletes a file somebody
/// wrote, and the trash is the way back they already know. This deletes a copy
/// of a pack: nobody wrote it, the way back is to install it again, and a
/// hundred megabytes of somebody else's samples sitting in the trash is a
/// worse answer than none.
fn discard(path: &Path) -> Result<(), String> {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else if path.exists() {
        std::fs::remove_file(path)
    } else {
        return Ok(());
    };
    result.map_err(|e| format!("could not remove {}: {e}", path.display()))
}

/// Copy a library from one store into another, so a project carries its own.
///
/// The point is a project folder that is the whole of itself: hand it to
/// somebody, or check it into a repository, and what it plays does not depend
/// on what happens to be installed on the machine that opens it.
pub fn vendor(name: &str, from: &Path, to: &Path) -> Result<Library, String> {
    let manifest = manifest_path(from, name);
    if !manifest.exists() {
        return Err(format!("`{name}` is not installed"));
    }

    std::fs::create_dir_all(to)
        .map_err(|e| format!("could not make {}: {e}", to.display()))?;

    for path in owned(from, name) {
        if !path.exists() {
            continue;
        }
        let into = to.join(path.file_name().expect("a store entry has a name"));
        discard(&into)?;
        copy_into(&path, &into)?;
    }

    std::fs::copy(&manifest, manifest_path(to, name))
        .map_err(|e| format!("could not copy the manifest: {e}"))?;

    list(to, Source::Project)
        .into_iter()
        .find(|lib| lib.name == name)
        .ok_or_else(|| format!("`{name}` did not copy"))
}

/// A file, or a folder and everything under it.
fn copy_into(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_file() {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("could not copy {}: {e}", from.display()))
    } else {
        std::fs::create_dir_all(to)
            .map_err(|e| format!("could not make {}: {e}", to.display()))?;
        let entries = std::fs::read_dir(from)
            .map_err(|e| format!("could not read {}: {e}", from.display()))?;
        for entry in entries.flatten() {
            copy_into(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    }
}

/// What a project would be packed as, writing the file if it has none yet.
///
/// Seeded from `seed` — the project's own name — so the first export is still
/// one click, and left on disk so the second one is too. The name it holds is
/// what a `use` will write, and what the project must hold a file or folder
/// called; if it does not, [`export`] says so and this file is where the
/// correction goes.
pub fn project_manifest(root: &Path, seed: &str) -> Result<Manifest, String> {
    let path = root.join(PROJECT_MANIFEST);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("could not read {PROJECT_MANIFEST}: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let manifest = Manifest {
                name: seed.to_string(),
                version: "1.0.0".to_string(),
                author: None,
                description: None,
            };
            let text = serde_json::to_string_pretty(&manifest)
                .map_err(|e| format!("could not write {PROJECT_MANIFEST}: {e}"))?;
            std::fs::write(&path, text + "\n")
                .map_err(|e| format!("could not write {PROJECT_MANIFEST}: {e}"))?;
            Ok(manifest)
        }
        Err(e) => Err(format!("could not read {PROJECT_MANIFEST}: {e}")),
    }
}

/// Write a folder out as a pack.
///
/// The mirror of [`install`], and held to the same rule: what goes in is what
/// comes out, so a pack built here installs without a word. `root` is the
/// folder holding `<name>.swync` and/or `<name>/`.
pub fn export(root: &Path, manifest: &Manifest, dest: &Path) -> Result<PathBuf, String> {
    if !usable_name(&manifest.name) {
        return Err(format!(
            "`{}` cannot be the name of a library: a `use` writes it, so it has to \
             start with a letter and hold only letters, digits and underscores",
            manifest.name
        ));
    }

    let sources: Vec<PathBuf> = owned(root, &manifest.name)
        .into_iter()
        .filter(|p| p.exists())
        .collect();
    if sources.is_empty() {
        let module = format!("{}.{MODULE_EXTENSION}", manifest.name);
        return Err(format!(
            "there is no `{module}` or `{}/` in {} — a library is named for what it \
             holds, so those are what would be packed",
            manifest.name,
            root.display()
        ));
    }

    audit(&sources, &manifest.name)?;

    let file = std::fs::File::create(dest)
        .map_err(|e| format!("could not write {}: {e}", dest.display()))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default();

    let write = (|| -> Result<(), String> {
        let text = serde_json::to_string_pretty(manifest)
            .map_err(|e| format!("could not write the manifest: {e}"))?;
        zip.start_file(MANIFEST, options)
            .map_err(|e| format!("could not write the manifest: {e}"))?;
        zip.write_all(text.as_bytes())
            .map_err(|e| format!("could not write the manifest: {e}"))?;

        for source in &sources {
            let name = source.file_name().expect("a source has a name");
            add(&mut zip, source, &Path::new(ROOT).join(name), options)?;
        }

        zip.finish()
            .map(|_| ())
            .map_err(|e| format!("could not write {}: {e}", dest.display()))
    })();

    if write.is_err() {
        std::fs::remove_file(dest).ok();
    }
    write?;

    Ok(dest.to_path_buf())
}

/// Everything that would still be true on the machine this pack is opened on.
///
/// A library is written in one folder and read in another, and two things a
/// file may say survive that move badly: a path out of the library, and a `use`
/// that names something the library does not carry. Neither fails here — they
/// fail on somebody else's machine, at an eval, as a file that is not there.
/// So they are refused while the author is still looking at it.
///
/// Which paths escape depends on where the file sits, because that is what they
/// are relative to. `kit.swync` sits beside every other library in the store,
/// so anything it names that is not under `kit/` reaches out of itself; a file
/// inside `kit/` is already enclosed, and only a `..` gets it out.
fn audit(sources: &[PathBuf], name: &str) -> Result<(), String> {
    let mut problems = Vec::new();

    for source in sources {
        let enclosed = source.is_dir();
        for file in modules_under(source) {
            let Ok(code) = std::fs::read_to_string(&file) else { continue };
            let Ok(items) = crate::parser::parser::parse(code) else {
                problems.push(format!("{} does not parse", name_of(&file)));
                continue;
            };

            for written in crate::samples::paths_in(&items) {
                let path = Path::new(&written);
                if path.is_absolute() {
                    problems.push(format!(
                        "{} loads {written}, which is a path on this machine and \
                         nowhere else — move the file into the library and name it \
                         from there",
                        name_of(&file)
                    ));
                } else if escapes(path, enclosed, name) {
                    problems.push(format!(
                        "{} loads {written}, which is outside the library",
                        name_of(&file)
                    ));
                }
            }

            // Only the module at the top: everything under `kit/` resolves
            // inside `kit/` already, whatever it writes.
            if enclosed {
                continue;
            }
            for item in &items {
                let SwyncItem::Use(u) = item else { continue };
                let head = u.path.first().map(|s| s.0.as_str()).unwrap_or_default();
                if head != name {
                    problems.push(format!(
                        "{} imports `{}`, which is not part of `{name}` — a library \
                         reaches its own files and no others",
                        name_of(&file),
                        u.written()
                    ));
                }
            }
        }
    }

    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "this would not play on another machine:\n  {}",
        problems.join("\n  ")
    ))
}

/// Whether a relative path reaches outside the library.
fn escapes(path: &Path, enclosed: bool, name: &str) -> bool {
    let mut depth: i32 = if enclosed { 1 } else { 0 };
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => depth -= 1,
            std::path::Component::Normal(part) => {
                // The one way up out of the store root and back in.
                if depth == 0 && part != std::ffi::OsStr::new(name) {
                    return true;
                }
                depth += 1;
            }
            _ => {}
        }
        if depth < 0 {
            return true;
        }
    }
    false
}

/// Every `.swync` file at or under a path.
fn modules_under(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return match path.extension() {
            Some(ext) if ext == MODULE_EXTENSION => vec![path.to_path_buf()],
            _ => Vec::new(),
        };
    }

    let Ok(entries) = std::fs::read_dir(path) else { return Vec::new() };
    entries
        .flatten()
        .flat_map(|entry| modules_under(&entry.path()))
        .collect()
}

/// One file, or one folder and everything under it, at `at` inside the pack.
fn add<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    from: &Path,
    at: &Path,
    options: zip::write::SimpleFileOptions,
) -> Result<(), String> {
    // Always `/`, whatever this platform writes: a pack is read on the other
    // machine as often as on this one.
    let name = at
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    if from.is_file() {
        let bytes = std::fs::read(from)
            .map_err(|e| format!("could not read {}: {e}", from.display()))?;
        zip.start_file(&name, options)
            .map_err(|e| format!("could not pack {name}: {e}"))?;
        zip.write_all(&bytes)
            .map_err(|e| format!("could not pack {name}: {e}"))?;
        return Ok(());
    }

    zip.add_directory(&name, options)
        .map_err(|e| format!("could not pack {name}: {e}"))?;

    // Sorted, so the same folder packs to the same bytes twice running — a
    // pack somebody rebuilds should not look like a different pack.
    let mut entries: Vec<PathBuf> = std::fs::read_dir(from)
        .map_err(|e| format!("could not read {}: {e}", from.display()))?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();

    for entry in entries {
        // Hidden files are the machine's, not the library's: a `.DS_Store` is
        // nobody's instrument.
        let entry_name = entry.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if entry_name.starts_with('.') {
            continue;
        }
        add(zip, &entry, &at.join(entry_name), options)?;
    }

    Ok(())
}

fn open(pack: &Path) -> Result<zip::ZipArchive<std::io::BufReader<std::fs::File>>, String> {
    let file = std::fs::File::open(pack)
        .map_err(|e| format!("could not open {}: {e}", name_of(pack)))
        .map_err(|e| match pack.try_exists() {
            Ok(false) => format!("there is no file at {}", pack.display()),
            _ => e,
        })?;
    zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|_| format!("{} is not a library pack", name_of(pack)))
}

/// What to call a pack in a message: its own name, since that is what the
/// person reading it just picked in a dialog.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
