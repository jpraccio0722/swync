/**
 * Dragging rows around the project tree, and dropping files into it.
 *
 * Both of these are ordinarily three attributes on a row and nothing else —
 * `draggable`, `onDragStart`, `onDrop`. They are hand-rolled here because a
 * Tauri window may have HTML5 drag and drop or it may have the *paths* of files
 * dragged in from the desktop, and never both. `dragDropEnabled` in
 * `tauri.conf.json` decides which: with it on, the webview's drag handler is
 * wry's, and wry claims every drag that crosses the window — a file from the
 * Finder and a row dragged three pixels inside the page alike — so no
 * `dragstart` fires in the page at all. With it off, a dropped file arrives as
 * a browser `File` with no path, which is the one thing about it the tree needs.
 *
 * A file browser you cannot drop a sample into is worse than one whose rows are
 * moved by pointer events, so the trade is made that way round and this module
 * is the price. What it buys back is that both kinds of drag now aim the same
 * way: at whatever folder is under the pointer, found by hit testing the tree
 * rather than by whichever row's handler the event happened to reach. One
 * answer to "where would this land", for a row and for a sample.
 *
 * The one thing lost with HTML5 drag and drop is dragging *text* out of the
 * editor into another part of the page, which CodeMirror offers and which no
 * longer arrives. Cut and paste is the same act with two more keys.
 */

import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isWithin, parentOf } from "./projectTree";

/**
 * A row the tree is acting on: where it is, and whether it is a folder.
 *
 * Both halves are needed after the act as well as during it — what a New File
 * does next depends on whether what is selected is a folder — and by then there
 * is no `Entry` left to ask.
 */
export interface Selected {
  path: string;
  isDir: boolean;
}

/** How far the pointer travels before a press on a row becomes a drag. */
const THRESHOLD = 4;

/** How near the top or bottom edge of the tree asks it to scroll. */
const EDGE = 28;

/** How fast it scrolls there, in pixels per frame. */
const SPEED = 8;

/**
 * The scrolling box the tree is drawn in, which is not the tree.
 *
 * The panel owns the scrollbar and the tree is one of four things inside it, so
 * this is asked of the DOM rather than held as a ref: what scrolls is a fact
 * about how the panel is laid out today, and a tree that had to be told would
 * be a tree that breaks the day the layout changes.
 */
function scrollerOf(el: HTMLElement | null): HTMLElement | null {
  for (let at = el?.parentElement ?? null; at !== null; at = at.parentElement) {
    const overflow = getComputedStyle(at).overflowY;
    if (overflow === "auto" || overflow === "scroll") return at;
  }
  return null;
}

/**
 * Scroll the tree if a drag is resting against its top or bottom edge.
 *
 * The browser did this for a drag of its own and does not for ours. Without it
 * a folder below the fold is a folder nothing can be dropped into, since the
 * hand holding the row is the same hand that would work the scrollbar.
 */
function scrollNearEdge(tree: HTMLElement | null, y: number): void {
  const scroller = scrollerOf(tree);
  if (scroller === null) return;
  const box = scroller.getBoundingClientRect();
  if (y < box.top + EDGE) scroller.scrollTop -= SPEED;
  else if (y > box.bottom - EDGE) scroller.scrollTop += SPEED;
}

/**
 * The folder a drop at this point on the screen would land in.
 *
 * A drop on a folder goes into it; a drop on a file goes beside that file,
 * which is into the folder holding it; a drop on the space below the rows is
 * the project folder itself, so dragging something back out to the top level
 * does not need a row to aim at. A point outside the tree — the heading, the
 * editor, another panel — is not a destination and answers null.
 */
export function folderAt(
  tree: HTMLElement | null,
  root: string | null,
  x: number,
  y: number,
): string | null {
  if (tree === null || root === null) return null;
  const under = document.elementFromPoint(x, y);
  if (under === null || !tree.contains(under)) return null;

  const row = under.closest<HTMLElement>("[data-path]");
  if (row === null || row.dataset.path === undefined) return root;
  return row.dataset.dir === "true" ? row.dataset.path : parentOf(row.dataset.path);
}

/**
 * Eat the click a finished drag raises.
 *
 * A pointerup is still half of a click, and the browser sends one to whatever
 * the press and the release have in common — the row the drag started on, or
 * the tree above them both. Neither of those meant "open this file" or "clear
 * the selection", which is what they would otherwise be taken to mean.
 */
function swallowClick(): void {
  const eat = (e: MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
  };
  window.addEventListener("click", eat, { capture: true, once: true });
  // The click follows the pointerup in the same turn of the event loop, so a
  // timeout of zero is after it. A listener left waiting for a click that never
  // came would eat a real one minutes later.
  setTimeout(() => window.removeEventListener("click", eat, { capture: true }), 0);
}

/** What the tree shows while something is being dragged over it. */
export interface Dragging {
  /** The row on its way somewhere, so it can be dimmed. Null for a drag that
   *  started outside the app, which has no row in the tree. */
  row: Selected | null;
  /** The folder a drop would land in, and the only one lit up. Null when the
   *  pointer is nowhere a drop could go. */
  dropTarget: string | null;
}

/**
 * Moving a row to another folder by dragging it.
 *
 * The press is recorded on pointerdown and becomes a drag only once the pointer
 * has travelled [`THRESHOLD`] — clicking a row to open it moves the mouse a
 * pixel or two, and a click that quietly moved the file would be unforgivable.
 */
export function useRowDrag(
  tree: RefObject<HTMLElement | null>,
  root: string | null,
  move: (from: Selected, into: string) => void,
): Dragging & { press: (row: Selected, e: React.PointerEvent) => void } {
  const [pressed, setPressed] = useState<{ row: Selected; x: number; y: number } | null>(null);
  const [row, setRow] = useState<Selected | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);

  // What the listeners below read, in a box rather than as dependencies. They
  // are installed once for the whole of a press, and `move` is a fresh function
  // on every render of the panel — an effect that listed it would tear its own
  // listeners down mid-drag and drop the row it was carrying.
  const now = useRef({ tree, root, move });
  now.current = { tree, root, move };

  const press = useCallback((row: Selected, e: React.PointerEvent) => {
    // The left button only: the right one is opening the context menu.
    if (e.button !== 0) return;
    // And a mouse or a pen only. A finger on a row is how the panel is
    // scrolled, and a drag that started from one would take the file along for
    // the ride. HTML5 drag and drop never fired for touch either, so this
    // refusal is the behaviour the tree already had.
    if (e.pointerType === "touch") return;
    setPressed({ row, x: e.clientX, y: e.clientY });
  }, []);

  useEffect(() => {
    if (pressed === null) return;
    const { row, x: fromX, y: fromY } = pressed;

    // Held as plain variables rather than as state: they change with every
    // pointer event and nothing on screen is drawn from them directly. The
    // effect body lives exactly as long as the press does, which is what makes
    // that safe.
    let started = false;
    let at = { x: fromX, y: fromY };
    let into: string | null = null;
    let frame = 0;

    /** Where the row would land, from where the pointer is now. */
    const aim = () => {
      const folder = folderAt(now.current.tree.current, now.current.root, at.x, at.y);
      const legal =
        folder !== null &&
        // Into the folder it is already in, which is a move to where it already
        // is, and into itself or anywhere below itself, which is not a move.
        parentOf(row.path) !== folder &&
        !isWithin(folder, row.path);
      into = legal ? folder : null;
      setDropTarget(into);
    };

    // A pointer held still against an edge has to keep scrolling, so this runs
    // per frame rather than per event — and re-aims as it goes, since it is the
    // tree that is moving and not the pointer.
    const scrolling = () => {
      scrollNearEdge(now.current.tree.current, at.y);
      aim();
      frame = requestAnimationFrame(scrolling);
    };

    const onMove = (e: PointerEvent) => {
      at = { x: e.clientX, y: e.clientY };
      if (!started) {
        if (Math.abs(at.x - fromX) < THRESHOLD && Math.abs(at.y - fromY) < THRESHOLD) return;
        started = true;
        setRow(row);
        // The pointer is carrying a row now, which is a fact about the whole
        // window rather than about the row it is over. See `index.css`.
        document.body.classList.add("dragging-row");
        frame = requestAnimationFrame(scrolling);
      }
      aim();
    };

    const onUp = () => {
      // The click is swallowed for any drag that got going, not only for one
      // that landed somewhere: a row dragged around and brought back to where
      // it started is a row nobody asked to open.
      if (started) {
        if (into !== null) now.current.move(row, into);
        swallowClick();
      }
      setPressed(null);
    };

    // Escape, and a pointer the system took away from us — a system drag, a
    // window losing focus mid-press — both mean the same thing: no move.
    const onCancel = () => setPressed(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onKey);

    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("keydown", onKey);
      document.body.classList.remove("dragging-row");
      setRow(null);
      setDropTarget(null);
    };
  }, [pressed]);

  return { row, dropTarget, press };
}

/**
 * Where on the page a native drag is, in the pixels the DOM measures in.
 *
 * The event calls its position physical and on Windows it is — client pixels of
 * a window that may be scaled. On macOS the same field is in points, which is
 * what the DOM already counts in, because the two platforms' handlers report
 * what their own window systems handed them and only one of them scales.
 * Dividing by the device pixel ratio on a Retina Mac would aim at a folder in
 * the top-left quarter of the panel.
 */
function pageAt(position: { x: number; y: number }): { x: number; y: number } {
  const scale = /Mac|iPhone|iPad/.test(navigator.userAgent) ? 1 : window.devicePixelRatio;
  return { x: position.x / scale, y: position.y / scale };
}

/**
 * Files dragged in from outside the app — the Finder, a browser's downloads,
 * another editor.
 *
 * This is the whole reason the tree moves its own rows by hand: the paths only
 * arrive because the webview's drag handling belongs to Tauri rather than to
 * the page. What lands where is the panel's business and is handed back to it;
 * all that is decided here is which folder the pointer was over.
 */
export function useFileDrop(
  tree: RefObject<HTMLElement | null>,
  root: string | null,
  onDropped: (paths: string[], into: string) => void,
): Dragging {
  const [dropTarget, setDropTarget] = useState<string | null>(null);

  // For the same reason as the row drag above: the listener outlives the render
  // that made it, and the panel's callbacks do not.
  const now = useRef({ tree, root, onDropped });
  now.current = { tree, root, onDropped };

  useEffect(() => {
    if (root === null) return;
    let stop: (() => void) | undefined;
    let gone = false;

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const drag = event.payload;
        if (drag.type === "leave") {
          setDropTarget(null);
          return;
        }

        const { x, y } = pageAt(drag.position);
        const into = folderAt(now.current.tree.current, now.current.root, x, y);

        if (drag.type === "drop") {
          setDropTarget(null);
          // A drop anywhere else in the window — the editor, another panel — is
          // not this panel's to answer, and dropping a file on a window that
          // does nothing with it is what every other application does too.
          if (into !== null) now.current.onDropped(drag.paths, into);
          return;
        }

        setDropTarget(into);
        // The same reason the row drag scrolls: a folder below the fold is one
        // nothing could otherwise be dropped into. A native drag repeats its
        // position while it is held, so a step per event is enough here.
        if (into !== null) scrollNearEdge(now.current.tree.current, y);
      })
      .then((unlisten) => {
        // The panel may have gone while the listener was being registered,
        // which is a listener nobody will ever remove.
        if (gone) unlisten();
        else stop = unlisten;
      });

    return () => {
      gone = true;
      stop?.();
      setDropTarget(null);
    };
    // Only whether there is a project at all: everything else the listener
    // needs is read through the box above, so one listener covers the life of
    // the panel rather than being torn down and rebuilt as the tree changes.
  }, [root === null]);

  return { row: null, dropTarget };
}
