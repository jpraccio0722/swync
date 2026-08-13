import { useEffect, useRef, useState } from "react";
import {
  GRID_MAX,
  GRID_MIN,
  MIDDLE_C,
  ROLL_HIGH,
  ROLL_LOW,
  draw,
  erase,
  fromCells,
  isAccidental,
  noteAt,
  noteName,
  regrid,
  relocate,
  resolution,
  type GraphicalPattern,
  type Mode,
  type PatternStep,
  type StepKind,
} from "./patterns";

/**
 * The pattern composer: a piano roll and a drum row, drawn rather than typed.
 *
 * The grid is one bar, divided into cells. A note is drawn by
 * dragging across the cells it should cover, and how many cells it covers *is*
 * its rhythm — that width becomes the `;` the language reads, so what is drawn
 * and what is played are the same thing with no translation in between.
 *
 * Both modes are the same grid with a different vertical axis: the roll has one
 * lane per semitone, drums have exactly one lane. Sharing the drawing code is
 * what keeps a note behaving identically in both.
 *
 * Three gestures reach the row — drawing a note, moving one, dragging one's
 * end — and they meet almost at once. `placement` turns any of them into the
 * same answer, one note at one place at one width, and `relocate` puts it down
 * through the very `draw` a new note goes through. So there is one rule about
 * what happens on landing rather than three, and the two tools differ only in
 * what the left button means: in `draw` a click on a note erases it, in `move`
 * it takes hold of it.
 */

/** One cell's width, and a lane's height, in pixels. */
const CELL_W = 22;
const LANE_H = 13;
/** The keyboard gutter down the left of the roll. */
const GUTTER = 44;

/**
 * How near a note's end the pointer grabs it by that end rather than by its
 * middle.
 *
 * Small, because it is taken out of the middle: a one-cell note is 22px wide
 * and both handles come out of it, so anything generous here leaves nothing
 * left to drag the note by.
 */
const EDGE_PX = 5;

/**
 * Which of the two things a pointer does on the grid.
 *
 * Sticky, because the two are used in runs — a phrase is drawn, then it is
 * nudged — and a tool that had to be re-chosen every gesture would be chosen
 * wrongly half the time. The held override is the other half of that: reaching
 * for one note in the middle of drawing is a moment, not a mode.
 */
type Tool = "draw" | "move";

/** Which end of a note a resize has hold of. */
type Edge = "start" | "end";

/**
 * A gesture in progress, from the pointer going down to it coming up.
 *
 * All three carry `to` and `lane` — where the pointer is now — and differ only
 * in what they remember from where it went down. That is what lets `placement`
 * turn any of them into the same answer: one note, at one place, one width.
 */
type Gesture =
  | { kind: "draw"; from: number; to: number; lane: number }
  /** `grab` is the cell within the note the pointer took hold of, so a note
   *  moves with the pointer rather than snapping its head under it. */
  | { kind: "move"; head: number; width: number; grab: number; to: number; lane: number }
  /** `was` is the note's own kind, kept because a resize must not repitch it —
   *  only a move follows the lane the pointer is in. */
  | { kind: "resize"; head: number; width: number; edge: Edge; was: StepKind; to: number; lane: number };

/** What a gesture comes to: a note to put down, and the one to lift first. */
interface Placement {
  /** The head to clear before placing, or null when nothing is being lifted. */
  lift: number | null;
  at: number;
  width: number;
  kind: StepKind;
}

const clamp = (n: number, low: number, high: number) => Math.max(low, Math.min(high, n));

/**
 * What a gesture would make of the row if it ended now.
 *
 * Every gesture resolves here and nowhere else, so the preview the grid draws
 * and the edit the pointer commits cannot drift apart — they are the same
 * value, read twice.
 */
function placement(
  gesture: Gesture,
  cells: number,
  lanes: number[],
  drums: boolean,
): Placement {
  const under: StepKind = drums
    ? { kind: "trigger" }
    : { kind: "pitch", note: lanes[gesture.lane] };

  if (gesture.kind === "draw") {
    const at = Math.min(gesture.from, gesture.to);
    return { lift: null, at, width: Math.abs(gesture.to - gesture.from) + 1, kind: under };
  }

  if (gesture.kind === "move") {
    // Clamped to the bar rather than allowed to run off it: a note pushed at
    // the end stops there, and comes back when the pointer does.
    const at = clamp(gesture.head + (gesture.to - gesture.grab), 0, cells - gesture.width);
    return { lift: gesture.head, at, width: gesture.width, kind: under };
  }

  // A resize holds one end still and moves the other, so the fixed end is what
  // the new span is measured from — never the pointer alone.
  if (gesture.edge === "end") {
    const width = clamp(gesture.to - gesture.head + 1, 1, cells - gesture.head);
    return { lift: gesture.head, at: gesture.head, width, kind: gesture.was };
  }
  const end = gesture.head + gesture.width;
  const at = clamp(gesture.to, 0, end - 1);
  return { lift: gesture.head, at, width: end - at, kind: gesture.was };
}

interface PatternComposerProps {
  pattern: GraphicalPattern;
  onChange: (pattern: GraphicalPattern) => void;
  /** Why the name cannot be used, or null. Checked by the caller, which is the
   *  only place that knows what the other patterns are called. */
  error: string | null;
  /** Beats in a bar, from the project's time signature. The cells are shares
   *  of a bar either way — this is which of them get the heavier line. */
  beatsPerBar: number;
}

export function PatternComposer({
  pattern,
  onChange,
  error,
  beatsPerBar,
}: PatternComposerProps) {
  const cells = resolution(pattern);
  const drums = pattern.mode === "drums";
  const [gesture, setGesture] = useState<Gesture | null>(null);
  const [tool, setTool] = useState<Tool>("draw");
  /** Whether the override key is down, flipping the tool while it is held. */
  const [flipped, setFlipped] = useState(false);
  /** The head cell of the selected note, if one is selected. */
  const [selected, setSelected] = useState<number | null>(null);
  const surface = useRef<HTMLDivElement>(null);
  const scroller = useRef<HTMLDivElement>(null);

  const active: Tool = flipped ? (tool === "draw" ? "move" : "draw") : tool;

  // Highest note first, so the roll reads the way a keyboard stands.
  const lanes = drums
    ? [0]
    : Array.from({ length: ROLL_HIGH - ROLL_LOW + 1 }, (_, i) => ROLL_HIGH - i);

  /**
   * Open looking at the music.
   *
   * The roll spans all 88 keys, so scrolling to whatever is drawn is the
   * difference between opening on a part and opening on an empty stretch of
   * register two octaves above it. An empty pattern gets middle C, which is
   * where the first note is most likely to go.
   *
   * Keyed on the pattern rather than on its steps, so this places the view
   * once when a pattern is opened and never fights the scrolling afterwards.
   */
  useEffect(() => {
    const el = scroller.current;
    if (!el || drums) return;
    const pitches = pattern.steps.flatMap((s) => (s.kind === "pitch" ? [s.note] : []));
    const focus = pitches.length
      ? (Math.min(...pitches) + Math.max(...pitches)) / 2
      : MIDDLE_C;
    const lane = ROLL_HIGH - Math.round(focus);
    el.scrollTop = Math.max(0, lane * LANE_H - el.clientHeight / 2);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pattern.id, drums]);

  const setSteps = (steps: PatternStep[]) => onChange({ ...pattern, steps });

  /**
   * The tool keys, and the selection's own.
   *
   * On the window rather than the surface because the grid is not focusable and
   * should not have to be — reaching for `V` after clicking a note is not a
   * request to click the background first. What that costs is having to say
   * where typing is really typing: the name field and the code editor are both
   * places where `b` is the letter b, and the editor is a `contenteditable`
   * rather than an input, which is why the test is broader than a tag name.
   *
   * A modifier held with a letter is somebody else's shortcut — `Cmd + B` is
   * not this — so only the bare keys switch tools.
   *
   * Re-bound every render, deliberately: Delete reads the current selection and
   * the current row, and a listener bound once would go on answering with
   * whichever ones existed when it was bound. Three listeners is a cheaper
   * price than that class of bug.
   */
  useEffect(() => {
    const typing = (target: EventTarget | null) => {
      const el = target as HTMLElement | null;
      if (!el?.tagName) return false;
      return (
        el.isContentEditable ||
        el.tagName === "INPUT" ||
        el.tagName === "TEXTAREA" ||
        el.tagName === "SELECT"
      );
    };

    const down = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey) setFlipped(true);
      if (typing(e.target)) return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "b" || e.key === "B") setTool("draw");
      else if (e.key === "v" || e.key === "V") setTool("move");
      else if (e.key === "Escape") setSelected(null);
      else if (e.key === "Delete" || e.key === "Backspace") {
        if (selected === null) return;
        // The grid is not a text field, so this would otherwise be the
        // browser's Back.
        e.preventDefault();
        setSteps(erase(pattern.steps, selected));
        setSelected(null);
      } else return;
    };

    const up = (e: KeyboardEvent) => {
      if (!e.metaKey && !e.ctrlKey) setFlipped(false);
    };
    // Releasing the key somewhere else — after a Cmd-Tab, say — never reaches
    // us, and a tool stuck inside-out is a puzzle rather than a bug anyone
    // would report.
    const clear = () => setFlipped(false);

    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", clear);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", clear);
    };
  });

  /** A pattern opened in place is a different row: nothing there is selected. */
  useEffect(() => setSelected(null), [pattern.id]);

  const setMode = (mode: Mode) => {
    // Switching modes keeps the rhythm and drops what the other mode cannot
    // hold: a trigger has no pitch to remember, and a note needs one.
    const steps = pattern.steps.map((s) =>
      s.kind === "rest"
        ? s
        : mode === "drums"
          ? ({ kind: "trigger", length: s.length } as PatternStep)
          : ({ kind: "pitch", note: 60, length: s.length } as PatternStep),
    );
    onChange({ ...pattern, mode, steps });
  };

  /**
   * Which cell and lane a pointer is over, or null if it is off the grid.
   *
   * `x` comes back too, in pixels across the surface, because grabbing a note
   * by its end is a question the cell it is over cannot answer: both ends of a
   * one-cell note are inside the same cell.
   */
  const locate = (
    e: React.PointerEvent,
  ): { cell: number; lane: number; x: number } | null => {
    const box = surface.current?.getBoundingClientRect();
    if (!box) return null;
    const x = e.clientX - box.left;
    const cell = Math.floor(x / CELL_W);
    const lane = Math.floor((e.clientY - box.top) / LANE_H);
    if (cell < 0 || cell >= cells || lane < 0 || lane >= lanes.length) return null;
    return { cell, lane, x };
  };

  /**
   * The note under the pointer, in the lane the pointer is actually in.
   *
   * The lane test is what keeps a click from taking hold of whatever happens
   * to share its column somewhere else in the roll. It has to come after
   * `noteAt` rather than instead of it: every cell of a note but its first
   * holds a `null`, so a test on the clicked cell alone would miss the body of
   * every note wider than one.
   */
  const noteUnder = (at: { cell: number; lane: number }) => {
    const found = noteAt(pattern.steps, at.cell);
    if (!found) return null;
    if (drums) return found;
    return found.step.kind === "pitch" && found.step.note === lanes[at.lane] ? found : null;
  };

  const onPointerDown = (e: React.PointerEvent) => {
    const at = locate(e);
    if (!at) return;
    const found = noteUnder(at);

    // Right-click erases under either tool, which is what leaves the left
    // button free to mean something different in each.
    if (e.button === 2) {
      if (found) setSteps(erase(pattern.steps, at.cell));
      return;
    }
    if (e.button !== 0) return;

    if (active === "move") {
      setSelected(found ? found.head : null);
      if (!found) return;
      e.currentTarget.setPointerCapture(e.pointerId);
      const width = found.step.length;
      const start = found.head * CELL_W;
      const end = (found.head + width) * CELL_W;
      const edge: Edge | null =
        at.x - start <= EDGE_PX ? "start" : end - at.x <= EDGE_PX ? "end" : null;
      setGesture(
        edge
          ? { kind: "resize", head: found.head, width, edge, was: found.step, to: at.cell, lane: at.lane }
          : { kind: "move", head: found.head, width, grab: at.cell, to: at.cell, lane: at.lane },
      );
      return;
    }

    // Drawing: a click on a note erases it, and anywhere else begins one.
    if (found) {
      setSteps(erase(pattern.steps, at.cell));
      return;
    }
    e.currentTarget.setPointerCapture(e.pointerId);
    setGesture({ kind: "draw", from: at.cell, to: at.cell, lane: at.lane });
  };

  const onPointerMove = (e: React.PointerEvent) => {
    if (!gesture) return;
    const at = locate(e);
    if (!at) return;
    // A resize stays in its own lane: only a move follows the pointer's pitch.
    setGesture({
      ...gesture,
      to: at.cell,
      lane: gesture.kind === "resize" ? gesture.lane : at.lane,
    });
  };

  /** Where the gesture in progress would put its note, if there is one. */
  const going = gesture && placement(gesture, cells, lanes, drums);

  /** Applying a placement: the one edit all three gestures come down to. */
  const apply = (steps: PatternStep[], p: Placement) =>
    p.lift === null
      ? draw(steps, p.at, p.width, p.kind)
      : relocate(steps, p.lift, p.at, p.width, p.kind);

  /**
   * What the grid draws: the pattern, or — mid-gesture — what the gesture would
   * make of it.
   *
   * Showing the outcome rather than the moving note alone is what makes the
   * truncation visible while there is still time to change it: a note being
   * landed on shortens as the pointer moves, and one about to be covered
   * outright vanishes before the button is released rather than after.
   */
  const shown = going ? apply(pattern.steps, going) : pattern.steps;

  const parked = pattern.parked ?? [];

  /**
   * Which head the selection sits on as things stand.
   *
   * Mid-gesture it is wherever the gesture has put the note, so the outline
   * travels with what is being dragged rather than staying behind on the cell
   * it left. Only the move tool shows it: a selection is nothing to draw with,
   * and an outline nobody can act on reads as a note in some other state.
   */
  const marked =
    active !== "move" ? null : going && going.lift !== null ? going.at : selected;

  /**
   * Every note to draw, in the one list, each with the cell it starts at.
   *
   * The parked cells are laid out as a continuation of the row because that is
   * what they are — the grid's width is only where the two are divided — so
   * they keep the positions they had before the divider passed them, and
   * shrinking the grid looks like what it is rather than like a deletion.
   *
   * Matching the outline by start cell is also what makes a stale selection
   * harmless: a head that no longer has a note on it simply matches nothing.
   */
  const drawn = [
    ...runningStarts(shown, 0).map((n) => ({ ...n, parked: false })),
    ...runningStarts(fromCells(parked), cells).map((n) => ({ ...n, parked: true })),
  ]
    .filter((n) => n.step.kind !== "rest")
    .map((n) => ({ ...n, marked: !n.parked && n.start === marked }));

  /** The selected note as it now stands, for the resize handles to sit on. */
  const handles = drawn.find((n) => n.marked) ?? null;

  /** Which lane a step belongs in. Drums have the one, so nothing to look up. */
  const laneOf = (kind: StepKind) =>
    drums ? 0 : lanes.indexOf(kind.kind === "pitch" ? kind.note : 60);

  const setGrid = (n: number) => {
    if (!Number.isFinite(n)) return;
    onChange({ ...pattern, ...regrid(pattern, n) });
  };

  const onPointerUp = () => {
    if (!going) return;
    setSteps(apply(pattern.steps, going));
    // The note is still the one that was grabbed, so it stays selected — at
    // wherever it now is, since a head that moved is a different cell.
    if (going.lift !== null) setSelected(going.at);
    setGesture(null);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-neutral-950">
      <header className="flex flex-wrap items-center gap-3 border-b border-neutral-800 px-4 py-2">
        <input
          value={pattern.name}
          onChange={(e) => onChange({ ...pattern, name: e.target.value })}
          spellCheck={false}
          placeholder="name"
          title="The name to use in play()"
          className={
            "w-40 rounded border bg-neutral-900 px-2 py-1 font-mono text-sm text-neutral-100 " +
            "outline-none " +
            (error ? "border-red-500" : "border-neutral-700 focus:border-blue-400")
          }
        />

        <div className="flex overflow-hidden rounded border border-neutral-700">
          {(["roll", "drums"] as const).map((m) => (
            <button
              key={m}
              onClick={() => setMode(m)}
              className={
                "px-3 py-1 text-xs font-semibold uppercase tracking-wide transition-colors " +
                (pattern.mode === m
                  ? "bg-blue-400 text-white"
                  : "text-neutral-400 hover:bg-neutral-800")
              }
            >
              {m === "roll" ? "Piano roll" : "Drums"}
            </button>
          ))}
        </div>

        {/* The tool, shown as well as keyed. A mode with no visible state is
            one you find out about by surprising yourself with it — and the
            held override moves this, which is what says the key is doing
            something while it is down. */}
        <div className="flex overflow-hidden rounded border border-neutral-700">
          {(
            [
              ["draw", "Draw", "B"],
              ["move", "Move", "V"],
            ] as const
          ).map(([t, label, key]) => (
            <button
              key={t}
              onClick={() => setTool(t)}
              title={`${label} (${key})`}
              className={
                "px-3 py-1 text-xs font-semibold uppercase tracking-wide transition-colors " +
                (active === t
                  ? "bg-neutral-200 text-neutral-900"
                  : "text-neutral-400 hover:bg-neutral-800")
              }
            >
              {label}
              <span
                className={
                  "ml-1.5 font-mono text-[10px] " +
                  (active === t ? "text-neutral-500" : "text-neutral-600")
                }
              >
                {key}
              </span>
            </button>
          ))}
        </div>

        <label className="flex items-center gap-1.5 text-[11px] uppercase tracking-wide text-neutral-500">
          Grid
          <span className="flex items-center overflow-hidden rounded border border-neutral-700 bg-neutral-900 focus-within:border-blue-400">
            <button
              onClick={() => setGrid(cells - 1)}
              disabled={cells <= GRID_MIN}
              title="Narrower"
              className="px-1.5 py-1 text-xs text-neutral-400 transition-colors hover:bg-neutral-800 hover:text-neutral-100 disabled:pointer-events-none disabled:text-neutral-700"
            >
              −
            </button>
            {/* Typing a width a digit at a time passes through narrow grids on
                the way — 1, then 12 — and parks most of the row in between.
                That is only survivable because parking is reversible, and it
                is why the field can be typed into at all. */}
            <input
              type="number"
              value={cells}
              min={GRID_MIN}
              max={GRID_MAX}
              onChange={(e) => setGrid(Number(e.target.value))}
              className="w-9 border-x border-neutral-700 bg-transparent py-1 text-center font-mono text-xs text-neutral-100 outline-none [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none"
            />
            <button
              onClick={() => setGrid(cells + 1)}
              disabled={cells >= GRID_MAX}
              title="Wider"
              className="px-1.5 py-1 text-xs text-neutral-400 transition-colors hover:bg-neutral-800 hover:text-neutral-100 disabled:pointer-events-none disabled:text-neutral-700"
            >
              +
            </button>
          </span>
        </label>

        <p className="text-[11px] text-neutral-600">
          {active === "draw"
            ? "Drag to draw · click a note to erase"
            : "Drag a note to move it · its ends to resize · Delete to erase"}
          {/* Said plainly, because a greyed note that looks deleted and a
              greyed note that is coming back are the same picture until
              something says which this is. */}
          {parked.length > 0 && (
            <span className="text-neutral-500">
              {" · "}
              {parked.length} cell{parked.length === 1 ? "" : "s"} past the grid, kept
            </span>
          )}
        </p>
      </header>

      {error && (
        <p className="border-b border-red-900/50 bg-red-950/30 px-4 py-1.5 text-[11px] text-red-400">
          {error}
        </p>
      )}

      <div ref={scroller} className="min-h-0 flex-1 overflow-auto">
        <div className="flex w-max">
          {/* The keyboard, which scrolls vertically with the grid because it is
              in the same scroll box and the same flow. */}
          {!drums && (
            <div style={{ width: GUTTER }} className="sticky left-0 z-10 shrink-0 bg-neutral-950">
              {lanes.map((note) => (
                <div
                  key={note}
                  style={{ height: LANE_H }}
                  className={
                    "flex items-center justify-end border-b border-r pr-1 font-mono text-[12px] " +
                    (isAccidental(note)
                      ? "border-neutral-900 bg-neutral-950 text-neutral-600"
                      : "border-neutral-700 bg-neutral-800 text-neutral-500")
                  }
                >
                  {/* Only C is labelled, or the gutter is unreadable noise. */}
                  {note % 12 === 0 ? noteName(note) : ""}
                </div>
              ))}
            </div>
          )}

          <div
            ref={surface}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onContextMenu={(e) => e.preventDefault()}
            style={{
              width: (cells + parked.length) * CELL_W,
              height: lanes.length * LANE_H,
            }}
            className={
              "relative shrink-0 select-none " +
              (active === "draw" ? "cursor-crosshair" : "cursor-default")
            }
          >
            {/* Lanes. */}
            {lanes.map((note, i) => (
              <div
                key={note}
                style={{ top: i * LANE_H, height: LANE_H }}
                className={
                  "absolute inset-x-0 border-b border-neutral-900 " +
                  (!drums && isAccidental(note) ? "bg-neutral-900/40" : "")
                }
              />
            ))}

            {/* Cell lines, across the parked cells too — they are the same row
                continued, and drawing them on nothing would read as the row
                having ended rather than as the grid having. */}
            {Array.from({ length: cells + parked.length + 1 }, (_, i) => (
              <div
                key={i}
                style={{ left: i * CELL_W }}
                className="absolute inset-y-0 w-px bg-neutral-900"
              />
            ))}

            {/* Everything past the grid, washed out. `locate` refuses these
                cells, so the region is inert as well as grey: what is not in
                the bar cannot be drawn on until the grid is widened to take
                it back. */}
            {parked.length > 0 && (
              <>
                <div
                  style={{ left: cells * CELL_W, width: parked.length * CELL_W }}
                  className="pointer-events-none absolute inset-y-0 cursor-default bg-neutral-950/70"
                />
                <div
                  style={{ left: cells * CELL_W }}
                  className="pointer-events-none absolute inset-y-0 w-px bg-neutral-600"
                />
              </>
            )}

            {/* And the beats over them, so the bar is readable at any
                resolution. Placed at their true fraction of the bar rather
                than every nth cell, because the two need not divide — sixteen
                cells against a bar of three beats, say. A beat line between
                two cells is honest, since that is where the beat is, and one
                only ever drawn where a cell happened to fall would be silently
                wrong in 3/4. Doing it this way is also what let the grid open
                up to widths that are not powers of two: nothing here had to
                learn about 3 or 12, having never assumed 16. */}
            {Array.from({ length: Math.max(beatsPerBar - 1, 0) }, (_, i) => i + 1).map((n) => (
              <div
                key={`beat-${n}`}
                style={{ left: (n / beatsPerBar) * cells * CELL_W }}
                className="absolute inset-y-0 w-px bg-neutral-700"
              />
            ))}

            {/* The notes, as they would stand if the drag ended here, and
                behind the wash the ones the grid no longer reaches. */}
            {drawn.map(({ step, start, parked: past, marked: on }, i) => {
              const lane = laneOf(step);
              if (lane < 0) return null;
              return (
                <div
                  key={i}
                  style={{
                    left: start * CELL_W + 1,
                    top: lane * LANE_H + 1,
                    width: step.length * CELL_W - 2,
                    height: LANE_H - 2,
                  }}
                  className={
                    "absolute rounded-sm " +
                    (past
                      ? "bg-neutral-500"
                      : step.kind === "trigger"
                        ? "bg-green-500/80 hover:bg-green-400"
                        : "bg-rose-400 hover:bg-rose-300") +
                    (on ? " outline-2 outline-blue-300" : "") +
                    (active === "move" && !past ? " cursor-move" : "")
                  }
                />
              );
            })}

            {/* The selected note's ends, as somewhere to take hold of it.
                Only a cursor and a target: the gesture they begin is decided
                in `onPointerDown` from the same pixel distance, so these
                cannot promise a grab the handler would not make. Events pass
                straight through to the surface, which is where every gesture
                on this grid starts. */}
            {active === "move" &&
              handles &&
              (["start", "end"] as const).map((edge) => (
                <div
                  key={edge}
                  style={{
                    left:
                      edge === "start"
                        ? handles.start * CELL_W
                        : (handles.start + handles.step.length) * CELL_W - EDGE_PX,
                    top: laneOf(handles.step) * LANE_H,
                    width: EDGE_PX,
                    height: LANE_H,
                  }}
                  className="absolute cursor-col-resize"
                />
              ))}

            {/* Which note the gesture has hold of. It is already drawn above,
                in the colour it will keep — this only says which, so the note
                being drawn, moved or resized is told apart from the ones the
                gesture is rearranging around it. Opaque rather than a wash,
                since it covers a note rather than the empty grid. */}
            {going && (
              <div
                style={{
                  left: going.at * CELL_W + 1,
                  top: laneOf(going.kind) * LANE_H + 1,
                  width: going.width * CELL_W - 2,
                  height: LANE_H - 2,
                }}
                className="pointer-events-none absolute rounded-sm bg-blue-400"
              />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * Each step paired with the cell it begins at, counting from `offset`.
 *
 * Steps carry widths and not positions, so where one starts is the sum of
 * everything before it. Accumulating once beats re-summing the prefix per step,
 * which is what drawing the row used to do.
 */
function runningStarts(
  steps: PatternStep[],
  offset: number,
): { step: PatternStep; start: number }[] {
  let at = offset;
  return steps.map((step) => {
    const start = at;
    at += step.length;
    return { step, start };
  });
}

