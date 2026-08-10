//! `use` — one file's definitions, named in another.
//!
//! Rust's spelling, and close to Rust's meaning. A path names a file relative
//! to the file the `use` is written in — `use lib::drums` is `lib/drums.swync`
//! beside it — and what comes back are that file's definitions, under the name
//! the `use` gives them:
//!
//! ```text
//! use drums                   // drums::kick, drums::snare
//! use drums as d              // d::kick
//! use drums::kick             // kick
//! use drums::{kick, snare}    // both, unqualified
//! use drums::*                // everything, unqualified
//! ```
//!
//! A `use` does not run the file it names. A module's own top-level expressions
//! — the `play`s that make it a piece rather than a library — stay where they
//! are; only its `fn`s and `let`s travel. This is the one place swync differs
//! from what a Rust reader would guess, and it is the same difference Rust
//! makes between an item and a statement.
//!
//! One import is written for you. The project's `patterns.swync` — what the
//! side panel draws into — is folded into every file that runs in the project,
//! as if it began with `use patterns::*`. That is the whole of how a drawn
//! pattern reaches a program, and it is why a drawn pattern is a `let` in a
//! file rather than a value smuggled into the environment.
//!
//! All of it happens between parsing and lowering, and produces an ordinary
//! `Vec<SwyncItem>`. By the time the lowerer or the scheduler sees a program
//! there are no modules left in it — only definitions with longer names, which
//! is why `play(pat, drums::kick)` needs nothing of either of them.

mod rename;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::diagnostic::{Diagnostic, Stage};
use crate::imports::rename::Scope;
use crate::parser::parser::{parse, SwyncItem, Use, UseName, UseTree};

/// The extension a module file carries. A `use` path never writes it.
pub const EXTENSION: &str = "swync";

/// Where module files are read from.
///
/// A trait so the resolver can be tested against a map of files rather than a
/// directory of them. In the app there is only ever one implementation, and it
/// reads the disk: a `use` names a file, and a file is what is saved.
pub trait Sources {
    /// The text of a file, or `None` if there is no such file.
    fn read(&self, path: &Path) -> Option<String>;
}

/// What the editor knows that the backend cannot: where the file being run
/// lives, and what the panel has drawn.
///
/// Everything a `use` reaches is read from disk, including a file open in
/// another tab — so a change to a module is heard once it is saved, and not
/// before. The one exception is the drawn patterns, which are the panel's and
/// have no other home; see [`Workspace::set_patterns`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// Absolute path of the file being run, if it has ever been saved.
    pub path: Option<String>,
    /// The project folder — where `use` resolves from when the file being run
    /// has no path of its own.
    pub root: Option<String>,
    /// The panel's rows, written as the patterns file would read. Never sent
    /// by the editor as text: it sends the rows, and `run_code` writes them.
    #[serde(skip)]
    patterns: Option<String>,
    /// The library stores this program may reach, nearest first. Filled in by
    /// `run_code` rather than sent: one of them is inside the app's own config
    /// folder, which the editor has no business knowing the path of.
    #[serde(skip)]
    libraries: Vec<PathBuf>,
}

impl Workspace {
    /// One file, in one project. What the editor sends, and what anything else
    /// that compiles a file has to say for itself.
    pub fn new(path: Option<String>, root: Option<String>) -> Workspace {
        Workspace { path, root, patterns: None, libraries: Vec::new() }
    }

    /// The file being run, when it is a file.
    pub fn path(&self) -> Option<PathBuf> {
        self.path.as_ref().map(PathBuf::from)
    }

    /// The folder `use` paths in the program itself are relative to: the file's
    /// own, or the project's while it has never been saved.
    pub fn dir(&self) -> Option<PathBuf> {
        match self.path() {
            Some(path) => path.parent().map(Path::to_path_buf),
            None => self.root.as_ref().map(PathBuf::from),
        }
    }

    /// Where the drawn patterns live: the project's folder, so every file in
    /// the project shares one set of them however deep it sits.
    ///
    /// Falls back to the file's own folder, and then to a bare name — which
    /// only ever resolves through the panel's own rows, since there is nowhere
    /// on disk it could mean.
    pub fn patterns_path(&self) -> PathBuf {
        let dir = self
            .root
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| self.path().and_then(|p| p.parent().map(Path::to_path_buf)))
            .unwrap_or_default();
        dir.join(crate::pattern::graphical::FILE)
    }

    /// Read the patterns file as the panel currently has it, rather than as it
    /// was last written.
    ///
    /// The one thing an eval does not take from disk, because the panel is the
    /// only place it exists: the file is written as you draw, and waiting for
    /// that write would make a folder that cannot be written to a folder you
    /// cannot play in.
    pub fn set_patterns(&mut self, source: String) {
        self.patterns = Some(source);
    }

    /// The folders an installed library's modules are looked for in, after the
    /// project has been asked and had nothing — nearest first, so a copy the
    /// project carries beats one installed on the machine.
    pub fn set_libraries(&mut self, stores: Vec<PathBuf>) {
        self.libraries = stores;
    }
}

impl Sources for Workspace {
    fn read(&self, path: &Path) -> Option<String> {
        if let Some(patterns) = &self.patterns {
            if path == self.patterns_path() {
                return Some(patterns.clone());
            }
        }
        std::fs::read_to_string(path).ok()
    }
}

/// Replace every `use` in a program with the definitions it names.
///
/// The result is the program the rest of the pipeline compiles: same items, in
/// the same order, with each module's definitions ahead of the file that asked
/// for them.
pub fn expand(
    items: Vec<SwyncItem>,
    code: &str,
    ws: &Workspace,
) -> Result<Vec<SwyncItem>, Diagnostic> {
    let wants_files = items.iter().any(|i| matches!(i, SwyncItem::Use(_)));

    // The project's drawn patterns, which every file in it can name without
    // asking. Not for the patterns file itself, which would be importing
    // itself, and not when there are none — a project with an empty panel and
    // no such file is just a project.
    let patterns = ws.patterns_path();
    let implicit = (ws.path().as_deref() != Some(patterns.as_path())
        && ws.read(&patterns).is_some())
    .then_some(patterns);

    // A program that needs neither never touches the filesystem, and so never
    // needs a folder to touch it in: an unsaved scratch buffer stays runnable.
    if !wants_files && implicit.is_none() {
        return Ok(items);
    }

    let dir = match (ws.dir(), &implicit) {
        (Some(dir), _) => dir,
        // A `use` is written by hand and has to resolve somewhere; the
        // implicit one already knows where it is.
        (None, _) if wants_files => {
            return Err(Diagnostic::message(
                Stage::Import,
                "`use` looks for files next to this one, and this one has never been \
                 saved — save it, or open a project folder first",
            ))
        }
        (None, Some(patterns)) => patterns.parent().unwrap_or(Path::new("")).to_path_buf(),
        (None, None) => unreachable!("returned above when neither wants a file"),
    };

    expand_from(
        items,
        code,
        ws,
        &dir,
        ws.path().as_deref(),
        implicit.as_deref(),
        &ws.libraries,
    )
}

/// `expand`, against any set of files.
///
/// Split out for the tests, which have no directory to be in: everything the
/// resolver does with the world it does through `Sources`.
fn expand_from(
    items: Vec<SwyncItem>,
    code: &str,
    src: &dyn Sources,
    dir: &Path,
    path: Option<&Path>,
    implicit: Option<&Path>,
    libraries: &[PathBuf],
) -> Result<Vec<SwyncItem>, Diagnostic> {
    expand_scoped(items, code, src, dir, path, implicit, libraries).map(|(items, _)| items)
}

/// `expand_from`, keeping what the entry file's own scope turned out to be.
///
/// The program is what runs; the scope is what the *editor* wants — see
/// [`symbols`]. Both fall out of the same pass, and doing it twice would be
/// two answers where there must only ever be one.
#[allow(clippy::type_complexity)]
fn expand_scoped(
    items: Vec<SwyncItem>,
    code: &str,
    src: &dyn Sources,
    dir: &Path,
    path: Option<&Path>,
    implicit: Option<&Path>,
    libraries: &[PathBuf],
) -> Result<(Vec<SwyncItem>, Scoped), Diagnostic> {
    let mut resolver = Resolver::new(src, dir.to_path_buf(), libraries.to_vec());
    // The file being run goes on the stack before anything it imports, so a
    // module that imports it back is a cycle rather than a second copy.
    if let Some(path) = path {
        resolver.loading.push(path.to_path_buf());
    }

    // The implicit import is written into the file's scope before its own
    // `use`s are read, so an explicit import of the same names wins — as does
    // the file's own definition of one, like any other import.
    let mut seed = HashMap::new();
    let mut modules = HashMap::new();
    if let Some(patterns) = implicit {
        resolver.load(patterns)?;
        let module = resolver.modules.get(patterns).expect("just loaded");
        // Qualified as well as bare, so `patterns::hats` says which `hats` is
        // meant when the file has one of its own.
        let alias = Path::new(crate::pattern::graphical::FILE)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| module.prefix.clone());
        modules.insert(alias, module.prefix.clone());
        seed.extend(module.exports.iter().map(|(k, v)| (k.clone(), v.clone())));
    }

    let expanded = resolver.file(items, code, path, dir, None, seed, modules)?;
    let scope = Scoped { names: expanded.names, modules: expanded.modules };

    let mut program = resolver.out;
    program.extend(expanded.items);
    Ok((program, scope))
}

/// What the entry file may write, and what each spelling means.
struct Scoped {
    /// Written name → the name the program files it under. Everything the file
    /// imported unqualified, and everything it defines.
    names: HashMap<String, String>,
    /// Module alias → the prefix that module's definitions carry, for the
    /// qualified spellings the file may also write.
    modules: HashMap<String, String>,
}

/// One name the file being run may write, and what writing it would reach.
///
/// The editor's question rather than the compiler's. Completion has to offer
/// the spelling *this file* uses — `kick` after `use kit::*`, `kit::kick` after
/// `use kit`, `k::kick` after `use kit as k` — and the map between a spelling
/// and a definition is built by expansion and nowhere else. Scraping the `use`
/// lines with a regex answers three of those four and cannot answer the glob at
/// all, because the names it introduces live in a file the editor has not read.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Symbol {
    /// As this file writes it.
    pub name: String,
    /// A `fn`'s parameters, in order. Empty for anything that is not one.
    pub params: Vec<String>,
    /// Which of those have a default, and so may be left out — which is also
    /// what makes one usable as a `play` lane.
    pub optional: Vec<bool>,
    /// True for a `fn`: something a `play` could name as an instrument.
    pub callable: bool,
}

/// Every name a file may write, resolved exactly as running it would resolve.
pub fn symbols(items: Vec<SwyncItem>, code: &str, ws: &Workspace) -> Vec<Symbol> {
    let Some(dir) = ws.dir() else { return Vec::new() };
    let patterns = ws.patterns_path();
    let implicit = (ws.path().as_deref() != Some(patterns.as_path())
        && ws.read(&patterns).is_some())
    .then_some(patterns);

    let expanded = expand_scoped(
        items,
        code,
        ws,
        &dir,
        ws.path().as_deref(),
        implicit.as_deref(),
        &ws.libraries,
    );
    // A file half-written is a file that does not expand, and that is most of
    // the time this is asked. Nothing to offer is the right answer: the editor
    // still has what it scraped from the buffer itself.
    let Ok((program, scope)) = expanded else { return Vec::new() };

    let mut defined: HashMap<&str, (Vec<String>, Vec<bool>, bool)> = HashMap::new();
    for item in &program {
        match item {
            SwyncItem::Function { name, params, .. } => {
                defined.insert(
                    name.0.as_str(),
                    (
                        params.iter().map(|p| p.name.0.clone()).collect(),
                        params.iter().map(|p| p.default.is_some()).collect(),
                        true,
                    ),
                );
            }
            SwyncItem::Let { name, .. } => {
                defined.insert(name.0.as_str(), (Vec::new(), Vec::new(), false));
            }
            _ => {}
        }
    }

    // Every spelling this file may write, paired with what it reaches.
    let mut spellings: Vec<(String, &str)> = scope
        .names
        .iter()
        .map(|(written, filed)| (written.clone(), filed.as_str()))
        .collect();
    // The qualified ones, which are every definition of a module the file named
    // without unpacking.
    for (alias, prefix) in &scope.modules {
        let head = format!("{prefix}::");
        for filed in defined.keys() {
            if let Some(tail) = filed.strip_prefix(&head) {
                spellings.push((format!("{alias}::{tail}"), filed));
            }
        }
    }

    let mut found: Vec<Symbol> = spellings
        .into_iter()
        .filter_map(|(written, filed)| {
            let (params, optional, callable) = defined.get(filed)?;
            Some(Symbol {
                name: written,
                params: params.clone(),
                optional: optional.clone(),
                callable: *callable,
            })
        })
        .collect();

    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|a, b| a.name == b.name);
    found
}

/// One file, expanded: what it contributes, what it lends out, and what it
/// could write while it was being read.
struct Expanded {
    items: Vec<SwyncItem>,
    /// Name as this file writes it → name it is filed under here.
    exports: HashMap<String, String>,
    names: HashMap<String, String>,
    modules: HashMap<String, String>,
}

/// A module that has been read: what its definitions are called now, and what
/// they were called in it.
struct Module {
    /// What every one of its definitions is prefixed with.
    prefix: String,
    /// Name as the module writes it → name it is filed under here.
    exports: HashMap<String, String>,
}

/// What a `use` takes from the file it resolved to.
enum Take {
    /// The whole module, reachable as `alias::name`.
    Module(String),
    /// Named items, unqualified.
    Names(Vec<UseName>),
    /// Every item, unqualified.
    Glob,
}

struct Resolver<'a> {
    src: &'a dyn Sources,
    /// The folder the program being run is in. Prefixes are measured from
    /// here, so a module's qualified name reads like its path.
    base: PathBuf,
    /// Every module read so far, by path — read once however many files import
    /// it, so its definitions appear in the program exactly once.
    modules: HashMap<PathBuf, Module>,
    /// The chain of files currently being read, for reporting a cycle as the
    /// route that closed it.
    loading: Vec<PathBuf>,
    /// Prefixes handed out, so two modules can never share one.
    prefixes: HashSet<String>,
    /// Folders holding installed libraries, asked in order once the project
    /// itself has no file by the name a `use` writes.
    libraries: Vec<PathBuf>,
    /// Every module's definitions, in the order they must be defined:
    /// a module is always finished before the file that imported it.
    out: Vec<SwyncItem>,
}

impl<'a> Resolver<'a> {
    fn new(src: &'a dyn Sources, base: PathBuf, libraries: Vec<PathBuf>) -> Resolver<'a> {
        Resolver {
            src,
            base,
            modules: HashMap::new(),
            loading: Vec::new(),
            prefixes: HashSet::new(),
            libraries,
            out: Vec::new(),
        }
    }

    /// Expand one file's items against one file's imports.
    ///
    /// `prefix` is what makes it a module rather than the program: its
    /// definitions are filed under that name, and its top-level expressions are
    /// dropped rather than played.
    ///
    /// `names` and `modules` start the file's scope off with what it did not
    /// have to ask for — the project's drawn patterns. A file's own `use`s are
    /// read on top of them, and its own definitions on top of those, so both
    /// beat an implicit import the way they beat any other.
    ///
    /// Returns the file's items, renamed, what it exports, and the scope it
    /// ended up with — which is the editor's business rather than the
    /// compiler's, and is thrown away everywhere but [`expand_scoped`].
    fn file(
        &mut self,
        items: Vec<SwyncItem>,
        code: &str,
        path: Option<&Path>,
        dir: &Path,
        prefix: Option<&str>,
        mut names: HashMap<String, String>,
        mut modules: HashMap<String, String>,
    ) -> Result<Expanded, Diagnostic> {
        let mut uses = Vec::new();
        let mut rest = Vec::new();

        for item in items {
            match item {
                SwyncItem::Use(u) => uses.push(u),
                // A module lends out its definitions, not its performance.
                SwyncItem::Expr(_) | SwyncItem::Call { .. } if prefix.is_some() => {}
                other => rest.push(other),
            }
        }

        for u in &uses {
            self.one_use(u, dir, &mut names, &mut modules)
                .map_err(|d| d.or_at(code, u.at).or_in(path))?;
        }

        // A file's own definitions win over everything it imported: a `use`
        // brings a name in, it never takes one away.
        let exports = own_names(&rest, prefix);
        names.extend(exports.iter().map(|(k, v)| (k.clone(), v.clone())));

        let mut scope = Scope::new(&names, &modules);
        // A module's `load` paths move with its definitions, so they are made
        // absolute here, while the folder they were written in is still known.
        // The program's own stay as written; nothing has moved them, and
        // `samples` resolves them against this same folder anyway.
        if prefix.is_some() {
            scope = scope.relocating_samples(dir);
        }
        let mut renamed = Vec::with_capacity(rest.len());
        for mut item in rest {
            scope
                .item(&mut item)
                .map_err(|m| Diagnostic::message(Stage::Import, m).or_in(path))?;
            define_as(&mut item, &exports);
            renamed.push(item);
        }

        Ok(Expanded { items: renamed, exports, names, modules })
    }

    /// Resolve one `use`, reading the module it names if nothing has yet, and
    /// record what it brings into scope.
    fn one_use(
        &mut self,
        u: &Use,
        dir: &Path,
        names: &mut HashMap<String, String>,
        modules: &mut HashMap<String, String>,
    ) -> Result<(), Diagnostic> {
        let (file, take) = self.locate(u, dir)?;
        self.load(&file)?;
        let module = self.modules.get(&file).expect("just loaded");

        match take {
            Take::Module(alias) => {
                modules.insert(alias, module.prefix.clone());
            }
            Take::Glob => {
                names.extend(module.exports.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            Take::Names(list) => {
                for wanted in list {
                    let Some(filed) = module.exports.get(&wanted.name.0) else {
                        return Err(Diagnostic::message(
                            Stage::Import,
                            format!(
                                "`{}` has no `{}`{}",
                                u.written(),
                                wanted.name.0,
                                did_you_mean(&wanted.name.0, module),
                            ),
                        ));
                    };
                    let local = wanted.alias.unwrap_or(wanted.name);
                    names.insert(local.0, filed.clone());
                }
            }
        }

        Ok(())
    }

    /// Which file a `use` means, and what it takes from it.
    ///
    /// Asked of the folder the `use` was written in first, and then of each
    /// library store in turn — each one answered in full before the next is
    /// asked, so a file in the project always beats an installed library of the
    /// same name. Installing something can then never change what a project
    /// that already worked means.
    fn locate(&self, u: &Use, dir: &Path) -> Result<(PathBuf, Take), Diagnostic> {
        let mut looked = Vec::new();
        for root in std::iter::once(dir).chain(self.libraries.iter().map(PathBuf::as_path)) {
            if let Some(found) = self.locate_in(u, root, &mut looked) {
                return Ok(found);
            }
        }
        Err(no_module(u, &looked))
    }

    /// The same question, of one folder.
    ///
    /// `use a::b::c` is the one shape that has to be looked up rather than
    /// read: `c` is a module in `a/b/` if there is such a file, and an item of
    /// `a/b.swync` otherwise. Rust resolves the same ambiguity the same way,
    /// by which one exists.
    ///
    /// Every path tried goes into `looked`, which is what a failure quotes
    /// back — so a `use` that finds nothing says where it went.
    fn locate_in(
        &self,
        u: &Use,
        root: &Path,
        looked: &mut Vec<PathBuf>,
    ) -> Option<(PathBuf, Take)> {
        let whole = module_file(root, &u.path);
        if self.exists(&whole) {
            let take = match &u.tree {
                UseTree::Names(list) => Take::Names(list.clone()),
                UseTree::Glob => Take::Glob,
                UseTree::Plain(alias) => {
                    let last = u.path.last().expect("a path has a last segment");
                    Take::Module(alias.clone().unwrap_or_else(|| last.clone()).0)
                }
            };
            return Some((whole, take));
        }
        looked.push(whole);

        // Not a file, so a plain path's last segment must be something inside
        // the file the rest of it names.
        let UseTree::Plain(alias) = &u.tree else { return None };
        if u.path.len() < 2 {
            return None;
        }

        let parent = module_file(root, &u.path[..u.path.len() - 1]);
        if self.exists(&parent) {
            let name = u.path.last().expect("checked above").clone();
            let take = Take::Names(vec![UseName { name, alias: alias.clone() }]);
            return Some((parent, take));
        }
        looked.push(parent);

        None
    }

    fn exists(&self, path: &Path) -> bool {
        self.modules.contains_key(path) || self.src.read(path).is_some()
    }

    /// Read a module, and everything it imports, into the program.
    fn load(&mut self, path: &Path) -> Result<(), Diagnostic> {
        if self.modules.contains_key(path) {
            return Ok(());
        }

        if let Some(start) = self.loading.iter().position(|p| p == path) {
            let mut chain: Vec<String> = self.loading[start..]
                .iter()
                .map(|p| file_name(p))
                .collect();
            chain.push(file_name(path));
            return Err(Diagnostic::message(
                Stage::Import,
                format!("circular import: {}", chain.join(" → ")),
            ));
        }

        let Some(code) = self.src.read(path) else {
            return Err(Diagnostic::message(
                Stage::Import,
                format!("could not read {}", path.display()),
            ));
        };

        let items = parse(code.clone()).map_err(|d| d.or_in(Some(path)))?;

        // Claimed before the recursion, so a module cannot take the prefix of
        // one that is still being read.
        let prefix = self.claim_prefix(path);
        let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();

        self.loading.push(path.to_path_buf());
        // A module is given nothing it did not ask for — not even the drawn
        // patterns. It is a file somebody else's project may import, and a
        // library that depends on what is in *this* panel would not survive
        // the trip.
        let expanded = self.file(
            items,
            &code,
            Some(path),
            &dir,
            Some(&prefix),
            HashMap::new(),
            HashMap::new(),
        );
        self.loading.pop();
        let Expanded { items, exports, .. } = expanded?;

        // After its own imports, which are already in `out`: a module is
        // defined only once everything it stands on is.
        self.out.extend(items);
        self.modules
            .insert(path.to_path_buf(), Module { prefix, exports });

        Ok(())
    }

    /// The name a module's definitions are filed under: its path below the
    /// folder the program is in, which is how it is written at the `use`.
    fn claim_prefix(&mut self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.base).ok().map(Path::to_path_buf);

        let wanted = match relative {
            Some(relative) => relative
                .with_extension("")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("::"),
            // Outside the program's folder, so there is no path to read it as.
            None => path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "module".to_string()),
        };

        // Two files can want the same prefix — one above the program's folder
        // and one in it, say. Whoever asks second gets a number, and neither
        // name is ever written by hand.
        let mut candidate = wanted.clone();
        let mut n = 2;
        while !self.prefixes.insert(candidate.clone()) {
            candidate = format!("{wanted}#{n}");
            n += 1;
        }
        candidate
    }
}

/// `dir/a/b/c.swync`, for the path `a::b::c`.
///
/// Shared with the explorer, which has to answer the same question in reverse:
/// a file that has been dragged somewhere else needs the path that reaches it
/// from where it is now, and the two must agree about what a path means.
pub(crate) fn module_file(dir: &Path, path: &[crate::parser::parser::Ident]) -> PathBuf {
    let mut file = dir.to_path_buf();
    for segment in path {
        file.push(&segment.0);
    }
    file.set_extension(EXTENSION);
    file
}

/// What a file defines, and what each of those definitions is called in the
/// program: itself, or the module's name and itself.
fn own_names(items: &[SwyncItem], prefix: Option<&str>) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for item in items {
        let own = match item {
            SwyncItem::Function { name, .. } | SwyncItem::Let { name, .. } => &name.0,
            _ => continue,
        };
        let filed = match prefix {
            Some(prefix) => format!("{prefix}::{own}"),
            None => own.clone(),
        };
        names.insert(own.clone(), filed);
    }
    names
}

/// File a definition under the name the program knows it by.
fn define_as(item: &mut SwyncItem, names: &HashMap<String, String>) {
    let name = match item {
        SwyncItem::Function { name, .. } | SwyncItem::Let { name, .. } => name,
        _ => return,
    };
    if let Some(filed) = names.get(&name.0) {
        name.0 = filed.clone();
    }
}

/// The last component of a path, for a message that is about the route between
/// files rather than about where they are.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn no_module(u: &Use, looked_in: &[PathBuf]) -> Diagnostic {
    let looked = looked_in
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" or ");
    Diagnostic::message(
        Stage::Import,
        format!("no module `{}` — looked for {looked}", u.written()),
    )
}

/// The nearest name the module does export, when there is an obvious one.
///
/// One typo's worth of difference, deliberately: a loose guess in a message
/// like this sends the reader looking in the wrong place, which is worse than
/// not guessing.
fn did_you_mean(wanted: &str, module: &Module) -> String {
    let mut near: Vec<&String> = module
        .exports
        .keys()
        .filter(|name| one_edit_apart(wanted, name))
        .collect();
    near.sort();

    match near.first() {
        Some(name) => format!(" — did you mean `{name}`?"),
        None => String::new(),
    }
}

/// Whether two names differ by a single character: one inserted, one dropped,
/// or one replaced. Case counts as a replacement, which is how `Kick` finds
/// `kick`.
fn one_edit_apart(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    // The shorter one first, so the only cases below are "same length" and
    // "one longer".
    let (short, long) = if a.len() <= b.len() { (&a, &b) } else { (&b, &a) };
    if long.len() - short.len() > 1 {
        return false;
    }

    let mut i = 0;
    let mut j = 0;
    let mut edits = 0;
    while i < short.len() && j < long.len() {
        if short[i] == long[j] {
            i += 1;
            j += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return false;
        }
        // Same length is a replacement, so both advance; otherwise the extra
        // character is the long one's, and only it does.
        if short.len() == long.len() {
            i += 1;
        }
        j += 1;
    }

    // Anything left over is the trailing character of the longer name.
    edits + (long.len() - j) + (short.len() - i) <= 1
}
