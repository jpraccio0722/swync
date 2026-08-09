import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import CodeMirror from "@uiw/react-codemirror";
import type { EditorView } from "@codemirror/view";
import {
  screeExtensions,
  revealPosition,
  showErrorLines,
  EMPTY_METADATA,
  loadMetadata,
  Symbols,
  type LanguageMetadata,
} from "./scree";
import { DocsPanel, type DocsFocus } from "./DocsPanel";
import { LibrariesPanel } from "./LibrariesPanel";
import { ProblemsPanel, type RunStatus } from "./ProblemsPanel";
import { ProjectPanel } from "./ProjectPanel";
import { isWithin } from "./projectTree";
import { PatternComposer } from "./PatternComposer";
import { RightPanel, type RightTab } from "./RightPanel";
import {
  SettingsPanel,
  type FinishedRecording,
  type RecordingFormat,
  type RecordingState,
  type Settings,
} from "./SettingsPanel";
import { SearchPanel } from "./SearchPanel";
import { SidePanel, type SideTab } from "./SidePanel";
import { toDiagnostic, type Diagnostic } from "./diagnostics";
import { TransportPanel } from "./TransportPanel";
import {
  fromWire,
  nameError,
  toWire,
  type GraphicalPattern,
  type WirePattern,
} from "./patterns";
import { Transport, type Meter, type TransportState } from "./component/Transport";


/** A project's drawn patterns, as `read_patterns` returns them. */
interface PatternsFile {
  /** Absolute path of the project's `patterns.scree`. */
  path: string;
  patterns: WirePattern[];
}

/**
 * What a project remembers between sessions, as its `scree-project.json` holds
 * it. The key order matters: it is what the two sides are compared as JSON in,
 * to tell a change worth saving from a file that already says this.
 */
interface ProjectSettings {
  name: string;
  /** Beats per minute. */
  bpm: number;
  /** The time signature, which is how long a bar is. */
  meter: Meter;
  /** Linear amplitude, 0 to 1. */
  volume: number;
}

/** A project's settings, as `open_project` returns them. */
interface ProjectFile {
  /** Absolute path of the project's `scree-project.json`. */
  path: string;
  project: ProjectSettings;
}

/** How long the panel waits before writing a change out. Long enough that
 *  dragging across a row is one write, short enough to be over before anyone
 *  reaches for the play key. */
const PATTERN_SAVE_DELAY = 400;
/** The same for the project's settings, where the drag is a fader rather than
 *  a row of cells. */
const PROJECT_SAVE_DELAY = 400;

/**
 * How long a take goes on recording after the engine has been silenced.
 *
 * `stop_audio` crossfades the graph out over 0.2s and cuts the voices the
 * scheduler had pushed (see `engine.rs`), so the last fifth of a second of any
 * performance is the fade itself. A file closed the moment stop was pressed
 * would end mid-waveform at whatever level was sounding, which is a click on
 * the end of every recording — and would cut the release off every note that
 * was still ringing.
 */
const FADE_OUT_DELAY = 250;

/** How long the app waits before writing the session out. Switching tabs and
 *  closing them come in bursts, and none of it is worth a write each. */
const SESSION_SAVE_DELAY = 400;

interface Tab {
  id: string;
  /** Display name in the tab bar. */
  title: string;
  /** Absolute path on disk, or null if the tab has never been saved. */
  path: string | null;
  content: string;
  /** True when content differs from what's on disk. */
  dirty: boolean;
  /**
   * The drawn pattern this tab edits, if it is a composer rather than a file.
   *
   * A composer is a tab because it is a thing you work *in*, beside the code
   * that plays it — and because the alternative, a modal or a wider side
   * panel, makes you choose between seeing the pattern and seeing the program.
   * Patterns live in the project rather than in the tab, so this is an id
   * rather than the pattern itself: a rename in the composer has to reach the
   * list in the panel, and both have to reach the file.
   */
  patternId?: string;
  /**
   * The pattern a restored composer is still waiting for, by name.
   *
   * A session cannot remember an id — ids are minted when a project's patterns
   * are read, and mean nothing across a launch — so a restored composer opens
   * holding the name from the file and takes its id once that read lands. See
   * the effect that resolves these.
   */
  patternName?: string;
}

/** One tab as `recent_session` remembers it: a file by path, or a composer by
 *  the name of the pattern it draws. */
interface SessionTab {
  path: string | null;
  pattern: string | null;
}

/** What was open when the app last closed. */
interface Session {
  project: string | null;
  tabs: SessionTab[];
  /** Which of `tabs` was in front, by position. */
  active: number | null;
}

/** True for a tab holding code, as against a composer — including one that is
 *  still waiting for its pattern, which has no buffer to run either. */
function isCode(tab: Tab): boolean {
  return !tab.patternId && !tab.patternName;
}

const SCREE_FILTER = [{ name: "scree", extensions: ["scree"] }];

/** What a library pack is called on disk, in both dialogs that name one. */
const PACK_EXTENSION = "screepack";

/** What `install_library` answers with: what it did, or what it would be
 *  overwriting if it went ahead. Nothing has been written when it is the
 *  second. */
type InstallOutcome =
  | { kind: "installed" }
  | { kind: "conflict"; name: string; installed: string; incoming: string };

/** What the project says it is, when it is packed as a library. */
interface LibraryManifest {
  name: string;
  version: string;
}

/** How wide either side panel may be dragged. The floor is what a pattern's
 *  grid and the sliders need; the ceiling keeps the editor from vanishing. */
const MIN_PANEL = 240;
const MAX_PANEL = 720;
const DEFAULT_PANEL = 288;
/** The left panel holds wrapped prose, a snippet and a file tree, all of which
 *  want a little more room than the transport's sliders do. */
const DEFAULT_SIDE_PANEL = 340;

/**
 * Drag a panel's inner edge.
 *
 * Pointer capture is what makes this reliable: the pointer leaves the 8px
 * handle on the first frame of any real drag, and without capture the moves
 * would be delivered to whatever it crossed — including the editor, which
 * would start selecting text.
 *
 * @param edge Which side of the window the panel is anchored to, which decides
 * whether its width grows or shrinks as the pointer moves right.
 */
function usePanelResize(edge: "left" | "right", setWidth: (width: number) => void) {
  return useCallback(
    (e: React.PointerEvent) => {
      e.preventDefault();
      const handle = e.currentTarget as HTMLElement;
      handle.setPointerCapture(e.pointerId);

      const onMove = (ev: PointerEvent) => {
        const width =
          edge === "left" ? ev.clientX : window.innerWidth - ev.clientX;
        setWidth(Math.min(MAX_PANEL, Math.max(MIN_PANEL, width)));
      };
      const onUp = () => {
        handle.removeEventListener("pointermove", onMove);
        handle.removeEventListener("pointerup", onUp);
        handle.removeEventListener("pointercancel", onUp);
      };

      handle.addEventListener("pointermove", onMove);
      handle.addEventListener("pointerup", onUp);
      handle.addEventListener("pointercancel", onUp);
    },
    [edge, setWidth],
  );
}

const STARTER_CONTENT = `// Write some code, then hit Play (or ⌘,)`;

let tabCounter = 0;
function makeTab(overrides: Partial<Tab> = {}): Tab {
  tabCounter += 1;
  return {
    id: `tab-${tabCounter}`,
    title: `untitled-${tabCounter}.scree`,
    path: null,
    content: "",
    dirty: false,
    ...overrides,
  };
}

/** Extract the file name from an absolute path (cross-platform). */
function basename(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function App() {
  // Empty until the last session has been read, which is a moment rather than
  // a state anyone sees: opening on a starter buffer and then replacing it
  // would flash a file that was never theirs, and — worse — the writer below
  // would have a window in which to save that buffer over the real session.
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [restoring, setRestoring] = useState(true);
  // Which file tab was last in front, so a composer can hand an eval back to
  // it. A ref rather than state: nothing renders from it.
  const lastCodeId = useRef<string | null>(null);
  const [panelOpen, setPanelOpen] = useState(true);
  const [panelWidth, setPanelWidth] = useState(DEFAULT_PANEL);
  // The right panel opens on the transport: it is the tab with the buttons the
  // app is played from. The reference is always beside it, and a ⌘-click in
  // the editor is what usually brings it up.
  const [panelTab, setPanelTab] = useState<RightTab>("transport");
  const [docsFocus, setDocsFocus] = useState<DocsFocus | null>(null);

  // The left panel starts shut, on its problems tab: there is nothing to say
  // until something has been run. A failed run opens it, which is the whole
  // point of it.
  const [sideOpen, setSideOpen] = useState(false);
  const [sideWidth, setSideWidth] = useState(DEFAULT_SIDE_PANEL);
  const [sideTab, setSideTab] = useState<SideTab>("project");
  const [diagnostics, setDiagnostics] = useState<Diagnostic[]>([]);
  const [runStatus, setRunStatus] = useState<RunStatus>("idle");
  // Whether the engine is holding a program, which is what lights the play
  // button. Not the same question as `runStatus`, which is about the last
  // run's diagnostics: a refused eval leaves whatever is playing alone, so it
  // is perfectly ordinary to be playing and in error at the same time.
  //
  // Tracked here rather than asked of the engine because the two ends of it
  // are both this side's doing — an eval that landed, and a stop — and a poll
  // would only be able to answer the different question of whether anything is
  // audible, which a program that builds silence is not either way.
  const [playing, setPlaying] = useState(false);
  // Which tab the diagnostics describe. They outlive the run, and the editor
  // may well have moved to another file by the time they are read.
  const [sourceTabId, setSourceTabId] = useState<string | null>(null);

  // Drawn patterns belong to the project rather than to any one tab: they live
  // in its `patterns.scree`, which every file in the project can name without
  // importing. The panel is that file's editor — it reads it when a project
  // opens and writes it as you draw.
  const [patterns, setPatterns] = useState<GraphicalPattern[]>([]);
  // Where that file is, as the backend composes it. Kept so the panel can
  // recognise the file when it is opened as an ordinary tab.
  const [patternsPath, setPatternsPath] = useState<string | null>(null);
  // The project whose patterns the panel is currently holding, and what that
  // project's file says. Together they are the guard on writing: the panel
  // never writes to a project it has not successfully read, so a file it could
  // not parse is left alone rather than overwritten with an empty grid.
  const patternsFrom = useRef<string | null>(null);
  const patternsOnDisk = useRef<string | null>(null);

  // A project is a folder and nothing else. It opens on whichever one was last
  // chosen and follows whatever File ▸ New Project… picks after that.
  //
  // Null is a real state, not just "the backend has not answered yet": a first
  // run has no remembered folder, and a remembered one may since have moved.
  // Everything that reads a project guards for it, and the patterns panel says
  // so rather than letting unsaved work look saved.
  const [projectRoot, setProjectRoot] = useState<string | null>(null);
  // Bumped to read the tree's open folders again. The backend watches the
  // project's folder and says when something in it changed, so this is raised
  // by the world as well as by the app: see the watch below.
  const [projectVersion, setProjectVersion] = useState(0);
  // The same, for what is installed. Bumped by the File menu's Install…, which
  // lands outside the panel that lists them.
  const [libraryVersion, setLibraryVersion] = useState(0);

  // The engine's own controls, held up here rather than in the title bar
  // because they are half of what a project remembers: opening one puts them
  // where its file says, and moving either writes that file. Null until
  // something has said where they sit — the engine on launch, or a project.
  const [transport, setTransport] = useState<TransportState | null>(null);
  // How many beats the drawn-pattern grids should rule a bar into. Four until
  // the transport has been read, which is what every project is in until its
  // file says otherwise — and a guide drawn a moment early in the wrong meter
  // is worse than one that is right from the first paint.
  const beatsPerBar = transport?.meter.top ?? 4;
  // What the project is called, which is the panel's to show and to edit. Null
  // while no project is open, and again after one that could not be read.
  const [projectName, setProjectName] = useState<string | null>(null);
  // Where the settings file is, so the panel can recognise it when it is opened
  // as an ordinary tab.
  const [projectPath, setProjectPath] = useState<string | null>(null);
  // The same guard the patterns keep, for the same reason: the settings are
  // only written back to a project they were successfully read from, so a file
  // nobody could parse is left for a person to fix rather than overwritten.
  const projectFrom = useRef<string | null>(null);
  const projectOnDisk = useRef<string | null>(null);

  // What the app itself is set to — where recordings go, and what they are
  // written as. The app's, not the project's: an output folder is a path into
  // one machine's disk and would mean nothing in a project handed to somebody
  // else. Null until the backend has answered, which the panel draws as
  // nothing rather than as a folder that may turn out to be wrong.
  const [settings, setSettings] = useState<Settings | null>(null);
  // Every format the recorder can write, from its own table. Fetched rather
  // than listed here so the dropdown can never offer one it cannot produce.
  const [formats, setFormats] = useState<RecordingFormat[]>([]);
  // The take that is running, and how long it has been. Null is not recording,
  // which is almost always.
  const [recording, setRecording] = useState<{ path: string; seconds: number } | null>(
    null,
  );
  // The last one that finished, so the settings panel can say where it went.
  // A performance you cannot find afterwards is one you did not record.
  const [lastRecording, setLastRecording] = useState<FinishedRecording | null>(null);

  const startResize = usePanelResize("right", setPanelWidth);
  const startSideResize = usePanelResize("left", setSideWidth);

  // Null when every tab has been closed, which the editor is built to show.
  const activeTab = tabs.find((t) => t.id === activeId) ?? null;
  if (activeTab && isCode(activeTab)) lastCodeId.current = activeTab.id;

  /**
   * The tab an eval runs. A composer holds no code, so playing from one runs
   * the last file that was open instead of an empty buffer — drawing a pattern
   * and pressing play should be heard, and the alternative is switching tabs
   * every time to hear what you just drew.
   */
  const codeTab =
    activeTab && isCode(activeTab)
      ? activeTab
      : (tabs.find((t) => t.id === lastCodeId.current && isCode(t)) ??
         tabs.find(isCode) ??
         null);

  /** Show a set of failures. Everything that can fail comes through here or
   *  through `report`, so nothing can fail quietly — which is what this panel
   *  exists to prevent. */
  const reportAll = useCallback((diagnostics: Diagnostic[], tabId: string | null) => {
    if (diagnostics.length === 0) return;
    setDiagnostics(diagnostics);
    setRunStatus("error");
    setSourceTabId(tabId);
    // Both, since the panel may well be open on the project tree instead: an
    // open panel showing something else is as silent as a shut one.
    setSideOpen(true);
    setSideTab("problems");
  }, []);

  /** Show one failure, which is what almost everything has. */
  const report = useCallback(
    (diagnostic: Diagnostic, tabId: string | null) => reportAll([diagnostic], tabId),
    [reportAll],
  );

  // The language's builtins, fetched once. Until it arrives the editor runs on
  // EMPTY_METADATA: syntax highlighting is already correct, only the builtin
  // colouring and completions are missing.
  const [metadata, setMetadata] = useState<LanguageMetadata>(EMPTY_METADATA);
  useEffect(() => {
    let live = true;
    loadMetadata()
      .then((meta) => {
        if (live) setMetadata(meta);
      })
      .catch((e) => console.error("could not load language metadata:", e));
    return () => {
      live = false;
    };
  }, []);

  // Where the transport sits, fetched once from the engine so the faders open
  // on its real defaults rather than a guess that drifts away from them.
  //
  // Never over a project's own settings: opening a project moves the engine to
  // what its file says, and if that has already happened this answer is the
  // older news, however it was ordered.
  useEffect(() => {
    let live = true;
    invoke<TransportState>("transport")
      .then((t) => {
        if (live) setTransport((current) => current ?? t);
      })
      .catch((e) => console.error("could not read the transport:", e));
    return () => {
      live = false;
    };
  }, []);

  /**
   * Watch whatever project is open, and read the tree again when it changes.
   *
   * This is what replaced the panel's refresh button. A file made by anything
   * else — dropped in from the Finder, written by a script, arriving with a
   * branch — used to be invisible until somebody thought to press it, which is
   * precisely the case where nobody knows to.
   *
   * The listener is set up once and the watch follows the project, so the two
   * are separate effects: re-subscribing on every project change would leave a
   * window in which a change is not heard, and there is nothing to gain by it.
   *
   * A folder that cannot be watched is logged rather than shown. What it costs
   * is what the button used to do, and interrupting somebody's work to say
   * "you will have to reopen the project to see new files" is worse than the
   * quiet the alternative leaves.
   */
  useEffect(() => {
    const subscription = listen("project-changed", () =>
      setProjectVersion((v) => v + 1),
    );
    return () => void subscription.then((unlisten) => unlisten());
  }, []);

  useEffect(() => {
    invoke("watch_project", { root: projectRoot }).catch((e) =>
      console.error("could not watch the project folder:", e),
    );
  }, [projectRoot]);

  // The app's settings and the formats a recording may be written as, fetched
  // once. Neither changes without this window changing it, so there is nothing
  // to watch for afterwards.
  useEffect(() => {
    let live = true;
    invoke<Settings>("settings")
      .then((s) => {
        if (live) setSettings(s);
      })
      // Logged rather than shown: what it costs is a panel that opens on the
      // defaults, and a first run has no file to read anyway.
      .catch((e) => console.error("could not read the app's settings:", e));
    invoke<RecordingFormat[]>("recording_formats")
      .then((f) => {
        if (live) setFormats(f);
      })
      .catch((e) => console.error("could not read the recording formats:", e));
    return () => {
      live = false;
    };
  }, []);

  /** Change a setting: the panel shows it at once, and it is remembered. */
  const changeSettings = useCallback(
    (next: Settings) => {
      setSettings(next);
      invoke("set_settings", { settings: next }).catch((e) =>
        // Like a project's file: the setting itself has already taken effect,
        // so what a failure costs is the memory of it rather than the setting.
        report(toDiagnostic(e, "could not save the settings"), null),
      );
    },
    [report],
  );

  // What was open last time, fetched once. Restoring a file tab is reading the
  // file again — nothing about a buffer is kept, so a restored tab is what is
  // on disk and says so by opening clean.
  //
  // A composer is restored by name against the project's patterns, which are
  // read separately and land after this: those tabs open holding a name, and
  // the effect below gives them their pattern when it arrives.
  useEffect(() => {
    let live = true;

    void (async () => {
      let session: Session = { project: null, tabs: [], active: null };
      try {
        session = await invoke<Session>("recent_session");
      } catch (e) {
        // A session that cannot be read costs this launch its tabs and
        // nothing else, so it is logged rather than shown: the app opens as
        // it would on a first run.
        console.error("could not read the last session:", e);
      }
      if (!live) return;

      // Before the tabs, so the patterns panel starts its own read as early as
      // possible — the composers below are waiting on it.
      if (session.project) setProjectRoot(session.project);

      const restored: Tab[] = [];
      let active: string | null = null;
      for (const [i, record] of session.tabs.entries()) {
        let tab: Tab | null = null;
        if (record.path) {
          try {
            const content = await invoke<string>("read_file", { path: record.path });
            tab = makeTab({ title: basename(record.path), path: record.path, content });
          } catch (e) {
            // Unreadable since — a permission change, or something that is no
            // longer text. Nothing has been lost: the file is still there, and
            // the open dialog will say why if it is asked again.
            console.error(`could not reopen ${record.path}:`, e);
          }
        } else if (record.pattern) {
          tab = makeTab({ title: record.pattern, patternName: record.pattern });
        }
        if (!tab) continue;
        if (session.active === i) active = tab.id;
        restored.push(tab);
      }
      if (!live) return;

      // A first run, or a session whose files have all gone: open on an empty
      // buffer, which is what the app has always started with.
      const opened = restored.length > 0 ? restored : [makeTab({ content: STARTER_CONTENT })];
      setTabs(opened);
      setActiveId(active ?? opened[0].id);
      setRestoring(false);
    })();

    return () => {
      live = false;
    };
  }, []);

  // Which project the panel's rows belong to, for the load below to check
  // against when it finally answers: opening two projects quickly must not
  // leave the first one's answer on screen.
  const projectRootRef = useRef(projectRoot);
  projectRootRef.current = projectRoot;

  /**
   * Read a project's drawn patterns into the panel.
   *
   * A project with no patterns file simply has none. A file that cannot be
   * read is a failure worth showing, and it also stops the panel writing:
   * showing an empty grid over a file we could not parse, and then saving that
   * grid over it, is how somebody's work disappears.
   */
  const loadPatterns = useCallback(
    async (root: string) => {
      try {
        const file = await invoke<PatternsFile>("read_patterns", { root });
        if (projectRootRef.current !== root) return; // the project moved on
        setPatternsPath(file.path);
        // Through the setter so the rows already on screen are in reach: a
        // row this file also names keeps its id, and the composer tab holding
        // it does not lose its pattern. See `fromWire`.
        //
        // Only for the same project, though. Two projects may each have a
        // `hats`, and they are not the same row — carrying an id across would
        // point an open composer at the other project's pattern.
        setPatterns((held) =>
          fromWire(file.patterns, patternsFrom.current === root ? held : []),
        );
        patternsOnDisk.current = JSON.stringify(file.patterns);
        patternsFrom.current = root;
      } catch (e) {
        if (projectRootRef.current !== root) return;
        // Cleared rather than left showing: rows from another project would
        // be played by this one, since an eval sends whatever the panel holds.
        setPatterns([]);
        patternsFrom.current = null;
        report(toDiagnostic(e, "could not read this project's patterns"), null);
      }
    },
    [report],
  );

  // On open, and again whenever the project tree is refreshed — which is the
  // way to pick up a patterns file changed outside the app.
  useEffect(() => {
    if (projectRoot === null) return;
    void loadPatterns(projectRoot);
  }, [projectRoot, projectVersion, loadPatterns]);

  // And back out again as the panel is drawn in. Debounced, because dragging
  // across a row is a dozen state changes and one edit.
  useEffect(() => {
    if (projectRoot === null || patternsFrom.current !== projectRoot) return;

    const wire = toWire(patterns);
    const json = JSON.stringify(wire);
    if (json === patternsOnDisk.current) return; // the file already says this

    const timer = setTimeout(() => {
      invoke<string>("write_patterns", { root: projectRoot, patterns: wire })
        .then((path) => {
          patternsOnDisk.current = json;
          setPatternsPath(path);
        })
        // Left un-synced on purpose, so the next change tries again. The music
        // is unaffected: an eval sends the panel's rows with the code.
        .catch((e) => report(toDiagnostic(e, "could not save the patterns"), null));
    }, PATTERN_SAVE_DELAY);

    return () => clearTimeout(timer);
  }, [patterns, projectRoot, report]);

  // A restored composer arrives holding a name and nothing else. The id it
  // needs exists only once the project's patterns have been read, so it is
  // handed over here — and if the pattern is not in that file, the tab goes:
  // it was an editor for something that has since been deleted.
  useEffect(() => {
    if (!tabs.some((t) => t.patternName)) return;
    // Only once the panel has actually read *this* project. Before that the
    // list is empty because nothing has been read, which is not the same as a
    // project whose patterns file no longer names these rows.
    if (projectRoot === null || patternsFrom.current !== projectRoot) return;

    const next = tabs.flatMap((tab) => {
      if (!tab.patternName) return [tab];
      const pattern = patterns.find((p) => p.name === tab.patternName);
      if (!pattern) return [];
      return [{ ...tab, patternName: undefined, patternId: pattern.id, title: pattern.name }];
    });
    setTabs(next);
    // Dropping the tab that was in front leaves the editor pointing at
    // nothing, so it falls back to a neighbour the way closing one does.
    if (!next.some((t) => t.id === activeId)) {
      setActiveId(next.length === 0 ? null : next[next.length - 1].id);
    }
  }, [tabs, patterns, projectRoot, activeId]);

  // What the session file already says, so an app that is only being clicked
  // around in does not rewrite it.
  const sessionOnDisk = useRef<string | null>(null);

  // And back out again as tabs are opened, closed, switched and saved, so the
  // next launch opens on this. Debounced for the same reason the patterns file
  // is: closing four tabs is one session, not four.
  useEffect(() => {
    // Nothing to remember before the last session has been read — and an empty
    // list written here would be it, forgetting everything.
    if (restoring) return;

    const records: SessionTab[] = [];
    let active: number | null = null;
    for (const tab of tabs) {
      const pattern = tab.patternId
        ? (patterns.find((p) => p.id === tab.patternId)?.name ?? null)
        : (tab.patternName ?? null);
      // A tab that has never been saved names nothing on disk, so there is
      // nothing to reopen: its buffer lives here and only here.
      if (!tab.path && !pattern) continue;
      if (tab.id === activeId) active = records.length;
      records.push({ path: pattern ? null : tab.path, pattern });
    }

    const session: Session = { project: projectRoot, tabs: records, active };
    const json = JSON.stringify(session);
    if (json === sessionOnDisk.current) return;

    const timer = setTimeout(() => {
      invoke("set_recent_session", { session })
        .then(() => {
          sessionOnDisk.current = json;
        })
        // Costs the next launch its tabs and nothing this one, so it is logged
        // rather than shown — and left un-synced, so the next change retries.
        .catch((e) => console.error("could not remember this session:", e));
    }, SESSION_SAVE_DELAY);

    return () => clearTimeout(timer);
  }, [tabs, activeId, projectRoot, patterns, restoring]);

  /**
   * Read a project's settings, and take on what they say.
   *
   * The tempo and volume are the backend's to apply — it does so as it answers,
   * so the engine and the faders drawn here can never disagree about a project
   * that has just opened. A folder with no settings file keeps the transport
   * where it is and is described that way, which is what makes opening one safe
   * mid-performance.
   *
   * A file that cannot be read is worth showing, and it also stops the writing
   * below: settings are small, but they are somebody's, and an unparseable file
   * is one to fix rather than to flatten.
   */
  const loadProject = useCallback(
    async (root: string) => {
      try {
        const file = await invoke<ProjectFile>("open_project", { root });
        if (projectRootRef.current !== root) return; // the project moved on
        setProjectPath(file.path);
        setProjectName(file.project.name);
        setTransport({
          bpm: file.project.bpm,
          meter: file.project.meter,
          volume: file.project.volume,
        });
        projectOnDisk.current = JSON.stringify(file.project);
        projectFrom.current = root;
      } catch (e) {
        if (projectRootRef.current !== root) return;
        // The name goes, since it described another project. The transport
        // stays where it is: it is the engine's, and nothing about a file that
        // would not parse is a reason to move what is playing.
        setProjectName(null);
        projectFrom.current = null;
        report(toDiagnostic(e, "could not read this project's settings"), null);
      }
    },
    [report],
  );

  // On open, and again whenever the project is refreshed — which is how a file
  // edited outside the app is picked up.
  useEffect(() => {
    if (projectRoot === null) return;
    void loadProject(projectRoot);
  }, [projectRoot, projectVersion, loadProject]);

  // And back out again as any of it changes. Debounced like the patterns: a
  // fader dragged across its travel is one write, not sixty.
  useEffect(() => {
    if (projectRoot === null || projectFrom.current !== projectRoot) return;
    if (projectName === null || transport === null) return;

    // Built in the backend's field order, so the comparison below is against
    // the same JSON the file was read as.
    const settings: ProjectSettings = {
      name: projectName,
      bpm: transport.bpm,
      meter: transport.meter,
      volume: transport.volume,
    };
    const json = JSON.stringify(settings);
    if (json === projectOnDisk.current) return; // the file already says this

    const timer = setTimeout(() => {
      invoke<string>("save_project", { root: projectRoot, project: settings })
        .then((path) => {
          projectOnDisk.current = json;
          setProjectPath(path);
        })
        // Left un-synced on purpose, so the next change tries again. Nothing is
        // lost this session — the engine has already been told.
        .catch((e) =>
          report(toDiagnostic(e, "could not save this project's settings"), null),
        );
    }, PROJECT_SAVE_DELAY);

    return () => clearTimeout(timer);
  }, [projectName, transport, projectRoot, report]);

  /**
   * A file or folder has been renamed or dragged somewhere else in the tree.
   *
   * Open tabs follow it. A tab is a file, and it does not stop being the same
   * file because the tree beside it moved — losing your place, or saving over
   * the path it used to have, are both worse than any amount of bookkeeping.
   *
   * The project's own two files are re-read when the move touched either of
   * them, because both have an editor here as well as a path: the patterns
   * panel and the transport are showing what those files said, and after a
   * rename that is no longer what they say.
   */
  const onMoved = useCallback(
    (from: string, to: string) => {
      setTabs((prev) =>
        prev.map((tab) => {
          if (tab.path === null || !isWithin(tab.path, from)) return tab;
          const path = to + tab.path.slice(from.length);
          return { ...tab, path, title: basename(path) };
        }),
      );

      // Either end: a patterns file renamed away, or an ordinary file renamed
      // into the name the project reads its patterns from.
      const touched = (path: string | null) =>
        path !== null && (isWithin(path, from) || path === to);

      if (projectRoot !== null && touched(patternsPath)) void loadPatterns(projectRoot);
      if (projectRoot !== null && touched(projectPath)) void loadProject(projectRoot);
    },
    [projectRoot, patternsPath, projectPath, loadPatterns, loadProject],
  );

  /**
   * A file or folder has gone to the trash.
   *
   * Its tabs stay open, and are marked dirty. The buffer is the last copy of
   * that work in the app, and closing the tab would throw it away on top of a
   * delete the user may well have meant for the file on disk and not for what
   * they had been typing — ⌘S writes it back out.
   */
  const onDeleted = useCallback(
    (path: string) => {
      setTabs((prev) =>
        prev.map((tab) =>
          tab.path !== null && isWithin(tab.path, path) ? { ...tab, dirty: true } : tab,
        ),
      );
      if (projectRoot !== null && patternsPath !== null && isWithin(patternsPath, path)) {
        void loadPatterns(projectRoot);
      }
      if (projectRoot !== null && projectPath !== null && isWithin(projectPath, path)) {
        void loadProject(projectRoot);
      }
    },
    [projectRoot, patternsPath, projectPath, loadPatterns, loadProject],
  );

  // The completion source reads the drawn patterns through a ref so a rename
  // in the panel shows up in the next completion without touching the
  // extension array — whose identity must stay stable, since CodeMirror
  // reconfigures whenever it changes.
  const patternsRef = useRef(patterns);
  patternsRef.current = patterns;
  const patternNames = useCallback(
    () => toWire(patternsRef.current).map((p) => p.name),
    [],
  );

  /**
   * Show a builtin in the reference, from a ⌘-click on its name in the editor.
   *
   * Opens the panel as well as switching it: a panel that is shut is as silent
   * as one showing the transport, and the click asked to read something.
   *
   * Identity-stable, like `patternNames` and for the same reason — it is built
   * into the extension array, which CodeMirror reconfigures from.
   */
  const openDocs = useCallback((name: string) => {
    setPanelOpen(true);
    setPanelTab("docs");
    // The nonce is what makes a second click on the same name scroll to it
    // again, after it has been scrolled away from.
    setDocsFocus((prev) => ({ name, nonce: (prev?.nonce ?? 0) + 1 }));
  }, []);

  // What the active file's `use` lines bring in, which completion needs and
  // cannot work out for itself. One for the life of the app, like the
  // extension array it goes into: it is a cache the backend fills, and
  // rebuilding it would throw the answer away on every render.
  const symbols = useRef(new Symbols()).current;
  useEffect(() => {
    symbols.setWorkspace({ path: activeTab?.path ?? null, root: projectRoot });
  }, [symbols, activeTab?.path, projectRoot]);

  const extensions = useMemo(
    () => screeExtensions(metadata, patternNames, openDocs, symbols),
    [metadata, patternNames, openDocs, symbols],
  );

  /**
   * Show a drawn pattern in a composer tab.
   *
   * One tab per pattern: opening the same one twice focuses what is already
   * there, because two composers over one pattern would each hold a view of it
   * that the other could invalidate.
   */
  const openComposer = useCallback(
    (patternId: string) => {
      // The tab is minted out here rather than inside the `setTabs` updater.
      // An updater has to be pure — StrictMode runs it twice — and this one
      // both bumped a counter and set the active id, so the tab that survived
      // the second run was never the one that got focus.
      const existing = tabs.find((t) => t.patternId === patternId);
      if (existing) {
        setActiveId(existing.id);
        return;
      }
      const tab = makeTab({ patternId, title: "pattern" });
      setTabs((prev) => [...prev, tab]);
      setActiveId(tab.id);
    },
    [tabs],
  );

  const newTab = useCallback(() => {
    const tab = makeTab();
    setTabs((prev) => [...prev, tab]);
    setActiveId(tab.id);
  }, []);

  const closeTab = useCallback(
    (id: string) => {
      const idx = tabs.findIndex((t) => t.id === id);
      if (idx === -1) return;

      const next = tabs.filter((t) => t.id !== id);
      setTabs(next);

      // Closing the active tab hands the editor to a neighbour; closing the
      // last one leaves nothing to hand it to, and the empty state shows.
      if (id === activeId) {
        setActiveId(next.length === 0 ? null : next[Math.min(idx, next.length - 1)].id);
      }
    },
    [tabs, activeId],
  );

  const updateContent = useCallback((id: string, content: string) => {
    setTabs((prev) =>
      prev.map((t) =>
        t.id === id ? { ...t, content, dirty: true } : t,
      ),
    );
  }, []);

  /** Open a file by path, wherever the path came from — the open dialog, or a
   *  click in the project tree. Answers whether there is now a tab in front for
   *  it, so a caller with something to do in that tab knows not to do it in
   *  someone else's. */
  const openPath = useCallback(
    async (path: string): Promise<boolean> => {
      // If the file is already open, just focus its tab.
      const existing = tabs.find((t) => t.path === path);
      if (existing) {
        setActiveId(existing.id);
        return true;
      }

      try {
        const content = await invoke<string>("read_file", { path });
        const tab = makeTab({ title: basename(path), path, content, dirty: false });
        setTabs((prev) => [...prev, tab]);
        setActiveId(tab.id);
        return true;
      } catch (e) {
        // Anything that isn't text lands here, which is the honest answer: the
        // editor has nothing to show for a binary file.
        report(toDiagnostic(e, `could not open ${basename(path)}`), null);
        return false;
      }
    },
    [tabs, report],
  );

  /** Set when a click has opened a file meaning to work in it, and cleared once
   *  that file's editor exists to take the keyboard. */
  const [pendingFocus, setPendingFocus] = useState(false);

  /** Where the cursor is on its way to, from a click in the problems panel or
   *  the search results. Pending because both may have to open a tab first, and
   *  the view for that tab does not exist until it has mounted — so the jump is
   *  held here and made once there is somewhere to jump in. */
  const [pendingReveal, setPendingReveal] = useState<{
    line: number | null;
    column: number | null;
  } | null>(null);

  /**
   * Open the project's patterns file from the panel's heading.
   *
   * `openPath` already brings the tab to the front, but the click leaves the
   * keyboard on the heading — and a file you opened in order to edit should be
   * one you can start typing in. The focus waits for the open the way the
   * diagnostic jump does: a tab that has just been added has no editor yet.
   */
  const openPatternsFile = useCallback(
    async (path: string) => {
      if (await openPath(path)) setPendingFocus(true);
    },
    [openPath],
  );

  /**
   * Open a file at a position, which is what a search result is a link to.
   *
   * The focus goes with it: clicking a result is asking to be at that line,
   * not to look at it from the panel. Both wait for the tab the way the
   * diagnostic jump does — a tab that has just been added has no editor yet.
   */
  const openAt = useCallback(
    async (path: string, line: number, column: number) => {
      if (!(await openPath(path))) return;
      setPendingReveal({ line, column });
      setPendingFocus(true);
    },
    [openPath],
  );

  const openTab = useCallback(async () => {
    try {
      const selected = await open({ multiple: false, filters: SCREE_FILTER });
      if (!selected || typeof selected !== "string") return; // user cancelled
      await openPath(selected);
    } catch (e) {
      report(toDiagnostic(e, "could not open that file"), null);
    }
  }, [openPath, report]);

  /**
   * Point the app at a folder, however the folder was chosen.
   *
   * Open tabs are left alone: they are files, and a file does not stop being
   * open because the tree beside it moved. Everything that belongs to the
   * project — its patterns, its settings — is read by the effects above, which
   * this sets going.
   */
  const chooseProject = useCallback((root: string) => {
    setProjectRoot(root);
    setProjectVersion((v) => v + 1);
    setSideOpen(true);
    setSideTab("project");
    // Remembering it for the next launch is the session writer's job — this is
    // a change to what is open, like any other.
  }, []);

  /**
   * Start a project in a folder.
   *
   * The platform's own dialog is where a new folder gets made, so this is a
   * folder chooser and then one write: the `scree-project.json` that is the
   * whole difference between a folder and a project. It is written with the
   * transport as it stands, since picking a folder should not change what is
   * playing.
   */
  const newProject = useCallback(async () => {
    try {
      const selected = await open({ directory: true, multiple: false });
      if (!selected || typeof selected !== "string") return; // user cancelled
      try {
        await invoke("create_project", { root: selected });
      } catch (e) {
        // Opened anyway: a folder that cannot be written to is still one you
        // can work in, and this is worth saying rather than refusing over.
        report(toDiagnostic(e, "could not start a project there"), null);
      }
      chooseProject(selected);
    } catch (e) {
      report(toDiagnostic(e, "could not open that folder"), null);
    }
  }, [chooseProject, report]);

  /**
   * Open a project that already exists.
   *
   * The same chooser, without the write. A folder that has settings opens on
   * them — its name, its tempo, its volume — and one that has none is simply a
   * folder full of files, which is what every project was before this file
   * existed and is still allowed to be.
   */
  const openProject = useCallback(async () => {
    try {
      const selected = await open({ directory: true, multiple: false });
      if (!selected || typeof selected !== "string") return; // user cancelled
      chooseProject(selected);
    } catch (e) {
      report(toDiagnostic(e, "could not open that folder"), null);
    }
  }, [chooseProject, report]);

  /**
   * Install a library pack.
   *
   * Installing over one that is already there asks first — the backend answers
   * with what is installed and what is arriving rather than replacing either
   * silently, and has written nothing by the time it does. Answering yes is the
   * same call again, which is why the confirm sits here rather than inside it.
   */
  const installLibrary = useCallback(async () => {
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: "Scree library", extensions: [PACK_EXTENSION] }],
      });
      if (!selected || typeof selected !== "string") return; // user cancelled

      let outcome = await invoke<InstallOutcome>("install_library", {
        pack: selected,
        replace: false,
      });

      if (outcome.kind === "conflict") {
        const { name, installed, incoming } = outcome;
        const ok = await confirm(
          `${name} ${installed || "(no version)"} is already installed. ` +
            `Replace it with ${incoming || "this copy"}?`,
          { title: "Replace library", kind: "warning" },
        );
        if (!ok) return;
        outcome = await invoke<InstallOutcome>("install_library", {
          pack: selected,
          replace: true,
        });
      }

      setLibraryVersion((v) => v + 1);
      setSideTab("libraries");
    } catch (e) {
      report(toDiagnostic(e, "could not install that library"), null);
    }
  }, [report]);

  /**
   * Pack the project up as a library somebody else can install.
   *
   * What is packed is named by a `scree-library.json` in the project, beside
   * the settings file and read the same way — rather than by a dialog asking
   * for a name every time. A library is exported more than once (that is what a
   * version is for), and the answers to "what is it called" and "what is in it"
   * should not have to be retyped, or be able to differ between two exports of
   * the same folder.
   *
   * The file is written the first time, seeded from the project's own name, so
   * the first export is still one click. If it names something the project does
   * not hold, the refusal says so and the file is there to correct.
   */
  const exportLibrary = useCallback(async () => {
    if (!projectRoot) {
      report(
        toDiagnostic(
          "a library is packed out of a project — open one first",
          "nothing to export",
        ),
        null,
      );
      return;
    }

    try {
      const manifest = await invoke<LibraryManifest>("library_manifest", {
        root: projectRoot,
      });

      const dest = await save({
        defaultPath: `${manifest.name}-${manifest.version}.${PACK_EXTENSION}`,
        filters: [{ name: "Scree library", extensions: [PACK_EXTENSION] }],
      });
      if (!dest) return; // user cancelled

      await invoke("export_library", { root: projectRoot, dest });
    } catch (e) {
      report(toDiagnostic(e, "could not export that library"), null);
    }
  }, [projectRoot, report]);

  /**
   * Evaluate the active tab.
   *
   * A refusal from any compiler pass arrives here as a rejection, and every
   * one of them ends up in the problems panel: this used to be an unhandled
   * `await`, so a program with a typo in it simply never started while the
   * previous one kept playing, with nothing on screen to say why.
   *
   * Answers whether the engine is playing what was asked for, which only
   * `toggleRecording` reads — a take that began with an eval nobody could
   * compile is a recording of the silence that followed it. A tab that is not
   * there is not a failure by that measure: nothing was asked for, and
   * whatever was already playing still is.
   */
  const play = useCallback(async (): Promise<boolean> => {
    // Nothing open is nothing to run — the transport's buttons stay live for
    // stop, which is about what the engine is holding, not about a tab.
    if (!codeTab) return true;
    const tabId = codeTab.id;
    try {
      // The drawn patterns go with the code: they are bindings the program can
      // name, and an eval is the only moment they mean anything.
      //
      // So does the workspace, for the same reason: a `use` resolves against
      // the folder this tab is saved in. What it finds there is what is on
      // disk, so a module edited in another tab is heard once it is saved.
      await invoke("run_code", {
        code: codeTab.content,
        // Null while the panel has nothing to say about this project — before
        // its file has been read, or after a read that failed. An empty panel
        // that *has* read the project is an empty list, and means it: only
        // that may hide a patterns file sitting on disk.
        patterns: patternsFrom.current === projectRoot ? toWire(patterns) : null,
        workspace: { path: codeTab.path, root: projectRoot },
      });
      setDiagnostics([]);
      setRunStatus("ok");
      setSourceTabId(tabId);
      // The engine is holding this program until something stops it, which is
      // what the lit play button says. Here rather than beside the `return`
      // below, because that answers true for a tab that was never run.
      setPlaying(true);
      return true;
    } catch (e) {
      report(toDiagnostic(e), tabId);
      return false;
    }
  }, [codeTab, patterns, projectRoot, report]);

  const stop = useCallback(async () => {
    try {
      await invoke("stop_audio");
      setPlaying(false);
    } catch (e) {
      // Left lit: the engine was not told, so whatever it is holding it is
      // still holding, and a dark button would be a claim about silence that
      // nothing has made true.
      report(toDiagnostic(e, "could not stop the engine"), null);
    }
  }, [report]);

  /**
   * Start a take, or end the one that is running.
   *
   * Record plays as well, because the two are one gesture: what you want
   * recorded is the piece from its first note, and pressing play and then
   * record loses the attack while pressing record and then play spends the
   * first bar of the file on an empty room. So this presses both, and in that
   * order — the tap is armed before the eval is sent, so the crossfade the
   * program fades in on is in the file rather than in front of it.
   *
   * Stopping is the mirror: it silences the engine and then closes the file.
   * In that order, and with a wait between them — `stop_audio` fades the graph
   * out over 0.2s, and a take closed before that landed would end mid-waveform
   * on a click. The button keeps its clock through the wait, which is honest:
   * the fade is still being recorded.
   *
   * Nothing here decides where the file goes or what it is written as: the
   * backend reads that out of the settings file at the moment recording
   * starts, so a folder chosen in the panel a second ago is the folder this
   * take lands in without anything having to be handed along.
   *
   * A refusal — no project and no folder chosen, a folder that has since been
   * unplugged — opens the settings panel as well as the problems one. It is
   * the only failure in the app whose fix is a control rather than an edit,
   * and the panel is where that control is.
   */
  const toggleRecording = useCallback(async () => {
    if (recording) {
      // Silence first, then wait out the fade, then close the file — see the
      // note on FADE_OUT_DELAY. A failure to stop the engine has already been
      // reported by `stop`, and is no reason not to save what was played.
      await stop();
      await new Promise((done) => window.setTimeout(done, FADE_OUT_DELAY));
      try {
        const finished = await invoke<FinishedRecording>("stop_recording");
        setRecording(null);
        setLastRecording(finished);
      } catch (e) {
        setRecording(null);
        report(toDiagnostic(e, "could not finish the recording"), null);
      }
      return;
    }

    try {
      const path = await invoke<string>("start_recording", {
        root: projectRoot,
        // What the take is named after: the project as it is called in the
        // panel, which is a piece's name rather than a folder's.
        name: projectName,
      });
      setRecording({ path, seconds: 0 });
    } catch (e) {
      report(toDiagnostic(e, "could not start recording"), null);
      setPanelOpen(true);
      setPanelTab("settings");
      return;
    }

    // The program that could not be compiled is already in the problems panel
    // by now. What is left is a take of the silence it did not make, and
    // leaving it running would be a red button recording nothing while its
    // reason sits in another panel. The file it made is closed rather than
    // removed — an empty take is still a file somebody's disk now has, and
    // nothing in this app deletes one behind your back.
    if (!(await play())) {
      try {
        setLastRecording(await invoke<FinishedRecording>("stop_recording"));
      } catch (e) {
        report(toDiagnostic(e, "could not finish the recording"), null);
      }
      setRecording(null);
    }
  }, [recording, projectRoot, projectName, play, stop, report]);

  /**
   * Keep the clock on the record button honest, and collect a take that ended
   * without being asked to.
   *
   * Polled rather than pushed because the elapsed time has to be drawn anyway,
   * and one ask a second answers both questions at once. The second is the
   * important one: a disk filling up mid-performance is nobody's command to
   * fail, so there is no rejection for it to arrive in, and this is where it
   * is noticed. It runs only while a take is running.
   */
  const isRecording = recording !== null;
  useEffect(() => {
    if (!isRecording) return;
    let live = true;

    const tick = async () => {
      try {
        const state = await invoke<RecordingState>("recording_state");
        if (!live) return;
        if (state.recording) {
          // Through the setter rather than off `recording`, so this effect
          // depends on *whether* a take is running and not on how long it has
          // been: the other way round tears the interval down and builds it
          // again twice a second.
          setRecording((current) =>
            current && { path: state.path ?? current.path, seconds: state.seconds },
          );
          return;
        }
        // It stopped on its own, which only happens when writing failed.
        setRecording(null);
        if (state.failure) {
          report(toDiagnostic(state.failure, "the recording stopped"), null);
        }
      } catch (e) {
        if (!live) return;
        setRecording(null);
        report(toDiagnostic(e, "could not read the recording"), null);
      }
    };

    const timer = window.setInterval(() => void tick(), 500);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [isRecording, report]);

  /**
   * A note that would not build, raised by the scheduler thread mid-pattern.
   *
   * It has already dropped the bindings and cut what it had pushed — a voice it
   * cannot build is one it cannot build for any later step either, so carrying
   * on would be the same failure every bar with nothing to hear for it. This
   * side finishes the job: `stop_audio` takes down the persistent graph too and
   * puts the clock back, so what is left is a real stop rather than a scheduler
   * sitting out a performance the rest of the engine thinks is still going.
   *
   * It goes to the problems panel like every other failure — `report` opens the
   * panel on that tab, which is what makes an error nobody asked for visible
   * without anything else on screen having to move.
   *
   * Subscribed once. `report` and `stop` are stable, so this never re-binds —
   * which matters here more than elsewhere, because re-subscribing is
   * asynchronous and the event it might fall between is the one that says the
   * music has stopped.
   */
  useEffect(() => {
    const subscription = listen<Diagnostic>("scheduler-error", (event) => {
      report(toDiagnostic(event.payload, "playback failed"), null);
      void stop();
    });
    return () => void subscription.then((unlisten) => unlisten());
  }, [report, stop]);

  const saveTab = useCallback(async () => {
    const tab = activeTab;
    // A composer has no buffer to save — its pattern is written to the
    // project's file as it is drawn — so ⌘S over one does nothing rather than
    // asking where to put an empty file.
    if (!tab || !isCode(tab)) return;
    try {
      let path = tab.path;
      if (!path) {
        // First save: ask where to put it, defaulting to a .scree file.
        path = await save({ defaultPath: tab.title, filters: SCREE_FILTER });
        if (!path) return; // user cancelled
      }
      await invoke("save_file", { path, content: tab.content });
      const savedPath = path;
      setTabs((prev) =>
        prev.map((t) =>
          t.id === tab.id
            ? { ...t, path: savedPath, title: basename(savedPath), dirty: false }
            : t,
        ),
      );

      // The patterns file has two editors — the panel and, since it is an
      // ordinary scree file, a tab. Saving it in one updates the other, so
      // they cannot drift apart without somebody watching it happen.
      if (savedPath === patternsPath && projectRoot !== null) {
        await loadPatterns(projectRoot);
      }
      // And the settings file the same way: edit the tempo in it by hand, save,
      // and the fader moves — which is also the moment the engine is told.
      if (savedPath === projectPath && projectRoot !== null) {
        await loadProject(projectRoot);
      }
    } catch (e) {
      // Left dirty on purpose: the tab still differs from what is on disk.
      report(toDiagnostic(e, `could not save ${tab.title}`), tab.id);
    }
  }, [activeTab, patternsPath, projectPath, projectRoot, loadPatterns, loadProject, report]);

  // The File menu's items arrive as events. Their handlers take new identities
  // on every keystroke, so the listeners reach them through a ref and subscribe
  // once for the life of the app: re-subscribing is asynchronous, and a menu
  // click landing mid-swap could be heard by both the old listener and the new.
  const fileActions = useRef({
    newTab,
    openTab,
    saveTab,
    newProject,
    openProject,
    installLibrary,
    exportLibrary,
  });
  fileActions.current = {
    newTab,
    openTab,
    saveTab,
    newProject,
    openProject,
    installLibrary,
    exportLibrary,
  };

  useEffect(() => {
    const subscriptions = [
      listen("file-new", () => fileActions.current.newTab()),
      listen("file-open", () => void fileActions.current.openTab()),
      listen("file-save", () => void fileActions.current.saveTab()),
      listen("project-new", () => void fileActions.current.newProject()),
      listen("project-open", () => void fileActions.current.openProject()),
      listen("library-install", () => void fileActions.current.installLibrary()),
      listen("library-export", () => void fileActions.current.exportLibrary()),
    ];
    return () => {
      for (const sub of subscriptions) void sub.then((unlisten) => unlisten());
    };
  }, []);

  // The mounted editor, and the tab it belongs to.
  //
  // State rather than a ref, and tagged with its tab, for two reasons. The
  // view is built in an effect inside CodeMirror, so a ref still reads empty
  // when this component's effects run on a remount — the marks below would
  // never be applied. And the editor remounts per tab, so a view without its
  // tab's name on it is indistinguishable from the one that just closed.
  const [editor, setEditor] = useState<{ tabId: string; view: EditorView } | null>(null);
  const activeView = editor && editor.tabId === activeId ? editor.view : null;

  // Only the ones about the file that was run. A line number from an imported
  // module means nothing in this buffer, and marking it would be pointing at
  // innocent code.
  const errorLines = useMemo(
    () =>
      diagnostics
        .filter((d) => d.file === null)
        .map((d) => d.line)
        .filter((line): line is number => line !== null),
    [diagnostics],
  );

  // Marks belong to the tab that was run, so switching away clears them — the
  // same line number in another file means nothing. Switching back re-applies
  // them, since the remount gives the editor a fresh state.
  useEffect(() => {
    if (!activeView) return;
    showErrorLines(activeView, sourceTabId === activeId ? errorLines : []);
  }, [activeView, errorLines, sourceTabId, activeId]);

  const revealDiagnostic = useCallback(
    (diagnostic: Diagnostic) => {
      const at = { line: diagnostic.line, column: diagnostic.column };
      // A diagnostic from an imported module is about that file, so the click
      // opens it. The jump waits for the open: until the tab exists there is
      // no editor to move the cursor in, and moving it in the old one would
      // put it on an unrelated line.
      if (diagnostic.file !== null) {
        void openPath(diagnostic.file).then((opened) => {
          if (opened) setPendingReveal(at);
        });
        return;
      }
      if (sourceTabId && sourceTabId !== activeId) setActiveId(sourceTabId);
      setPendingReveal(at);
    },
    [openPath, sourceTabId, activeId],
  );

  useEffect(() => {
    if (!pendingReveal || !activeView) return;
    if (pendingReveal.line !== null) {
      revealPosition(activeView, pendingReveal.line, pendingReveal.column);
    }
    setPendingReveal(null);
  }, [pendingReveal, activeView]);

  // `activeView` is null until the editor mounted for the tab that is actually
  // in front, so this waits out both the open and the remount before taking the
  // keyboard — and never puts the cursor in the tab being switched away from.
  useEffect(() => {
    if (!pendingFocus || !activeView) return;
    activeView.focus();
    setPendingFocus(false);
  }, [pendingFocus, activeView]);

  // ⌘, plays, ⌘. stops. The file shortcuts aren't here: they hang off their
  // menu items, whose accelerators fire before the key reaches the webview.
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key === ",") {
        e.preventDefault();
        void play();
      } else if (mod && e.key === ".") {
        e.preventDefault();
        void stop();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [play, stop]);

  return (
    <div className="flex h-screen flex-col bg-neutral-900 text-neutral-100">
      <header className="flex items-center justify-between border-b border-neutral-800 px-4 py-2">
        <div className="flex items-center gap-2">
          {/* On the left, for the panel on the left. Wears the number of
              problems so a run that failed is visible with the panel shut. */}
          <button
            onClick={() => setSideOpen((open) => !open)}
            title={sideOpen ? "Hide panel" : "Show panel"}
            aria-expanded={sideOpen}
            className={
              "relative rounded-md p-1.5 transition-colors " +
              (sideOpen
                ? "bg-neutral-700 text-neutral-100 hover:bg-neutral-600"
                : diagnostics.length > 0
                  ? "text-red-400 hover:bg-neutral-800 hover:text-red-300"
                  : "text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100")
            }
          >
            <svg viewBox="0 0 24 24" fill="currentColor" className="h-4 w-4">
              <path d="M3 6h18v2H3V6zm0 5h18v2H3v-2zm0 5h18v2H3v-2z" />
            </svg>
            {diagnostics.length > 0 && (
              <span className="absolute -right-1 -top-1 min-w-4 rounded-full bg-red-600 px-1 text-center text-[10px] font-semibold leading-4 text-white">
                {diagnostics.length}
              </span>
            )}
          </button>
          <div className="ml-4">
            <Transport
              play={play}
              stop={stop}
              state={transport}
              onChange={setTransport}
              playing={playing}
              recording={recording}
              onToggleRecording={() => void toggleRecording()}
            />
          </div>
        </div>
        <div className="flex items-center gap-2">
          {/* One hamburger, both ways: it is where a reader looks for the
              panel whether it is showing or not. */}
          <button
            onClick={() => setPanelOpen((open) => !open)}
            title={panelOpen ? "Hide right panel" : "Show right panel"}
            aria-expanded={panelOpen}
            className={
              "rounded-md p-1.5 transition-colors " +
              (panelOpen
                ? "bg-neutral-700 text-neutral-100 hover:bg-neutral-600"
                : "text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100")
            }
          >
            <svg viewBox="0 0 24 24" fill="currentColor" className="h-4 w-4">
              <path d="M3 6h18v2H3V6zm0 5h18v2H3v-2zm0 5h18v2H3v-2z" />
            </svg>
          </button>
        </div>
      </header>

      {/* Tab bar */}
      <div className="flex items-stretch border-b border-neutral-800 bg-neutral-950/40">
        <div className="flex flex-1 items-stretch overflow-x-auto">
          {tabs.map((tab) => {
            const isActive = tab.id === activeId;
            return (
              <div
                key={tab.id}
                onClick={() => setActiveId(tab.id)}
                className={
                  "group flex cursor-pointer items-center gap-2 border-r border-neutral-800 px-3 py-1.5 text-sm " +
                  (isActive
                    ? "bg-neutral-900 text-neutral-100"
                    : "bg-neutral-950/40 text-neutral-400 hover:bg-neutral-900/60")
                }
              >
                <span className="whitespace-nowrap">
                  {tab.patternId
                    ? (patterns.find((p) => p.id === tab.patternId)?.name ?? "pattern")
                    : tab.title}
                </span>
                {tab.dirty && (
                  <span
                    className="h-1.5 w-1.5 rounded-full bg-neutral-400"
                    title="Unsaved changes"
                  />
                )}
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    closeTab(tab.id);
                  }}
                  title="Close tab"
                  className="rounded p-0.5 text-neutral-500 opacity-0 transition-opacity hover:bg-neutral-700 hover:text-neutral-100 group-hover:opacity-100"
                >
                  <svg viewBox="0 0 24 24" fill="currentColor" className="h-3.5 w-3.5">
                    <path d="M18.3 5.7 12 12l6.3 6.3-1.4 1.4L10.6 13.4 4.3 19.7 2.9 18.3 9.2 12 2.9 5.7 4.3 4.3l6.3 6.3 6.3-6.3z" />
                  </svg>
                </button>
              </div>
            );
          })}
        </div>
        <button
          onClick={newTab}
          title="New tab (⌘N)"
          className="flex items-center px-3 text-neutral-400 transition-colors hover:bg-neutral-900 hover:text-neutral-100"
        >
          <svg viewBox="0 0 24 24" fill="currentColor" className="h-4 w-4">
            <path d="M11 11V5h2v6h6v2h-6v6h-2v-6H5v-2z" />
          </svg>
        </button>
      </div>

      <div className="flex min-h-0 flex-1">
        <SidePanel
          open={sideOpen}
          width={sideWidth}
          onResizeStart={startSideResize}
          tab={sideTab}
          onTabChange={setSideTab}
          problemCount={diagnostics.length}
          problems={
            <ProblemsPanel
              status={runStatus}
              diagnostics={diagnostics}
              sourceTitle={tabs.find((t) => t.id === sourceTabId)?.title ?? null}
              sourceIsActive={sourceTabId === activeId}
              onReveal={revealDiagnostic}
            />
          }
          project={
            <ProjectPanel
              root={projectRoot}
              name={projectName}
              onRename={setProjectName}
              // Throws away every listing the tree is holding, which is how a
              // refresh — or a new project — is picked up.
              version={projectVersion}
              activePath={activeTab?.path ?? null}
              onOpenFile={(path) => void openPath(path)}
              onMoved={onMoved}
              onDeleted={onDeleted}
              onProblems={(diagnostics) => reportAll(diagnostics, null)}
            />
          }
          search={
            <SearchPanel
              root={projectRoot}
              version={projectVersion}
              onOpen={(path, line, column) => void openAt(path, line, column)}
            />
          }
          libraries={
            <LibrariesPanel
              root={projectRoot}
              // A vendored library is a folder in the project, so anything
              // that changes what is installed also changes the tree.
              version={libraryVersion + projectVersion}
              onInstall={() => void installLibrary()}
              onChanged={() => {
                setLibraryVersion((v) => v + 1);
                setProjectVersion((v) => v + 1);
              }}
              onError={(message) =>
                report(toDiagnostic(message, "library"), null)
              }
            />
          }
        />
        <main className="flex min-h-0 min-w-0 flex-1 flex-col">
          {activeTab?.patternId ? (
            (() => {
              const pattern = patterns.find((p) => p.id === activeTab.patternId);
              // The pattern can go while its tab is open — deleted from the
              // panel, or dropped when a project was reloaded from disk.
              if (!pattern) {
                return (
                  <div className="flex h-full items-center justify-center text-sm text-neutral-500">
                    This pattern no longer exists.
                  </div>
                );
              }
              return (
                <PatternComposer
                  pattern={pattern}
                  error={nameError(pattern, patterns)}
                  beatsPerBar={beatsPerBar}
                  onChange={(next) =>
                    setPatterns(patterns.map((p) => (p.id === next.id ? next : p)))
                  }
                />
              );
            })()
          ) : activeTab?.patternName ? (
            // A restored composer whose patterns have not arrived yet, or
            // whose project could not be read at all. Never an editor: this
            // tab has no buffer, and one shown here could be typed into and
            // saved over something.
            <div className="flex h-full items-center justify-center text-sm text-neutral-500">
              Opening {activeTab.patternName}…
            </div>
          ) : activeTab ? (
            <CodeMirror
              key={activeTab.id}
              onCreateEditor={(view) => setEditor({ tabId: activeTab.id, view })}
              value={activeTab.content}
              onChange={(value) => updateContent(activeTab.id, value)}
              height="100%"
              theme="dark"
              extensions={extensions}
              className="h-full text-sm"
            />
          ) : restoring ? (
            // The last session is still being read. Blank rather than "no file
            // open", which would be advice about a state nobody is in — the
            // tabs are on their way.
            <div className="h-full" />
          ) : (
            // Closing the last tab is allowed to leave nothing open. The two
            // ways back are both in the File menu, so name them rather than
            // growing a second set of buttons that only exist here.
            <div className="flex h-full flex-col items-center justify-center gap-1 text-sm text-neutral-500">
              <p>No file open</p>
              <p className="text-xs">
                <span className="font-mono text-neutral-400">⌘N</span> for a new
                one, <span className="font-mono text-neutral-400">⌘O</span> to
                open
              </p>
            </div>
          )}
        </main>
        <RightPanel
          open={panelOpen}
          width={panelWidth}
          onResizeStart={startResize}
          tab={panelTab}
          onTabChange={setPanelTab}
          transport={
            <TransportPanel
              patterns={patterns}
              onPatternsChange={setPatterns}
              onOpenPattern={openComposer}
              onOpenFile={(path) => void openPatternsFile(path)}
              patternsPath={patternsPath}
              hasProject={projectRoot !== null}
              beatsPerBar={beatsPerBar}
            />
          }
          docs={<DocsPanel builtins={metadata.builtins} focus={docsFocus} />}
          settings={
            <SettingsPanel
              settings={settings}
              onChange={changeSettings}
              formats={formats}
              projectRoot={projectRoot}
              recording={recording}
              last={lastRecording}
              onError={(message) =>
                report(toDiagnostic(message, "recording"), null)
              }
            />
          }
        />
      </div>
    </div>
  );
}

export default App;
