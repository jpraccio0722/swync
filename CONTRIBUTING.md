# Contributing to Swync

Thanks for taking an interest. This file covers what you need to know before
your first pull request — how to get the app running, which test failures are
expected, and the two places where a small change has to be made in more than
one file.

## Layout

Two npm projects, no workspace root. Install and run commands from inside one
of them.

| | |
|---|---|
| `swync-app/` | The app. React 19 + TypeScript + Vite + CodeMirror 6 frontend (`src/`), Rust/Tauri 2 backend (`src-tauri/`) holding the whole language and the audio engine. |
| `website/` | The Astro docs site, including the generated function reference. |

## Running it

```bash
cd swync-app && npm install && npm run tauri dev
```

You will need a Rust toolchain and Node 22. Development and CI happen on
macOS, and releases are built for macOS and Windows. So far, there is no Linux build:
the crate binds to an audio backend and a webview through the platform, so on
Linux you will need ALSA and webkit2gtk development packages before anything
compiles, and nothing in CI covers that path.

| | |
|---|---|
| Rust tests | `cargo test` from `swync-app/src-tauri/` |
| One Rust test | `cargo test master_volume_scales_the_output` |
| Typecheck + build frontend | `npm run build` from `swync-app/` |
| Typecheck + build site | `npm run build` from `website/` |

## Three tests fail on a fresh clone

Before you assume you broke something: `the_example_project_compiles`,
`every_example_compiles_and_realizes` and
`a_program_without_imports_needs_no_folder` fail on any clone of this
repository and always have.

The first two read fixtures from `swync-app/examples/`, which is not checked
in. The third reads the checked-in `src-tauri/patterns.swync` as a module and
gets back a program it did not write — so it fails on a clean tree, not only a
dirty one.

CI skips exactly those three and counts the skips, failing if the number is not
three. That last part matters to you: `--skip` matches a substring of a test's
path, so **a new test named near one of those three would be silently filtered
out**. If CI tells you the count moved, it means your test name collided, not
that the gate is broken.

If you see a fourth failure, it is yours. Confirm against `HEAD` before
spending time on it.

## What CI checks

`.github/workflows/test.yml` runs on every pull request into `main`. Four jobs:

- **rust** — `cargo test --locked` with those three skips, then a diff of the
  website's generated language reference against `lang.rs`.
- **frontend** — `npm ci && npm run build` in `swync-app/` (`tsc && vite build`).
- **website** — `npm ci && npm run build` in `website/` (`astro check && astro build`).
- **versions** — `swync-app/scripts/check-version.sh`.

`--locked` and `npm ci` are both there to fail on a lockfile that disagrees
with its manifest rather than quietly rewriting it. If CI fails on one of
those, commit the lockfile change deliberately.

**Deliberately not checked: formatting and lints.** There is no linter, no
`cargo fmt --check`, no clippy, and no frontend test runner. This tree is not
rustfmt-clean. Please do not run a formatter across it in a pull request that
is about something else — reformatting the tree is a decision to make on
purpose, in its own commit.

## Two changes that reach further than they look

**Adding or changing a builtin.** `swync-app/src-tauri/src/lang.rs` holds every
callable name with its arity, parameter names, `receives`/`returns` kinds and
doc string. It is the single source for all three consumers: the lowerer
dispatches off it, the editor reads it over the `language_metadata` command for
highlighting and completion, and the docs site generates from it. So adding a
builtin is one entry in `UGENS` (or `LIST_BUILTINS`, etc.) and no TypeScript
change at all — but you must then regenerate the site's reference:

```bash
cd website && npm run reference
```

CI diffs the two and fails if they have drifted. Edit the **body** of a docs
page freely; do not hand-edit its frontmatter, since `npm run reference`
rewrites it.

**Changing the version.** Five manifests carry it across both projects — six
places, since `package-lock.json` records it twice. Use the script, never a
hand edit:

```bash
swync-app/scripts/set-version.sh 1.2.3
```

`check-version.sh` fails the build if they disagree.

## Conventions

**The docs site is the user-facing manual.** The [function
reference](https://www.swync.io/docs/) is where to look for what a language
feature is supposed to do, and its pages live in `website/src/content/docs/`.
When you change what a feature does, update the page's prose body in the same
pull request — the signature in its frontmatter comes from `lang.rs` and is
regenerated, but nothing generates the explanation.

If you are touching the backend, read `swync-app/CLAUDE.md` first — it explains
the eval pipeline, the scheduler, the recorder and the audio-device handling,
and most of the non-obvious orderings are documented there rather than
discoverable from the code.

## Pull requests

Branch off `main` and open a pull request against it. Keep a pull request to
one thing; the version bump, the reformat and the feature are three pull
requests, not one.

Say what the change does and why in the description. If it changes what a
language feature does, say what the old behaviour was — that is the part a
reviewer cannot reconstruct.

CI must be green. If you are adding behaviour, add a test that pins it; the
Rust suite is the test suite here, and an untested change to the lowerer or the
scheduler is very hard to review.

## Reporting bugs

Open an issue with the program that reproduces it — a few lines of Swync is
usually enough, and is much faster to act on than a description of what the
sound did. Include your OS and how you installed the app.

## Reporting a security issue

Please do not open a public issue for a security problem. Use GitHub's private
vulnerability reporting on this repository (the **Security** tab → *Report a
vulnerability*), which reaches the maintainer privately.
