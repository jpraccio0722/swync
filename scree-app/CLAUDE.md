# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Scree: a live-coding language and its editor, shipped as a Tauri 2 desktop app. The frontend (`src/`) is React 19 + TypeScript + Vite, with CodeMirror 6 as the editor. The backend (`src-tauri/src/`) is Rust and holds the whole language — lexer, parser, lowerer, audio-graph realizer, pattern scheduler — plus the audio engine (`fundsp` for DSP, `cpal` for the output stream).

The repo directory is `brap`; the product, npm package and Rust crate are all `scree`.

## Commands

```bash
npm install && npm run tauri dev
```

That is the only way to run the app — `npm run dev` alone serves the frontend with no backend, so every `invoke` fails.

| | |
|---|---|
| Typecheck + build frontend | `npm run build` (`tsc && vite build`) |
| Bundle the app | `npm run tauri build` |
| Rust tests | `cargo test` from `src-tauri/` |
| One Rust test | `cargo test the_test_name` from `src-tauri/` (names are sentences: `cargo test master_volume_scales_the_output`) |

There is no linter and no frontend test runner. The Rust suite is the test suite — ~725 tests, mostly `#[cfg(test)]` modules beside the code, with larger ones in `lowerer/tests.rs`, `imports/tests.rs`, `files/tests.rs` and `library/tests.rs`.

Three tests fail on a fresh clone and always have: `the_example_project_compiles`, `every_example_compiles_and_realizes` and `a_program_without_imports_needs_no_folder`. The first two want fixture files under `examples/`, which is not checked in; the third is confused by a stray `src-tauri/patterns.scree` if one is sitting there. Confirm against `HEAD` before assuming a change caused them.

`[profile.dev]` in `Cargo.toml` raises `opt-level` for this crate and to 3 for dependencies. That is not incidental: an unoptimized `fundsp` misses the real-time deadline and the audio crackles. Leave it alone.

## The eval pipeline

`run_code` in `src-tauri/src/lib.rs` is the spine — the editor's play key (`Cmd/Ctrl + ,`) lands there, and reading it explains most of the backend. In order:

1. **`parser::parse`** — `logos` lexer (`parser/lex.rs`), `chumsky` parser (`parser/parser.rs`) → `Vec<ScreeItem>`.
2. **`imports::expand`** — resolves `use` into one flat program. After this pass no modules exist, only definitions with longer names, so nothing downstream knows about files. It is also the last moment anything knows *which* file a definition came out of, so it is where a module's `load` paths are made absolute (see Libraries below).
3. **`samples::load`** — decodes what `load` names. This is the only thread allowed to touch a disk; the scheduler thread builds a voice per note and must never block on I/O.
4. **`lowerer::lower_with_samples`** — evaluates the program into two artifacts: a `ScreeGraph` (the persistent signal graph) and pattern `Binding`s.
5. **`scree_graph::realizer::realize`** — turns the graph into a `fundsp` `Net`.
6. Publish: the net is crossfaded into the engine's one slot; instruments then patterns go to the scheduler.

Every stage returns `Result<_, Diagnostic>` tagged with a `Stage`, and nothing is swapped in until all of them agree — a program that does not compile leaves whatever is playing alone. Diagnostics surface in the editor's problems panel.

**Two things make sound, by different routes.** The graph is continuous and lives in `engine.slot`, replaced by a 0.2s crossfade. Patterns are discrete: the scheduler thread (`scheduler/scheduler.rs`) free-runs from app start, wakes every 25ms, and pushes voices into a `Sequencer` 0.2s ahead of the audio clock. An eval never "triggers" the scheduler — it swaps the state the scheduler reads on its next pass. Ordering inside `run_code` matters and is commented where it does: instruments before patterns, clock reset before patterns are published.

## Recording

`src-tauri/src/recorder/` — the record button's other half. The tap is in
`render` in `engine.rs`, on the block the device is about to be filled from, so
a recording is the graph, the sequencer and the master fader together and
nothing earlier.

That puts half of it on the audio callback's side of the deadline, and the
split is the whole design. `Tap` is a fixed ring of `AtomicU32` sample bits,
allocated once at startup and sized in seconds; the callback's entire part in a
recording is copying a block in and releasing one index. A writer thread drains
it every 25ms and writes the file. When nothing is being recorded the callback
does one relaxed load and returns.

Three things follow from that and are what the tests pin:

- **A ring that overruns drops the block and counts it.** The writer thread
  would have to fall four seconds behind for this to happen. Waiting instead
  would put a gap in the *output*, which everyone in the room hears, rather
  than in the file, which one person notices later.
- **The file's header is patched every second** (`wav::Writer::checkpoint`), so
  a take that ends in a crash is still playable. A WAV whose sizes are still
  zero is not a short recording, it is nothing.
- **A failure on the writer thread has no command to be the answer to.** A
  disk filling up mid-performance is collected by `recording_state`, which the
  editor polls twice a second while a take runs — the same poll that draws the
  clock on the button.

Record plays as well, and the order is load-bearing: `toggleRecording` in
`App.tsx` arms the tap and *then* evals, so the 0.2s crossfade the program
fades in on is inside the file rather than in front of it. `play` returns
whether the eval compiled for this one caller — a take that began with a
program nobody could compile is stopped at once, since what it would otherwise
hold is the silence after a typo. Stopping is the mirror — `stop_audio`, then
`FADE_OUT_DELAY`, then close the file. The wait is the point: the graph fades
out over 0.2s, and a file closed before that landed ends mid-waveform on a
click.

`settings.rs` is the app's own settings file, in the config dir beside the
remembered session, and holds the output folder and the format. It is separate
from `project.rs` on purpose: a project's file travels with the piece, and an
absolute output path would be meaningless on anybody else's machine. The
formats come from `recorder::formats()` rather than a TypeScript list, for the
same reason `lang.rs` serves the builtins — a dropdown offering something the
recorder cannot write is a promise broken after the take.

## Bars, passes, and the one place "cycle" survives

Musical time is counted in **bars**. A **pass** is one trip through a pattern: a list of shares is one pass and fills the bar in any signature, while a pass written in note values is as long as its values add up to and rotates against the bar when they disagree. The two words are not interchangeable, and the tests say which they mean — `a_three_beat_pass_fills_a_three_four_bar` against `a_three_beat_pass_takes_three_quarters_of_a_four_four_bar` is the whole distinction in two cases.

How long a bar is comes from `Meter` in `scheduler/clock.rs`, a project setting saved in `scree-project.json`. **It reaches only two places**, and both are conversions *out of* beats: `to_pattern_timed` in `lowerer/play.rs`, where a metrical pass becomes a fraction of a bar, and the `bpm` builtin, which answers in bars per second. Everything else — the scheduler, `Rate`, `Pattern::query`, every `Binding` — already counted bars and never asks how many beats made one, which is why a signature could be added without touching them.

The clock keeps `cps` as a field name, and that is deliberate: it is bars per second, but `bps` reads as *beats* per second and the two differ by the whole signature. It is the only survival of the older word, and it survives as an abbreviation rather than as a unit.

Two orderings are load-bearing. `set_bpm` converts through whatever meter is running, so **`set_meter` comes first** when a project opens — `open_project` says so. And a signature change moves the transport but does not re-lower anything: what is playing keeps the bar it was lowered against until the next eval, the same gap a tempo drag leaves on the persistent graph.

## Language surface lives in one table

`src-tauri/src/lang.rs` holds every callable name with its arity, parameter names, `receives`/`returns` kinds and doc string. The lowerer dispatches off these tables, and the `language_metadata` Tauri command serves the same data to `src/scree/metadata.ts`, which drives highlighting, completion, signature help and the docs panel.

So **adding a builtin means adding one entry to `UGENS` (or `LIST_BUILTINS`, etc.)** — the editor picks it up with no TypeScript change. `ValueKind` is mirrored by hand in `metadata.ts`; the Rust test `every_builtin_receives_what_it_declares` keeps the declarations honest against the compiler.

## Projects, patterns, imports

A project is a folder with a `scree-project.json` (name, bpm, meter, volume), written debounced as you change things. What belongs to the machine rather than the piece — the recording folder and format — is in the app config dir instead; see Recording above. `src-tauri/src/project.rs` and `files/` own it. Two other files may sit beside it — a `scree-library.json` naming what the project exports as, and a hidden `.scree/libraries/` holding vendored libraries — both covered below.

Drawn patterns from the right-hand panel are a real file, `patterns.scree` at the project root, folded into every eval as an implicit `use patterns::*`. The panel also sends its patterns *with* the eval rather than relying on the write having landed, so `run_code` takes `Option<Vec<GraphicalPattern>>`: `None` means "the panel has nothing to say, use the disk", `Some([])` means "the panel read this project and it has no patterns" — only the second may hide a file on disk.

The project's folder is **watched** (`src-tauri/src/watcher.rs`, `notify`), which is why the tree has no refresh button. One `project-changed` event per settled 200ms burst reaches `App.tsx`, which bumps `projectVersion`; the tree re-reads the folders it has open, and the patterns and settings files are read again with it. Two filters keep that from firing on the app's own work: hidden paths are skipped, exactly as `list_dir` skips them, and content changes are ignored because the tree shows names. Anything else — including events the platform could not classify — refreshes, since an extra directory listing costs nothing and a missed file is the whole point of the watch.

That the tree now re-reads unprompted is what makes two things load-bearing. `useProjectTree` keeps every open folder open across a version bump, so nothing moves under a pointer reaching for it. And `fromWire` keeps a drawn pattern's id when the same project's file names that row again — the id is what a composer tab holds, and minting a fresh one on every re-read would take the pattern out from under an open tab.

`use` paths are routes, not names, and only ever go downward. Renaming or moving a file therefore rewrites the `use` lines that pointed at it (`files/reroute.rs`); a move that no rewrite can honestly follow still happens, and the broken imports are named in the problems panel. Imports read what is *saved*, so a module must be written to disk before a file that uses it is played.

Two rules hold across `files/`: nothing is overwritten (a collision is refused, never merged), and nothing is destroyed (deletes go to the platform trash).

## Libraries

`src-tauri/src/library/` — a shareable folder of modules, installed once and reachable from every project. A **pack** is a `.screepack`: a zip of `manifest.json` plus a `root/` whose contents are copied into a **store**, and a store is nothing but another folder a `use` resolves in. That is the whole design — `use kit::kick` already means `kit/kick.scree` beside the file, and an installed library is that same shape somewhere else, so importing, renaming and lowering learn nothing new.

Three things carry the weight:

- **`Resolver::locate` asks each root in turn and answers each in full before moving on** — beside the file, then `<project>/.scree/libraries/`, then the app config dir's store. A project file therefore beats an installed library of the same name at every step, so installing something can never change what a working project means. The roots reach `Workspace` via `set_libraries`, filled in by `run_code`: only the Rust side knows where the config dir is.
- **A pack owns exactly one top-level name.** Install refuses any entry under `root/` that is not `<name>.scree` or inside `<name>/`. That single check is why two libraries can never write the same file, and why there is no version resolution anywhere in here to get wrong. `install` returns `Outcome::Conflict` rather than replacing, having written nothing, so the editor can ask.
- **A module's `load` paths are rewritten to absolute during expansion** (`imports/rename.rs`, `Scope::relocating_samples`). Without this a library's samples resolve against whatever file imported it, which is the wrong folder — and it is the only reason a pack can carry audio at all. The program's own paths are left as written.

`library/` deliberately breaks `files/`'s trash rule: an installed pack is app-managed and reinstallable, and leaving a hundred megabytes of somebody else's samples in the Trash is worse than no undo. `export` audits before it writes, refusing an absolute `load` path, one that reaches out of the library, or a top-level `use` naming something the library does not carry — all three work on the author's machine and nowhere else.

## Frontend shape

`src/App.tsx` is deliberately the center — tabs, project state, transport, recording, panel wiring and all the `invoke` calls live there; the panel components are mostly presentational. Native menu items arrive as Tauri events (`file-new`, `project-open`, …) listened to in `App.tsx`.

`src/scree/` is the CodeMirror extension bundle. `screeExtensions()` must be called once and memoized — CodeMirror reconfigures when the extension array's identity changes, which would discard completion state on every keystroke. Values that change (drawn pattern names, the docs callback, the `Symbols` cache) are passed as getters or long-lived objects for the same reason.

Completion reads the buffer with regexes, because the text being completed is half-written and the real parser would reject it. Two things it cannot get that way:

- **What a `use kit::*` brought in.** Those names live in a file the frontend has never read, so the `module_symbols` command runs the real expander and reports the spellings *this file* would write (`kick`, `kit::kick`, `k::kick`). `src/scree/symbols.ts` asks only when the document's `use` lines change, and answers nothing when the file does not expand — which is most keystrokes, and is the right answer while it is half-typed.
- **Which argument of a call the cursor is in.** `src/scree/callsite.ts` holds `callAt`, shared with signature help rather than written twice. It is what makes `play(pat, ` offer only playable `fn`s and `play(pat, kick, ` offer that instrument's lanes — both rules live in `lowerer/play.rs` and are otherwise invisible until the program is run.

`src/scree/indent.ts` is the third thing the frontend cannot read off the buffer alone: which line breaks end a statement. It mirrors `cont_next` in `parser/lex.rs`, so a line opening with `.` or `>>` is indented one step from the line that began the statement. It applies through `indentOnInput` rather than on Enter — a break after `some()` ends a statement until the `.` is typed, and the `.` is the only moment the answer changes.

Icons are imported as components via `vite-plugin-svgr` (`import Icon from "./icon.svg?react"`), so they take a `className` and inherit `currentColor`. Styling is Tailwind 4 via the Vite plugin.

## Conventions

Comments in this codebase explain *why*, in prose, and are load-bearing — constants, orderings and refusals carry the reasoning that would otherwise be lost. Match that: a change that invalidates a comment's reasoning should update the reasoning. Test names are full sentences describing the behavior being pinned.

`README.md` is the user-facing manual, including the full function reference. It is the place to look for what a language feature is supposed to do, and the place to update when that changes.
