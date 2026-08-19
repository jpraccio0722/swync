import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { Entry } from "./projectTree";

/** Which store a library came out of, as the backend spells it. */
export type LibrarySource = "global" | "project";

/** One installed library, as `list_libraries` reports it. */
export interface Library {
  name: string;
  version: string;
  author: string | null;
  description: string | null;
  source: LibrarySource;
  /** Absolute, so Reveal can hand it straight to the platform. */
  path: string;
}

interface LibrariesPanelProps {
  /** The open project, or null. Vendoring needs one; nothing else does. */
  root: string | null;
  /** Bumped when something outside this panel changed what is installed — the
   *  File menu's own Install…, which lands here rather than in the panel. */
  version: number;
  /** Install a pack, which is the File menu's action and the button's alike. */
  onInstall: () => void;
  /** Something changed, so anything else counting libraries should look again. */
  onChanged: () => void;
  onError: (message: string) => void;
}

/**
 * What is installed, and what can be done about it.
 *
 * A library is a folder of modules a `use` may resolve in, and until it is on
 * screen that is a folder nobody can see: it is not in the project tree, it is
 * not in a file, and the only evidence of it is that `use kit` suddenly works.
 * So this is the list — what is installed, which store it came from, and the
 * three things worth doing to one.
 *
 * **Vendor** is the one that is not obvious. It copies a library into the
 * project, so the project stops depending on what happens to be installed on
 * this machine: hand the folder to somebody, or check it into a repository, and
 * it still plays.
 */
export function LibrariesPanel({
  root,
  version,
  onInstall,
  onChanged,
  onError,
}: LibrariesPanelProps) {
  const [libraries, setLibraries] = useState<Library[]>([]);
  const [shadowed, setShadowed] = useState<string[]>([]);
  const [busy, setBusy] = useState<string | null>(null);

  // The load below re-runs whenever `refresh` changes identity, and `refresh`
  // is rebuilt whenever anything it closes over does. A parent that writes
  // `onError={(m) => ...}` inline hands down a new function every render, which
  // would make that a loop: load, report or set state, render, load again. The
  // report is one-way and nothing waits on it, so calling whichever one is
  // current is exactly as good as calling the one captured when `refresh` was
  // built — and it keeps the load keyed to the project rather than the render.
  const latestError = useRef(onError);
  latestError.current = onError;

  const refresh = useCallback(async () => {
    try {
      setLibraries(await invoke<Library[]>("list_libraries", { root }));
    } catch (e) {
      latestError.current(String(e));
    }

    // What the project itself would answer a `use` with. Only the top level:
    // that is where a file being run usually sits, and a `use` resolves beside
    // the file that writes it rather than from the project's root — so this is
    // a warning about the likely case, not a claim about every file.
    if (!root) {
      setShadowed([]);
      return;
    }
    try {
      const entries = await invoke<Entry[]>("list_dir", { path: root });
      setShadowed(
        entries
          .filter((e) => e.isDir || e.name.endsWith(".swync"))
          .map((e) => (e.isDir ? e.name : e.name.slice(0, -".swync".length))),
      );
    } catch {
      // The tree next door reports its own failures; this is only a note.
      setShadowed([]);
    }
  }, [root]);

  useEffect(() => {
    void refresh();
  }, [refresh, version]);

  const vendor = useCallback(
    async (library: Library) => {
      if (!root) return;
      setBusy(library.name);
      try {
        await invoke("vendor_library", { name: library.name, root });
        onChanged();
      } catch (e) {
        onError(String(e));
      } finally {
        setBusy(null);
      }
    },
    [root, onChanged, onError],
  );

  const remove = useCallback(
    async (library: Library) => {
      const where =
        library.source === "project" ? "this project" : "every project on this machine";
      const ok = await confirm(
        `Remove ${library.name} from ${where}? Its files and samples go with it.`,
        { title: "Remove library", kind: "warning" },
      );
      if (!ok) return;

      setBusy(library.name);
      try {
        await invoke("remove_library", {
          name: library.name,
          source: library.source,
          root,
        });
        onChanged();
      } catch (e) {
        onError(String(e));
      } finally {
        setBusy(null);
      }
    },
    [root, onChanged, onError],
  );

  const reveal = useCallback(
    async (library: Library) => {
      try {
        await revealItemInDir(library.path);
      } catch (e) {
        onError(String(e));
      }
    },
    [onError],
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center justify-between gap-2 border-b border-neutral-800 px-3 py-1.5">
        <span className="text-xs text-neutral-500">
          {libraries.length === 0
            ? "Nothing installed"
            : `${libraries.length} installed`}
        </span>
        <button
          onClick={onInstall}
          className="shrink-0 rounded bg-neutral-800 px-2 py-0.5 text-[11px] text-neutral-300 transition-colors hover:bg-neutral-700 hover:text-neutral-100"
        >
          Install…
        </button>
      </div>

      {libraries.length === 0 ? (
        <div className="px-4 py-4 text-xs leading-relaxed text-neutral-500">
          A library is a folder of instruments somebody sent you. Install one and
          every project on this machine can <span className="font-mono">use</span>{" "}
          it.
        </div>
      ) : (
        <ul className="min-h-0 flex-1 overflow-y-auto py-1">
          {libraries.map((library) => (
            <li key={`${library.source}:${library.name}`} className="px-3 py-1.5">
              <div className="flex items-baseline gap-2">
                <span className="font-mono text-xs text-neutral-200">
                  {library.name}
                </span>
                {library.version && (
                  <span className="text-[10px] text-neutral-600">
                    {library.version}
                  </span>
                )}
                <span className="ml-auto shrink-0 rounded-full bg-neutral-800 px-1.5 text-[10px] leading-4 text-neutral-400">
                  {library.source === "project" ? "in project" : "installed"}
                </span>
              </div>

              {library.description && (
                <p className="mt-0.5 text-[11px] leading-snug text-neutral-500">
                  {library.description}
                </p>
              )}

              {/* A file in the project answers a `use` before any store does,
                  so a name that is both is a library this project cannot
                  reach. Worth saying where it is noticed rather than leaving
                  it to be discovered at an eval. */}
              {shadowed.includes(library.name) && (
                <p className="mt-0.5 text-[11px] leading-snug text-amber-500/80">
                  This project has a {library.name}.swync of its own, which
                  wins.
                </p>
              )}

              <div className="mt-1 flex gap-3 text-[11px] text-neutral-500">
                {library.source === "global" && root && (
                  <Action
                    label="Vendor into project"
                    disabled={busy === library.name}
                    onClick={() => void vendor(library)}
                  />
                )}
                <Action label="Reveal" onClick={() => void reveal(library)} />
                <Action
                  label="Remove"
                  disabled={busy === library.name}
                  onClick={() => void remove(library)}
                />
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function Action({
  label,
  disabled,
  onClick,
}: {
  label: string;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className="transition-colors hover:text-neutral-200 disabled:text-neutral-700"
    >
      {label}
    </button>
  );
}
