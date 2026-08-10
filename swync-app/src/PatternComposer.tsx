import { useEffect, useRef, useState } from "react";
import {
  MIDDLE_C,
  RESOLUTIONS,
  ROLL_HIGH,
  ROLL_LOW,
  draw,
  erase,
  isAccidental,
  noteName,
  resize,
  resolution,
  toCells,
  type GraphicalPattern,
  type Mode,
  type PatternStep,
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
 */

/** One cell's width, and a lane's height, in pixels. */
const CELL_W = 22;
const LANE_H = 13;
/** The keyboard gutter down the left of the roll. */
const GUTTER = 44;

interface DragState {
  /** Cell the drag began in. */
  from: number;
  /** Cell it is currently over. */
  to: number;
  lane: number;
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
  const grid = toCells(pattern.steps);
  const [drag, setDrag] = useState<DragState | null>(null);
  const surface = useRef<HTMLDivElement>(null);
  const scroller = useRef<HTMLDivElement>(null);

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

  /** Which cell and lane a pointer is over, or null if it is off the grid. */
  const locate = (e: React.PointerEvent): { cell: number; lane: number } | null => {
    const box = surface.current?.getBoundingClientRect();
    if (!box) return null;
    const cell = Math.floor((e.clientX - box.left) / CELL_W);
    const lane = Math.floor((e.clientY - box.top) / LANE_H);
    if (cell < 0 || cell >= cells || lane < 0 || lane >= lanes.length) return null;
    return { cell, lane };
  };

  const onPointerDown = (e: React.PointerEvent) => {
    const at = locate(e);
    if (!at) return;
    // Right-click, or a click on a note that is already there, erases.
    //
    // `noteAt` walks back to the head itself, so it must not be guarded by a
    // test on the clicked cell: every cell of a note but its first holds a
    // `null`, and rejecting those first made clicking the body of a note draw
    // a new one inside it.
    if (e.button === 2 || noteAt(grid, at, lanes, drums)) {
      setSteps(erase(pattern.steps, at.cell));
      return;
    }
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ from: at.cell, to: at.cell, lane: at.lane });
  };

  const onPointerMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const at = locate(e);
    if (at) setDrag({ ...drag, to: at.cell });
  };

  const onPointerUp = () => {
    if (!drag) return;
    const from = Math.min(drag.from, drag.to);
    const width = Math.abs(drag.to - drag.from) + 1;
    setSteps(
      draw(
        pattern.steps,
        from,
        width,
        drums ? { kind: "trigger" } : { kind: "pitch", note: lanes[drag.lane] },
      ),
    );
    setDrag(null);
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

        <label className="flex items-center gap-1.5 text-[11px] uppercase tracking-wide text-neutral-500">
          Grid
          <select
            value={cells}
            onChange={(e) => setSteps(resize(pattern.steps, Number(e.target.value)))}
            className="rounded border border-neutral-700 bg-neutral-900 px-1.5 py-1 font-mono text-xs text-neutral-100 outline-none focus:border-blue-400"
          >
            {(RESOLUTIONS as readonly number[]).map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
            {/* A hand-written row may be at a width the menu does not offer;
                showing it beats silently snapping someone's pattern. */}
            {!(RESOLUTIONS as readonly number[]).includes(cells) && (
              <option value={cells}>{cells}</option>
            )}
          </select>
        </label>

        <p className="text-[11px] text-neutral-600">
          Drag to draw · click a note to erase
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
            style={{ width: cells * CELL_W, height: lanes.length * LANE_H }}
            className="relative shrink-0 cursor-crosshair select-none"
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

            {/* Cell lines. */}
            {Array.from({ length: cells + 1 }, (_, i) => (
              <div
                key={i}
                style={{ left: i * CELL_W }}
                className="absolute inset-y-0 w-px bg-neutral-900"
              />
            ))}

            {/* And the beats over them, so the bar is readable at any
                resolution. Placed at their true fraction of the bar rather
                than every nth cell, because the two need not divide: the grids
                on offer are powers of two and a bar of three beats divides
                none of them. A beat line between two cells is honest — it is
                where the beat is — and a beat line only ever drawn where a
                cell happened to fall would be silently wrong in 3/4. */}
            {Array.from({ length: Math.max(beatsPerBar - 1, 0) }, (_, i) => i + 1).map((n) => (
              <div
                key={`beat-${n}`}
                style={{ left: (n / beatsPerBar) * cells * CELL_W }}
                className="absolute inset-y-0 w-px bg-neutral-700"
              />
            ))}

            {/* The notes. */}
            {pattern.steps.map((step, i) => {
              const start = pattern.steps.slice(0, i).reduce((n, s) => n + s.length, 0);
              if (step.kind === "rest") return null;
              const lane = drums ? 0 : lanes.indexOf(step.kind === "pitch" ? step.note : 60);
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
                    (step.kind === "trigger"
                      ? "bg-green-500/80 hover:bg-green-400"
                      : "bg-rose-400 hover:bg-rose-300")
                  }
                />
              );
            })}

            {/* What is being dragged, before it is committed. */}
            {drag && (
              <div
                style={{
                  left: Math.min(drag.from, drag.to) * CELL_W + 1,
                  top: drag.lane * LANE_H + 1,
                  width: (Math.abs(drag.to - drag.from) + 1) * CELL_W - 2,
                  height: LANE_H - 2,
                }}
                className="pointer-events-none absolute rounded-sm bg-blue-400/60"
              />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * Whether there is a note under `at`, in the lane the pointer is actually in.
 *
 * Walks back to the head that owns the cell, since only the head carries what
 * the note is. The lane test is what keeps a click from erasing whatever
 * happens to share its column in a different part of the roll.
 */
function noteAt(
  grid: (PatternStep | null)[],
  at: { cell: number; lane: number },
  lanes: number[],
  drums: boolean,
): boolean {
  let head = at.cell;
  while (head > 0 && grid[head] === null) head -= 1;
  const step = grid[head];
  if (!step || step.kind === "rest") return false;
  if (drums) return true;
  return step.kind === "pitch" && lanes[at.lane] === step.note;
}
