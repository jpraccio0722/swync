// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

use crate::engine::{stop as stop_graph, swap_program};
use crate::{
    scree_graph::realizer::realize,
    lowerer::lower::lower_at_tempo,
    parser::parser::parse,
};
use crate::diagnostic::{Diagnostic, Stage};
use crate::engine::AudioEngine;
use crate::imports::Workspace;
use crate::pattern::graphical::{self, GraphicalPattern};
use crate::pattern::patterns::Patterns;
use crate::project::Project;
use crate::scheduler::clock::Meter;
use crate::scheduler::scheduler::SchedulerState;
use crate::scheduler::voice::Instruments;

use std::sync::Mutex;
use tauri::menu::{IsMenuItem, Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager};

mod pattern;
mod scheduler;
mod diagnostic;
mod engine;
mod files;
mod imports;
mod parser;
mod scree_graph;
mod lowerer;
// Public for `src/bin/dump-metadata.rs`, which writes the same tables the
// editor reads out to the website's reference so the two cannot disagree.
pub mod lang;
mod library;
mod project;
mod recorder;
mod samples;
mod settings;
mod watcher;

/// Backend hook for the editor's "play" button.
///
/// Produces two artifacts from one program: the persistent graph, which is
/// crossfaded into the engine's slot, and the pattern bindings, which go to
/// the scheduler along with the instrument definitions it builds voices from.
///
/// `patterns` are the ones drawn in the side panel. They arrive with the code
/// rather than being held here, because they are part of what is being
/// evaluated: the editor is the only thing that knows them, and an eval is the
/// only moment they matter.
///
/// They reach the program as the project's `patterns.scree`, laid over
/// whatever is on disk: the panel writes that file as you draw, and this makes
/// an eval independent of whether the write has landed — or is even possible,
/// in a folder that cannot be written to. `imports::expand` folds the file into
/// the program, which is the whole of how a drawn pattern gets a name.
///
/// `None` is the panel saying it has nothing to add — it has not read this
/// project yet, or could not — as against `Some([])`, which is a panel that
/// has read the project and found no patterns in it. Only the second one is
/// allowed to hide a file that is on disk.
///
/// `workspace` is what the editor knows and this side cannot: where the file
/// being run lives, so a `use` in it has somewhere to look. Everything a `use`
/// names is then read from disk — a module open in another tab included, which
/// means a change to one is heard once it has been saved.
///
/// A refusal from any of the passes goes back as a `Diagnostic` — which pass,
/// what it said, and where — for the editor's problems panel. Nothing is
/// swapped into the engine until all of them have agreed, so a program that does
/// not compile leaves whatever is playing alone.
#[tauri::command]
fn run_code(
    app: tauri::AppHandle,
    code: String,
    patterns: Option<Vec<GraphicalPattern>>,
    workspace: Workspace,
    engine: tauri::State<Mutex<AudioEngine>>,
    sched: tauri::State<SchedulerState>,
    samples: tauri::State<samples::Cache>,
) -> Result<(), Diagnostic> {
    let mut workspace = workspace;
    if let Some(patterns) = &patterns {
        graphical::check_names(patterns)?;
        workspace.set_patterns(graphical::to_source(patterns));
    }
    // Where a `use` looks once the project has no file by that name. Filled in
    // here because the editor does not know where either store is.
    workspace.set_libraries(library_stores(&app, workspace.root.as_deref()));

    // Imports are resolved into the program before anything else sees it, so
    // the lowerer, the realizer and the scheduler all compile one flat file.
    let ast = imports::expand(parse(code.clone())?, &code, &workspace)?;

    // Files before lowering, and before the scheduler is told anything. A
    // `load` names a path relative to this file, exactly as a `use` does, and
    // this is the only thread allowed to read a disk — a voice is built per
    // note on the scheduler's.
    let loaded = samples::Samples::load(&ast, workspace.dir().as_deref(), &samples)
        .map_err(|e| Diagnostic::message(Stage::Lower, e))?;

    // The tempo and signature the graph is written against. Read here rather
    // than passed along with the origin below, because lowering needs them and
    // the origin is settled after — and read together under one short lock, so
    // that a bar length and the beat it is made of can never come from either
    // side of a change. An eval never holds the engine across a parse or a
    // decode.
    let (beat_secs, meter) = {
        let eng = engine
            .lock()
            .map_err(|_| Diagnostic::message(Stage::Engine, "audio engine poisoned"))?;
        (eng.clock.beat_secs(), eng.clock.meter())
    };

    let lowered = lower_at_tempo(&ast, loaded.clone(), beat_secs, meter)
        .map_err(|e| Diagnostic::message(Stage::Lower, e))?;
    let audio_graph = realize(&lowered.graph)
        .map_err(|e| Diagnostic::message(Stage::Realize, e))?;

    // Instruments before patterns: the scheduler must be able to build a voice
    // for any binding it can see.
    *sched
        .instruments
        .lock()
        .map_err(|_| Diagnostic::message(Stage::Engine, "instruments lock poisoned"))? =
        Instruments::from_program(&ast).with_samples(loaded);

    let starting_from_silence = sched
        .patterns
        .lock()
        .map_err(|_| Diagnostic::message(Stage::Engine, "patterns lock poisoned"))?
        .is_empty();

    // Starting from silence should begin a pattern at its first step, but a
    // re-eval of something already playing must not jolt the groove. The clock
    // moves *before* the patterns are published, so the scheduler can never
    // see the new bindings against the old origin.
    //
    // Whatever bar that leaves us on is the origin the bounded bindings —
    // `play_once`, `playn` — count from, read under the same lock as the reset
    // so the two can never disagree.
    let origin = {
        let eng = engine
            .lock()
            .map_err(|_| Diagnostic::message(Stage::Engine, "audio engine poisoned"))?;
        if starting_from_silence && !lowered.bindings.is_empty() {
            eng.clock.reset();
        }
        eng.clock.now_bars()
    };

    *sched
        .patterns
        .lock()
        .map_err(|_| Diagnostic::message(Stage::Engine, "patterns lock poisoned"))? =
        Patterns { bindings: lowered.bindings, origin, choices: lowered.choices };

    let mut eng = engine
        .lock()
        .map_err(|_| Diagnostic::message(Stage::Engine, "audio engine poisoned"))?;
    swap_program(&mut eng, audio_graph);

    Ok(())
}

/// Backend hook for the editor's "stop" button.
///
/// Silence has three parts, because a program has three places sound can come
/// from: the persistent graph in the engine's slot, the pattern bindings the
/// scheduler keeps turning into voices, and the voices it has already pushed
/// into the lookahead window. Clearing only the first two leaves the last few
/// notes ringing out.
#[tauri::command]
fn stop_audio(
    engine: tauri::State<Mutex<AudioEngine>>,
    sched: tauri::State<SchedulerState>,
) -> Result<(), String> {
    // Bindings first, so the next pass has nothing to schedule, then the flag
    // the scheduler thread reads to cut what it already pushed.
    *sched.patterns.lock().map_err(|_| "patterns lock poisoned")? = Patterns::default();
    sched.request_stop();

    let mut eng = engine.lock().map_err(|_| "audio engine poisoned")?;
    stop_graph(&mut eng);
    // The clock keeps counting through the silence, so without this the next
    // play would rejoin the pattern a little past where it stopped.
    eng.clock.reset();

    Ok(())
}

/// The transport panel's controls, as the frontend shows them: tempo in beats
/// per minute, the signature those beats are barred into, and volume as a
/// linear amplitude between silence and unity.
#[derive(serde::Serialize)]
struct Transport {
    bpm: f64,
    meter: Meter,
    volume: f64,
}

/// What the transport is set to right now, so the panel can open showing the
/// engine's own defaults rather than a guess that drifts away from them.
#[tauri::command]
fn transport(engine: tauri::State<Mutex<AudioEngine>>) -> Result<Transport, String> {
    let eng = engine.lock().map_err(|_| "audio engine poisoned")?;
    Ok(Transport {
        bpm: eng.clock.bpm(),
        meter: eng.clock.meter(),
        volume: eng.master.value() as f64,
    })
}

/// Set the global tempo.
///
/// The clock holds the current beat across the change, so this is safe to call
/// while something is playing — including on every frame of a drag.
#[tauri::command]
fn set_tempo(bpm: f64, engine: tauri::State<Mutex<AudioEngine>>) -> Result<(), String> {
    if !bpm.is_finite() || bpm <= 0.0 {
        return Err(format!("tempo must be above zero, got {bpm}"));
    }
    engine
        .lock()
        .map_err(|_| "audio engine poisoned")?
        .clock
        .set_bpm(bpm);
    Ok(())
}

/// Set the time signature.
///
/// This changes how long a bar is, and therefore what every bare pattern in
/// the program fills and what every `take` and `at` counts — but it does *not*
/// change the tempo, and it does not re-evaluate anything. The graph and the
/// bindings already on the timeline were lowered against the old signature and
/// keep the lengths they were given; play the file again to bring them along.
/// That is the same gap a tempo drag leaves, for the same reason.
#[tauri::command]
fn set_meter(
    top: u32,
    bottom: u32,
    engine: tauri::State<Mutex<AudioEngine>>,
) -> Result<(), String> {
    let meter = Meter { top, bottom };
    if !meter.is_valid() {
        return Err(format!(
            "{meter} is not a signature that can be written: the beat must be a \
             note value — 1, 2, 4, 8, 16 or 32 — and a bar must hold between 1 \
             and 64 of them"
        ));
    }
    engine
        .lock()
        .map_err(|_| "audio engine poisoned")?
        .clock
        .set_meter(meter);
    Ok(())
}

/// Set the master output volume, as a linear amplitude.
///
/// Out-of-range values are clamped rather than refused: a control surface
/// sending 1.0000001 is not an error worth interrupting a performance for.
#[tauri::command]
fn set_master_volume(volume: f64, engine: tauri::State<Mutex<AudioEngine>>) -> Result<(), String> {
    if !volume.is_finite() {
        return Err(format!("volume must be a number, got {volume}"));
    }
    engine
        .lock()
        .map_err(|_| "audio engine poisoned")?
        .master
        .set(volume.clamp(0.0, 1.0) as f32);
    Ok(())
}

/// Write a tab's contents to disk. The frontend picks the path via the save
/// dialog; here we just persist the bytes.
#[tauri::command]
fn save_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

/// Read a file's contents from disk. The frontend picks the path via the open
/// dialog; here we just return the text.
#[tauri::command]
fn read_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// A project's drawn patterns, and the file they came from.
///
/// The path is returned rather than composed on the frontend so there is one
/// answer to where the file is, and the editor can recognise the file when it
/// is opened as an ordinary tab.
#[derive(serde::Serialize, Debug)]
struct PatternsFile {
    path: String,
    patterns: Vec<GraphicalPattern>,
}

/// Read a project's patterns into the panel.
///
/// A project with no such file has no drawn patterns, which is not a failure —
/// it is every new project. A file that does not parse *is* one: the panel must
/// not show an empty grid over a file it could not read, because the next thing
/// it would do is write that empty grid back over it.
#[tauri::command]
fn read_patterns(root: String) -> Result<PatternsFile, Diagnostic> {
    let path = std::path::Path::new(&root).join(graphical::FILE);
    let display = path.display().to_string();

    let patterns = match std::fs::read_to_string(&path) {
        Ok(code) => graphical::from_source(&code).map_err(|e| e.or_in(Some(&path)))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            return Err(Diagnostic::message(
                Stage::Import,
                format!("could not read {display}: {e}"),
            ))
        }
    };

    Ok(PatternsFile { path: display, patterns })
}

/// Write a project's patterns out, and say where they went.
///
/// Called as the panel is drawn in, so the file is what the panel holds. It is
/// only persistence: an eval sends the panel's rows along with the code, so
/// music does not stop for a folder that cannot be written to — the failure
/// shows up in the problems panel instead.
#[tauri::command]
fn write_patterns(root: String, patterns: Vec<GraphicalPattern>) -> Result<String, Diagnostic> {
    graphical::check_names(&patterns)?;

    let path = std::path::Path::new(&root).join(graphical::FILE);
    std::fs::write(&path, graphical::to_source(&patterns)).map_err(|e| {
        Diagnostic::message(
            Stage::Import,
            format!("could not save {}: {e}", path.display()),
        )
    })?;

    Ok(path.display().to_string())
}

/// A project's saved settings, and the file they came from.
///
/// The path is returned rather than composed on the frontend for the same
/// reason `PatternsFile`'s is: one answer to where the file is, and the editor
/// can recognise it when it is opened as an ordinary tab.
#[derive(serde::Serialize, Debug)]
struct ProjectFile {
    path: String,
    project: Project,
}

/// Read a project's settings, and put the transport where they say.
///
/// The engine is moved here rather than by the frontend because this is the
/// moment the file is known to be good: a project that could not be read must
/// not half-apply, leaving the tempo from its file and the volume from the last
/// one. What the editor gets back is what the engine is now set to, so the
/// faders it draws are never a guess.
///
/// A folder with no settings file keeps the transport exactly where it is and
/// is described that way. That is not a fallback — it is the right answer for
/// every folder opened for the first time, and it means picking a project can
/// never move a fader under a performer's hands.
#[tauri::command]
fn open_project(
    root: String,
    engine: tauri::State<Mutex<AudioEngine>>,
) -> Result<ProjectFile, Diagnostic> {
    let root = std::path::Path::new(&root);
    let saved = project::read(root)?;

    let mut eng = engine
        .lock()
        .map_err(|_| Diagnostic::message(Stage::Engine, "audio engine poisoned"))?;

    let project = match saved {
        Some(project) => {
            // The signature before the tempo, and not the other way round:
            // `set_bpm` works out a bar rate from the meter running when it is
            // called, so a 3/4 project opened tempo-first would spend the rest
            // of the session a bar rate short until something moved.
            eng.clock.set_meter(project.meter);
            eng.clock.set_bpm(project.bpm);
            eng.master.set(project.volume as f32);
            project
        }
        None => Project::new(
            root,
            eng.clock.bpm(),
            eng.clock.meter(),
            eng.master.value() as f64,
        ),
    };

    Ok(ProjectFile { path: project::file_path(root).display().to_string(), project })
}

/// Start a project in a folder, which is what File ▸ New Project… does.
///
/// The only difference from opening one is that this writes the file: a folder
/// that already has settings is adopted exactly as `open_project` would adopt
/// it, and one that does not gets the transport as it stands written into a new
/// file. That is the whole of what "new" means here — there is nothing else to
/// create, since a project is still just a folder.
#[tauri::command]
fn create_project(
    root: String,
    engine: tauri::State<Mutex<AudioEngine>>,
) -> Result<ProjectFile, Diagnostic> {
    let file = open_project(root.clone(), engine)?;
    project::write(std::path::Path::new(&root), &file.project)?;
    Ok(file)
}

/// Watch a project's folder, or stop watching when it closes.
///
/// Called with whatever project is open, and again whenever that changes. The
/// tree needs no refresh button once this is running: a file made by anything
/// else — the Finder, a script, a branch being checked out — raises
/// `project-changed`, and the editor reads the folders it is showing again.
///
/// A folder that cannot be watched is worth saying so about, since what it
/// costs is exactly the thing the button used to do: the tree carries on
/// working, and stops noticing anything it did not do itself.
#[tauri::command]
fn watch_project(
    app: tauri::AppHandle,
    root: Option<String>,
    watch: tauri::State<watcher::Watch>,
) -> Result<(), String> {
    let handle = app.clone();
    watch.set(root.as_deref().map(std::path::Path::new), move || {
        let _ = handle.emit(watcher::PROJECT_CHANGED, ());
    })
}

/// The app's own settings — where recordings go, and what they are written as.
///
/// A separate file from a project's, in the app's config directory: see
/// `settings.rs` for why an output folder must not travel with a piece.
#[tauri::command]
fn settings(app: tauri::AppHandle) -> Result<settings::Settings, String> {
    Ok(settings::read(&config_file(&app, settings::FILE)?))
}

/// Remember them. Called as the settings panel is changed, the same way a
/// project's file is written as the transport moves.
#[tauri::command]
fn set_settings(app: tauri::AppHandle, settings: settings::Settings) -> Result<(), String> {
    settings::write(&config_file(&app, settings::FILE)?, &settings)
}

/// What a recording may be written as, for the settings panel's dropdown.
///
/// Served from the recorder's own table rather than written out again in
/// TypeScript, for the same reason the language's builtins are: a format the
/// editor offers and the recorder cannot write would be a promise broken after
/// the take, which is the worst moment to find out.
#[tauri::command]
fn recording_formats() -> Vec<recorder::FormatInfo> {
    recorder::formats()
}

/// Start recording what comes out of the engine.
///
/// The name is made here rather than asked for: a performance is not a save
/// dialog, and a take that had to be named before it could begin is a take
/// that starts several seconds late. `root` is the open project, which is
/// where recordings go when no folder has been chosen, and what the file is
/// named after.
///
/// Everything that can refuse — no folder, a folder that is gone, a name
/// already taken — refuses here, before anything is armed, so a recording that
/// says it started really has.
#[tauri::command]
fn start_recording(
    app: tauri::AppHandle,
    root: Option<String>,
    name: Option<String>,
    recorder: tauri::State<recorder::Recorder>,
) -> Result<String, String> {
    let settings = settings::read(&config_file(&app, settings::FILE)?);
    let dir = settings.recording_dir_for(root.as_deref())?;

    // The project's name if it has one, then the folder's, then a word that is
    // at least not a lie about what the file is.
    let stem = name
        .or_else(|| {
            root.as_deref()
                .and_then(|root| std::path::Path::new(root).file_name()?.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "recording".to_string());

    let path = recorder::unique_path(&dir, &stem, settings.recording_format)?;
    recorder.start(&path, settings.recording_format)
}

/// Stop, and answer with the finished file.
///
/// This waits for the file to be closed rather than returning as soon as the
/// tap is disarmed: the editor is about to say where the recording is, and a
/// recording is not one until its header has been filled in.
#[tauri::command]
fn stop_recording(recorder: tauri::State<recorder::Recorder>) -> Result<recorder::Finished, String> {
    recorder.stop()
}

/// How the take is going.
///
/// Asked for while one is running, which is also how a failure on the writer
/// thread — a disk filling up — reaches the editor: nothing commanded it, so
/// there is no answer for it to travel back in, and it waits here instead.
#[tauri::command]
fn recording_state(recorder: tauri::State<recorder::Recorder>) -> recorder::RecordingState {
    recorder.state()
}

/// Write a project's settings out, and say where they went.
///
/// Called as the tempo, the volume or the name changes. Like the patterns file
/// it is only persistence: the engine has already been told, so a folder that
/// cannot be written to costs you the memory of the setting rather than the
/// setting itself.
#[tauri::command]
fn save_project(root: String, project: Project) -> Result<String, Diagnostic> {
    project::write(std::path::Path::new(&root), &project)
        .map(|path| path.display().to_string())
}

/// Where installed libraries are looked for, nearest first: the project's own
/// copies, then the ones installed on this machine.
///
/// The order is the whole of the precedence rule, and it is written here rather
/// than in the resolver because only this side knows where either folder is —
/// the global one is inside the app's config directory, which the editor has no
/// business knowing the path of.
fn library_stores(app: &tauri::AppHandle, root: Option<&str>) -> Vec<std::path::PathBuf> {
    let mut stores = Vec::new();
    if let Some(root) = root {
        stores.push(library::project_store(std::path::Path::new(root)));
    }
    if let Ok(dir) = app.path().app_config_dir() {
        stores.push(library::store_in(&dir));
    }
    stores
}

/// The global store, made if it is not there yet.
///
/// Installing is the first thing that ever needs the folder to exist, so it is
/// the first thing that makes it.
fn global_store(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map(|dir| library::store_in(&dir))
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Every library this project can reach — its own copies first, then the
/// machine's, and never the same name twice.
///
/// A project copy hides a global one of the same name exactly as it does at a
/// `use`, so the list reads as what a program in this project would actually
/// get.
#[tauri::command]
fn list_libraries(
    app: tauri::AppHandle,
    root: Option<String>,
) -> Result<Vec<library::Library>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut all = Vec::new();

    if let Some(root) = &root {
        let store = library::project_store(std::path::Path::new(root));
        for lib in library::list(&store, library::Source::Project) {
            seen.insert(lib.name.clone());
            all.push(lib);
        }
    }

    if let Ok(dir) = app.path().app_config_dir() {
        for lib in library::list(&library::store_in(&dir), library::Source::Global) {
            if seen.insert(lib.name.clone()) {
                all.push(lib);
            }
        }
    }

    all.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(all)
}

/// Install a pack, for every project on this machine.
///
/// Answers rather than fails when something of that name is already installed,
/// so the editor can say what is there and what is arriving and ask. Nothing
/// has been written when it does.
#[tauri::command]
fn install_library(
    app: tauri::AppHandle,
    pack: String,
    replace: bool,
) -> Result<library::Outcome, String> {
    let store = global_store(&app)?;
    library::install(
        std::path::Path::new(&pack),
        &store,
        library::Source::Global,
        replace,
    )
}

/// Uninstall a library, from whichever store it is in.
#[tauri::command]
fn remove_library(
    app: tauri::AppHandle,
    name: String,
    source: library::Source,
    root: Option<String>,
) -> Result<(), String> {
    let store = match (source, root.as_deref()) {
        (library::Source::Project, Some(root)) => library::project_store(std::path::Path::new(root)),
        (library::Source::Project, None) => {
            return Err(format!("`{name}` belongs to a project, and none is open"))
        }
        (library::Source::Global, _) => global_store(&app)?,
    };
    library::remove(&name, &store)
}

/// What this project would be packed as.
///
/// Reads `scree-library.json`, and writes a seeded one if the project has none
/// — so the editor can name the file it is about to save without asking, and
/// the name lives somewhere it can be corrected.
#[tauri::command]
fn library_manifest(root: String) -> Result<library::Manifest, String> {
    let root = std::path::Path::new(&root);
    let seed = project::read(root)
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_else(|| {
            root.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "library".to_string())
        });
    library::project_manifest(root, &seed)
}

/// Pack a project up as a library.
#[tauri::command]
fn export_library(root: String, dest: String) -> Result<String, String> {
    let root = std::path::Path::new(&root);
    let manifest = library_manifest(root.display().to_string())?;
    library::export(root, &manifest, std::path::Path::new(&dest))
        .map(|path| path.display().to_string())
}

/// Copy a library into the project, so the project carries its own.
#[tauri::command]
fn vendor_library(
    app: tauri::AppHandle,
    name: String,
    root: String,
) -> Result<library::Library, String> {
    let from = global_store(&app)?;
    let to = library::project_store(std::path::Path::new(&root));
    library::vendor(&name, &from, &to)
}

/// One entry in a project directory, as the project panel draws it.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    name: String,
    /// Absolute, so a click can open it without knowing where it came from.
    path: String,
    is_dir: bool,
}

/// What the app remembers between launches, inside its own config folder.
const SESSION: &str = "session.json";

/// The single-line file earlier versions wrote, holding a project and nothing
/// else. Read when there is no session yet, so an upgrade does not forget the
/// project somebody had open.
const RECENT_PROJECT: &str = "recent-project";

/// One tab as the session remembers it: a file by path, or a composer by the
/// name of the pattern it was drawing.
///
/// A composer cannot be remembered by id — ids are minted when a project's
/// patterns are read and mean nothing across a launch — and the name is what
/// the patterns file holds anyway. Exactly one of the two is set; a tab that
/// has never been saved has neither, and is not remembered, because there is
/// nothing on disk to reopen.
#[derive(serde::Serialize, serde::Deserialize, Default, Debug, PartialEq)]
struct SessionTab {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
}

/// What was open last time.
#[derive(serde::Serialize, serde::Deserialize, Default, Debug, PartialEq)]
struct Session {
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    tabs: Vec<SessionTab>,
    /// Which of `tabs` was in front, by position — the tabs have no ids that
    /// outlive a launch either.
    #[serde(default)]
    active: Option<usize>,
}

/// Whether a remembered path is still worth opening.
///
/// A project is nothing more than a folder, so the only thing that can go
/// wrong is that it stopped being one — moved, deleted, or on a volume that is
/// not mounted this time. None of those deserve an error: the app simply opens
/// with no project, which is the same state it is in on a first run.
fn usable_project(saved: &str) -> Option<String> {
    let saved = saved.trim();
    if saved.is_empty() {
        return None;
    }
    std::path::Path::new(saved).is_dir().then(|| saved.to_string())
}

/// What a remembered session is still worth restoring of.
///
/// Everything in one is a path into a filesystem that has moved on since it
/// was written, and none of the ways it can have moved on are errors: a
/// project that is no longer a folder, or a file that has been renamed or
/// deleted, simply does not come back. Filtering here rather than in the
/// frontend keeps `active` meaning what it says — it is a position in `tabs`,
/// so dropping a tab before it has to move it.
fn usable_session(saved: Session) -> Session {
    let project = saved.project.as_deref().and_then(usable_project);

    let was_active = saved.active;
    let mut tabs = Vec::with_capacity(saved.tabs.len());
    let mut active = None;
    for (i, tab) in saved.tabs.into_iter().enumerate() {
        let keep = match (&tab.path, &tab.pattern) {
            (Some(path), _) => std::path::Path::new(path).is_file(),
            // A composer is restored against the project's patterns file, so
            // it only means anything while there is a project to read it from.
            (None, Some(_)) => project.is_some(),
            // Neither, which a tab that was never saved would be. Nothing to
            // reopen, so it was not written in the first place.
            (None, None) => false,
        };
        if !keep {
            continue;
        }
        if was_active == Some(i) {
            active = Some(tabs.len());
        }
        tabs.push(tab);
    }

    Session { project, tabs, active }
}

fn config_file(app: &tauri::AppHandle, name: &str) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(name))
        .map_err(|e| e.to_string())
}

/// What to open on launch: the project last chosen, and the tabs last open,
/// so far as they are all still there.
///
/// The project is deliberately *not* the working directory. A launched
/// binary's cwd says nothing about what someone is working on — under `tauri
/// dev` it is the Rust crate, and a bundled app opened from the desktop gets
/// `/` — so using it meant the app wrote the panel's `patterns.scree` into the
/// source tree in development, and somewhere unwritable in a real build. An
/// empty session is a real answer here: the app is allowed to open with no
/// project and no files, which is what a first run does.
#[tauri::command]
fn recent_session(app: tauri::AppHandle) -> Result<Session, String> {
    let saved = match std::fs::read_to_string(config_file(&app, SESSION)?) {
        // A file half-written by a crash, or written by a version that has
        // since changed the shape, is a first run rather than a failure:
        // there is nothing in one worth refusing to start over.
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        // No session yet. There may still be the older record, which knew
        // only about the project.
        Err(_) => Session {
            project: std::fs::read_to_string(config_file(&app, RECENT_PROJECT)?).ok(),
            ..Session::default()
        },
    };
    Ok(usable_session(saved))
}

/// Remember what is open now, for the next launch.
#[tauri::command]
fn set_recent_session(app: tauri::AppHandle, session: Session) -> Result<(), String> {
    let path = config_file(&app, SESSION)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string(&session).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// List one directory, for the project tree.
///
/// One level deep on purpose: the tree asks again when a folder is opened, so
/// a project with a `target/` or `node_modules/` in it costs nothing until
/// somebody actually looks inside one.
#[tauri::command]
fn list_dir(path: String) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let full = entry.path();
        // Follows symlinks, so a link to a folder opens like the folder.
        let is_dir = full.is_dir();
        // A name or path that isn't text is one the editor could not open
        // either, so it is left out rather than drawn as mojibake.
        if let (Some(name), Some(path)) = (entry.file_name().to_str(), full.to_str()) {
            // A project's `.git` and `.DS_Store` are not part of the project,
            // and the walk behind search already reads the tree this way.
            if name.starts_with('.') {
                continue;
            }
            entries.push(Entry {
                name: name.to_string(),
                path: path.to_string(),
                is_dir,
            });
        }
    }

    // Folders first, then by name, the way a file browser reads.
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(entries)
}

/// The language's callable surface, for the editor's highlighting, completion
/// and signature help.
///
/// Served from the same tables the lowerer dispatches on, so the editor can
/// never offer a name the language does not have.
#[tauri::command]
fn language_metadata() -> lang::LanguageMetadata {
    lang::metadata()
}

/// Every name the file in the editor may write, as running it would resolve
/// them.
///
/// What completion cannot work out for itself. It scrapes the buffer with
/// regexes — a half-written file has no syntax tree to walk, and a round trip
/// per keystroke would be worse than imprecise — and that reaches everything a
/// `use` names *except* what a glob brings in, whose names live in a file the
/// editor has never read. An installed library is that case almost by
/// definition: `use kit::*` is the whole reason to install one.
///
/// So this runs the real expander and reports the spellings it ends up with. It
/// is asked when the file's `use` lines change rather than as it is typed, and
/// it never fails: a file mid-edit expands to nothing, and nothing extra to
/// offer is the right answer while it does.
#[tauri::command]
fn module_symbols(
    app: tauri::AppHandle,
    code: String,
    workspace: Workspace,
) -> Vec<imports::Symbol> {
    let mut workspace = workspace;
    workspace.set_libraries(library_stores(&app, workspace.root.as_deref()));

    match parse(code.clone()) {
        Ok(items) => imports::symbols(items, &code, &workspace),
        Err(_) => Vec::new(),
    }
}

/// Menu item ids, which are also the events the frontend listens for. One
/// constant each so the two can't drift apart.
const MENU_NEW: &str = "file-new";
const MENU_OPEN: &str = "file-open";
const MENU_SAVE: &str = "file-save";
const MENU_NEW_PROJECT: &str = "project-new";
const MENU_OPEN_PROJECT: &str = "project-open";
const MENU_INSTALL_LIBRARY: &str = "library-install";
const MENU_EXPORT_LIBRARY: &str = "library-export";

/// The event a playback failure reaches the editor on.
///
/// Not a command's return value, because nothing asked: the scheduler free-runs
/// on its own thread, and the note that fails to build is being built bars
/// after the eval that bound it. The payload is a `Diagnostic`, the same shape
/// a refused eval comes back as, so the problems panel needs no second reader.
const SCHEDULER_ERROR: &str = "scheduler-error";

/// Every id the File menu can raise. The event handler forwards these and
/// ignores anything else, so the platform's own items keep working.
const FILE_ITEMS: [&str; 7] = [
    MENU_NEW,
    MENU_OPEN,
    MENU_SAVE,
    MENU_NEW_PROJECT,
    MENU_OPEN_PROJECT,
    MENU_INSTALL_LIBRARY,
    MENU_EXPORT_LIBRARY,
];

/// The platform's default menu with New, Open and Save added to the top of
/// File. Each one carries the accelerator the toolbar buttons used to advertise.
///
/// Editing the default rather than rebuilding it keeps every other item —
/// Quit, Copy, Minimise, the lot — exactly where the platform puts it.
fn build_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::default(app)?;

    let new_item = MenuItem::with_id(app, MENU_NEW, "New", true, Some("CmdOrCtrl+N"))?;
    let open_item = MenuItem::with_id(app, MENU_OPEN, "Open…", true, Some("CmdOrCtrl+O"))?;
    let save_item = MenuItem::with_id(app, MENU_SAVE, "Save", true, Some("CmdOrCtrl+S"))?;
    // No accelerators: picking a project is a once-a-session act, and the
    // obvious shortcuts are all spoken for by the file items above.
    let new_project_item =
        MenuItem::with_id(app, MENU_NEW_PROJECT, "New Project…", true, None::<&str>)?;
    let open_project_item =
        MenuItem::with_id(app, MENU_OPEN_PROJECT, "Open Project…", true, None::<&str>)?;
    // Libraries are a rarer act still: installed once and then forgotten about,
    // which is the point of them.
    let install_library_item =
        MenuItem::with_id(app, MENU_INSTALL_LIBRARY, "Install Library…", true, None::<&str>)?;
    let export_library_item =
        MenuItem::with_id(app, MENU_EXPORT_LIBRARY, "Export as Library…", true, None::<&str>)?;

    // Four groups, because there are four kinds of act here: making or fetching
    // a file, choosing which folder the project panel shows, moving a library
    // in or out, and saving. The trailing rule keeps all of it off whatever the
    // platform put in File.
    let items: [&dyn IsMenuItem<tauri::Wry>; 11] = [
        &new_item,
        &open_item,
        &PredefinedMenuItem::separator(app)?,
        &new_project_item,
        &open_project_item,
        &PredefinedMenuItem::separator(app)?,
        &install_library_item,
        &export_library_item,
        &PredefinedMenuItem::separator(app)?,
        &save_item,
        &PredefinedMenuItem::separator(app)?,
    ];

    let file = menu.items()?.into_iter().find_map(|item| match item {
        MenuItemKind::Submenu(s) if s.text().map(|t| t == "File").unwrap_or(false) => Some(s),
        _ => None,
    });

    match file {
        Some(file) => file.insert_items(&items, 0)?,
        // No File submenu on this platform's default: make one. Nothing to sit
        // below it, so the trailing separator goes.
        None => menu.append(&Submenu::with_items(app, "File", true, &items[..7])?)?,
    }

    Ok(menu)
}

#[cfg(test)]
mod list_dir_tests {
    use super::*;

    /// What the tree shows of a folder, and what it leaves out: dotfiles and
    /// dot-folders, which are the project's plumbing rather than the project.
    #[test]
    fn a_listing_leaves_out_hidden_files_and_folders() {
        let root = std::env::temp_dir().join(format!(
            "scree-list-dir-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join(".git")).expect("should create");
        std::fs::create_dir_all(root.join("lib")).expect("should create");
        std::fs::write(root.join("song.scree"), "").expect("should write");
        std::fs::write(root.join(".DS_Store"), "").expect("should write");

        let names: Vec<String> = list_dir(root.display().to_string())
            .expect("should list")
            .into_iter()
            .map(|e| e.name)
            .collect();

        // Folders first, then by name — and neither hidden one at all.
        assert_eq!(names, vec!["lib".to_string(), "song.scree".to_string()]);
        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod pattern_file_tests {
    use super::*;
    use crate::pattern::graphical::{GraphicalStep, StepKind};

    fn temp_project(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "scree-project-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("should create a temp project");
        dir.display().to_string()
    }

    fn rows() -> Vec<GraphicalPattern> {
        vec![GraphicalPattern {
            name: "hats".to_string(),
            steps: vec![
                GraphicalStep::new(StepKind::Trigger),
                GraphicalStep::new(StepKind::Rest),
                GraphicalStep::new(StepKind::Pitch { note: 60.0 }),
            ],
            mode: graphical::Mode::Roll,
        }]
    }

    /// The panel's whole round trip, through the two commands it calls: draw,
    /// save, reopen the project, and find the same rows.
    #[test]
    fn patterns_survive_a_project_being_reopened() {
        let root = temp_project("roundtrip");

        let path = write_patterns(root.clone(), rows()).expect("should write");
        assert!(path.ends_with(graphical::FILE), "wrote {path}");

        let read = read_patterns(root.clone()).expect("should read");
        assert_eq!(read.path, path);
        assert_eq!(read.patterns, rows());

        std::fs::remove_dir_all(&root).ok();
    }

    /// A project nobody has drawn in yet has no patterns, which is not a
    /// failure — it is every new project.
    #[test]
    fn a_project_with_no_file_has_no_patterns() {
        let root = temp_project("empty");
        let read = read_patterns(root.clone()).expect("a missing file is not an error");
        assert!(read.patterns.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    /// One that does not parse is a failure, and says where: the panel needs
    /// to know, because what it does on a successful read is start writing.
    #[test]
    fn an_unreadable_file_is_reported_with_its_path() {
        let root = temp_project("broken");
        std::fs::write(
            std::path::Path::new(&root).join(graphical::FILE),
            "let hats = [\\, `\nfn oops(\n",
        )
        .expect("should write");

        let err = read_patterns(root.clone()).expect_err("should not read");
        assert_eq!(err.file.as_deref(), Some(format!("{root}/{}", graphical::FILE).as_str()));

        std::fs::remove_dir_all(&root).ok();
    }

    /// The file is written as scree, and reads as scree: this is the property
    /// that lets it be included like any other file in the project.
    #[test]
    fn the_written_file_is_a_program() {
        let root = temp_project("program");
        let path = write_patterns(root.clone(), rows()).expect("should write");
        let text = std::fs::read_to_string(&path).expect("should read back");

        let items = parse(text).expect("the panel's file must parse");
        assert!(matches!(items.first(), Some(crate::parser::parser::ScreeItem::Let { .. })));

        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .menu(build_menu)
        .on_menu_event(|app, event| {
            // The work is the frontend's: it owns the tabs, and the dialogs
            // that pick a path. All this side does is relay the click.
            if let Some(id) = FILE_ITEMS.iter().find(|id| event.id() == **id) {
                let _ = app.emit(id, ());
            }
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            run_code,
            stop_audio,
            transport,
            set_tempo,
            set_meter,
            set_master_volume,
            save_file,
            read_file,
            read_patterns,
            write_patterns,
            open_project,
            create_project,
            save_project,
            recent_session,
            set_recent_session,
            list_dir,
            files::create_dir,
            files::create_file,
            files::move_path,
            files::delete_path,
            files::search::search_project,
            list_libraries,
            install_library,
            remove_library,
            vendor_library,
            library_manifest,
            export_library,
            module_symbols,
            language_metadata,
            settings,
            set_settings,
            recording_formats,
            start_recording,
            stop_recording,
            recording_state,
            watch_project
        ])
        .setup(|app| {
            let (engine, seq, tap) = engine::start()?;
            let clock = engine.clock.clone();
            let sched = SchedulerState::new();

            // Before the thread starts, so a failure in its very first pass
            // has somewhere to go.
            let handle = app.handle().clone();
            sched.on_error(move |diagnostic| {
                let _ = handle.emit(SCHEDULER_ERROR, diagnostic);
            });

            // Free-runs for the life of the app; evals only swap what it reads.
            scheduler::scheduler::start(seq, clock, sched.clone());

            app.manage(Mutex::new(engine));
            app.manage(sched);
            // Shares the tap with the audio callback, and is the only thing
            // that ever arms it — the engine renders whether or not anybody is
            // recording, and does not need to know which.
            app.manage(recorder::Recorder::new(tap));
            // Empty until a project is opened, which the editor does on launch
            // as soon as it knows which one that is.
            app.manage(watcher::Watch::default());
            // Outlives every eval: a break named by a program being edited is
            // decoded on the first run and shared by every run after it.
            app.manage(samples::Cache::default());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod session_tests {
    use super::{usable_project, usable_session, Session, SessionTab};

    fn file_tab(path: &str) -> SessionTab {
        SessionTab { path: Some(path.to_string()), pattern: None }
    }

    fn pattern_tab(name: &str) -> SessionTab {
        SessionTab { path: None, pattern: Some(name.to_string()) }
    }

    /// A file that is really there, since that is the only thing a restored
    /// tab is checked against.
    fn a_real_file() -> String {
        let path = std::env::temp_dir().join(format!("scree-session-{}.scree", std::process::id()));
        std::fs::write(&path, "// open").expect("should write");
        path.to_string_lossy().to_string()
    }

    /// A file left where it was comes back, and so does the one that was in
    /// front — which is the whole of what a restored session is.
    #[test]
    fn tabs_that_still_exist_come_back() {
        let here = a_real_file();
        let session = usable_session(Session {
            project: Some(std::env::temp_dir().to_string_lossy().to_string()),
            tabs: vec![file_tab(&here), file_tab(&here)],
            active: Some(1),
        });
        assert_eq!(session.tabs.len(), 2);
        assert_eq!(session.active, Some(1));
        assert!(session.project.is_some());
    }

    /// A file that has moved or been deleted is not an error, and the tab in
    /// front is still the tab that was in front — its position has moved, so
    /// remembering the number rather than the tab would open the wrong one.
    #[test]
    fn a_missing_file_is_dropped_and_the_front_tab_follows() {
        let here = a_real_file();
        let session = usable_session(Session {
            project: None,
            tabs: vec![file_tab("/nowhere/gone.scree"), file_tab(&here)],
            active: Some(1),
        });
        assert_eq!(session.tabs, vec![file_tab(&here)]);
        assert_eq!(session.active, Some(0));
    }

    /// Dropping the tab that was in front leaves no answer to which one is,
    /// rather than an index pointing at somebody else's tab.
    #[test]
    fn dropping_the_front_tab_leaves_none() {
        let here = a_real_file();
        let session = usable_session(Session {
            project: None,
            tabs: vec![file_tab(&here), file_tab("/nowhere/gone.scree")],
            active: Some(1),
        });
        assert_eq!(session.tabs.len(), 1);
        assert_eq!(session.active, None);
    }

    /// A composer draws a pattern out of the project's patterns file, so
    /// without a project there is nothing for it to open on.
    #[test]
    fn composers_need_a_project() {
        let with = usable_session(Session {
            project: Some(std::env::temp_dir().to_string_lossy().to_string()),
            tabs: vec![pattern_tab("hats")],
            active: Some(0),
        });
        assert_eq!(with.tabs, vec![pattern_tab("hats")]);

        let without = usable_session(Session {
            project: Some("/nowhere/that/exists".to_string()),
            tabs: vec![pattern_tab("hats")],
            active: Some(0),
        });
        assert!(without.tabs.is_empty());
        assert_eq!(without.active, None);
    }

    /// An untitled buffer names nothing on disk. It should never be written,
    /// and is not restored if it somehow was.
    #[test]
    fn a_tab_naming_nothing_is_dropped() {
        let session = usable_session(Session {
            project: None,
            tabs: vec![SessionTab::default()],
            active: Some(0),
        });
        assert!(session.tabs.is_empty());
    }

    /// The record earlier versions wrote is a bare path, and is read as one:
    /// an upgrade keeps the project it had, and starts remembering tabs from
    /// the next launch.
    #[test]
    fn an_older_record_still_names_its_project() {
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        let session = usable_session(Session {
            project: Some(format!("{dir}\n")),
            ..Session::default()
        });
        assert_eq!(session.project, Some(dir));
        assert!(session.tabs.is_empty());
    }

    /// A remembered folder that is still a folder is the one to open.
    #[test]
    fn an_existing_folder_is_usable() {
        let dir = std::env::temp_dir();
        let saved = dir.to_string_lossy().to_string();
        assert_eq!(usable_project(&saved), Some(saved.clone()));
    }

    /// Trailing whitespace is the file's, not the path's — it is written with
    /// whatever the OS left on the line.
    #[test]
    fn surrounding_whitespace_is_ignored() {
        let dir = std::env::temp_dir();
        let saved = format!("  {}\n", dir.to_string_lossy());
        assert_eq!(usable_project(&saved), Some(dir.to_string_lossy().to_string()));
    }

    /// A folder that has moved, been deleted, or is on an unmounted volume is
    /// not an error — the app just opens with no project, as on a first run.
    #[test]
    fn a_missing_folder_is_no_project() {
        assert_eq!(usable_project("/nowhere/that/exists/at/all"), None);
    }

    /// A file is not a project. Only folders are.
    #[test]
    fn a_file_is_not_a_project() {
        let file = std::env::temp_dir().join("scree-usable-project-test");
        std::fs::write(&file, "x").expect("should write");
        assert_eq!(usable_project(&file.to_string_lossy()), None);
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn an_empty_record_is_no_project() {
        assert_eq!(usable_project(""), None);
        assert_eq!(usable_project("   \n"), None);
    }
}
