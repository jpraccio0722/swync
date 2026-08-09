/**
 * What the project tree is currently showing, in one place.
 *
 * The tree used to be self-describing: every open folder was a component that
 * fetched its own listing and threw it away when it closed. That is the right
 * shape for a tree you only look at, and the wrong one for a tree you edit —
 * every act in the panel now touches folders no single row owns. Dragging a
 * file changes two listings, neither of which is the row that was dragged.
 * Renaming into `lib/` changes a folder that may not even be open. Deleting
 * changes the folder above, which is the one part of the tree the deleted row
 * cannot reach.
 *
 * So the listings live here, keyed by path, and the rows read them. Expansion
 * lives here too, for the same reason: creating a file inside a shut folder has
 * to open it, and the row that was clicked is not that folder.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** One entry in a directory, as the backend's `list_dir` reports it. */
export interface Entry {
  name: string;
  /** Absolute, so opening it needs nothing but the click. */
  path: string;
  isDir: boolean;
}

/** What is known about one folder's contents. */
export type Listing =
  | { status: "loading" }
  | { status: "ready"; entries: Entry[] }
  /** A folder that would not list — a permission, or one that has since gone.
   *  Shown on the row rather than in the problems panel: it is that row's
   *  problem, not the program's. */
  | { status: "error"; message: string };

export interface ProjectTree {
  /** What is in a folder, or undefined for one nothing has asked about yet. */
  listing: (path: string) => Listing | undefined;
  isExpanded: (path: string) => boolean;
  /** Open a shut folder, or shut an open one. */
  toggle: (path: string) => void;
  /** Open everything above a path, so it has a row — without opening the path
   *  itself, which for a file would mean nothing and for a folder would be
   *  more than was asked. */
  reveal: (path: string) => void;
  /** Open a folder, and everything above it. */
  expand: (path: string) => void;
  /** Read one folder again. Safe for a folder nobody has opened: there is
   *  nothing to refresh, and it stays unread until it is. */
  refresh: (path: string) => void;
}

/** Which separator a path is written with. The backend hands back whatever the
 *  platform uses, so every path taken apart here has to ask first. */
function separator(path: string): string {
  return path.includes("\\") && !path.includes("/") ? "\\" : "/";
}

/** The folder a path sits in, or null for one with nowhere above it. */
export function parentOf(path: string): string | null {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (cut <= 0) return null;
  return path.slice(0, cut);
}

/** A path with one more name on the end of it. */
export function join(dir: string, name: string): string {
  return dir + separator(dir) + name;
}

/** Extract the file name from an absolute path (cross-platform). */
export function basename(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() || path;
}

/** Whether `path` is `ancestor` or sits somewhere below it. What a move has to
 *  ask of every open tab, and what the tree has to ask before letting a folder
 *  be dropped into its own child. */
export function isWithin(path: string, ancestor: string): boolean {
  return (
    path === ancestor ||
    path.startsWith(ancestor + "/") ||
    path.startsWith(ancestor + "\\")
  );
}

/**
 * The listings and the open folders, for one project.
 *
 * `version` is bumped when the project's folder changes under the app — the
 * backend watches it — and by whatever in the app has just changed it. It
 * re-reads every folder that is open, which is how the tree stays what is on
 * the disk without anything here having to know what happened.
 */
export function useProjectTree(root: string | null, version: number): ProjectTree {
  const [listings, setListings] = useState<Record<string, Listing>>({});
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());

  // Which read of a folder is the current one. Two reads can be in flight —
  // a refresh landing on top of a first open — and without this the slower one
  // would win and put back what the refresh was called to replace.
  const reads = useRef<Record<string, number>>({});

  const load = useCallback((path: string) => {
    const token = (reads.current[path] ?? 0) + 1;
    reads.current[path] = token;

    setListings((prev) => ({ ...prev, [path]: { status: "loading" } }));
    invoke<Entry[]>("list_dir", { path })
      .then((entries) => {
        if (reads.current[path] !== token) return;
        setListings((prev) => ({ ...prev, [path]: { status: "ready", entries } }));
      })
      .catch((e) => {
        if (reads.current[path] !== token) return;
        setListings((prev) => ({
          ...prev,
          [path]: { status: "error", message: String(e) },
        }));
      });
  }, []);

  // A different project. Everything read of the last one goes: a listing is
  // about a folder that is no longer being shown, and the paths in it would be
  // drawn against the wrong root for the instant before the new read lands.
  useEffect(() => {
    reads.current = {};
    setListings({});
    setExpanded(new Set(root === null ? [] : [root]));
    if (root !== null) load(root);
  }, [root, load]);

  // Something changed in the project already open. Every folder that has been
  // read is read again, and every folder that is open stays open: what you are
  // looking at is the thing being brought up to date, and collapsing it back
  // to the root answers a question nobody asked. This runs unprompted now that
  // the folder is watched, which makes keeping the tree still that much more
  // important — a row must not move under a pointer that was reaching for it.
  const seen = useRef(version);
  useEffect(() => {
    if (seen.current === version) return;
    seen.current = version;
    for (const path of Object.keys(reads.current)) load(path);
  }, [version, load]);

  const toggle = useCallback(
    (path: string) => {
      setExpanded((prev) => {
        const next = new Set(prev);
        if (next.delete(path)) return next;
        next.add(path);
        return next;
      });
      // Read on the way open, every time. A folder shut an hour ago and opened
      // now should show what is in it now, which is what the tree did when
      // closing one threw its listing away.
      if (!expanded.has(path)) load(path);
    },
    [expanded, load],
  );

  /** Open every folder from `from` up to the project, and read the ones
   *  nothing has read yet. */
  const openChain = useCallback(
    (from: string | null) => {
      const chain: string[] = [];
      for (let at = from; at !== null; at = parentOf(at)) {
        chain.push(at);
        if (root !== null && at === root) break;
      }
      if (chain.length === 0) return;
      setExpanded((prev) => new Set([...prev, ...chain]));
      for (const folder of chain) {
        if (listings[folder] === undefined) load(folder);
      }
    },
    [root, listings, load],
  );

  const reveal = useCallback(
    (path: string) => openChain(parentOf(path)),
    [openChain],
  );
  const expand = useCallback((path: string) => openChain(path), [openChain]);

  const refresh = useCallback(
    (path: string) => {
      // Only what is being shown. Reading a folder nobody has opened would put
      // a listing in the map that the tree has no row for, and the next open
      // would show it rather than asking again.
      if (listings[path] !== undefined) load(path);
    },
    [listings, load],
  );

  const listing = useCallback((path: string) => listings[path], [listings]);
  const isExpanded = useCallback((path: string) => expanded.has(path), [expanded]);

  return { listing, isExpanded, toggle, reveal, expand, refresh };
}
