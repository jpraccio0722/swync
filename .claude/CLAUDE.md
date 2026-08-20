# CLAUDE.md

Guidance for Claude Code (claude.ai/code) working in this repository.

## Layout

`brap` is the repo; `swync` is the product. Two npm projects, no workspace root — install and run commands from inside one of them.

| | |
|---|---|
| [swync-app/](swync-app/) | The app: React 19 + Vite + CodeMirror frontend, Rust/Tauri 2 backend holding the language and audio engine. Read [swync-app/CLAUDE.md](swync-app/CLAUDE.md) before touching it. |
| [website/](website/) | Astro docs site, including the generated language reference. Read [website/AGENTS.md](website/AGENTS.md). |
| [.github/workflows/](.github/workflows/) | `test.yml` gates pull requests into `main`; `build-macos.yml` / `build-windows.yml` release on a tag. |

## Commands

```bash
cd swync-app && npm install && npm run tauri dev
```

`npm run dev` alone serves the frontend with no backend, so every `invoke` fails. Rust tests are `cargo test` from `swync-app/src-tauri/`; there is no linter and no frontend test runner.

## What crosses the boundary

- **`swync-app/src-tauri/src/lang.rs` is the single source of every builtin's arity, params, kinds and docs.** The frontend reads it over the `language_metadata` command; the website dumps it via `npm run reference` in `website/`. Adding a builtin is one entry there — CI diffs the site's reference against it.
- **Version lives in six manifests across both projects.** `swync-app/scripts/set-version.sh` writes them all; `check-version.sh` fails a merge if they disagree.
- **MIDI ports are named inside a program, not chosen in a panel** (`swync-app/src-tauri/src/midi/`). That is the opposite of how an audio device is picked, and deliberately: `midiout("deluge")` says which synth a part is for, which travels with the piece. The panel lists the ports only so the names and numbers can be read off. See swync-app/CLAUDE.md.
- **A slider is written into the program, not added in the panel** (`swync-app/src-tauri/src/controls.rs`). `slider("cutoff", 200, 5000)` is a control the panel draws and the graph reads at audio rate, and the panel has no way to make one: a control exists by being written at the place it is used, so it travels with the piece. The same claim `midiout` makes about a port. Which sliders exist is read off the *syntax* before lowering, for the reason `load` paths are — an instrument's body is not lowered until a note plays. See swync-app/CLAUDE.md.
- **Play runs the project's `main.swync`, not the tab you are looking at** (`swync-app/src/App.tsx`, `project.rs`). A project is a program in several files with one entry point, so play means the same thing wherever you happen to be editing; `Cmd/Ctrl + Shift + ,` runs the file in front, which is how a module is auditioned while it is being written. New Project creates that file empty and opens it, and a project without one — every project made before it existed — plays the tab in front exactly as it always did. See swync-app/CLAUDE.md.
- Three Rust tests fail on a fresh clone and always have (see swync-app/CLAUDE.md); CI skips exactly those three and fails if the count changes.

## Conventions

Comments are prose explaining *why*, and are load-bearing — a change that invalidates a comment's reasoning updates the reasoning. Test names are full sentences. `swync-app/README.md` is the user-facing manual and the reference for what a language feature should do.