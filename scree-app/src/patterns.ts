/**
 * Patterns drawn in the composer.
 *
 * A drawn pattern is exactly what a list literal is — a row of steps, each one
 * a pitch, a rest or a bare trigger, each covering some number of grid cells —
 * so the backend can bind its name to the same `Value::List` the language would
 * have built from `[c4;2, `;2, e4;4]`. That is the whole trick: nothing
 * downstream needs to know a pattern was drawn.
 *
 * The `length` on a step is in *cells*, not bars. A note four cells wide is
 * written `;4`, and a row's cells always sum to the resolution it was drawn at
 * — which is why the resolution is derived rather than stored. See
 * `src-tauri/src/pattern/graphical.rs`, which this file is the other half of.
 */

/** What a step is, independent of how wide it is. */
export type StepKind =
  | { kind: "rest" }
  | { kind: "trigger" }
  | { kind: "pitch"; note: number };

/** One step: what it is, plus how many cells it covers. */
export type PatternStep = StepKind & { length: number };

/** Which editor a pattern is drawn in. */
export type Mode = "roll" | "drums";

export interface GraphicalPattern {
  /** Stable across renames — the name is the user's, this is React's. */
  id: string;
  name: string;
  steps: PatternStep[];
  mode: Mode;
}

/** Grid widths the composer offers, in cells per bar. A cell is a share of the
 *  bar rather than a note value, so in 4/4 the default 16 is sixteenth notes
 *  and in 3/4 the same grid is sixteen to a three-beat bar. Powers of two
 *  because that is what a rhythm drawn on a grid wants; the beat lines are
 *  placed by fraction rather than by cell, which is what lets the two disagree. */
export const RESOLUTIONS = [4, 8, 16, 32] as const;
export const DEFAULT_RESOLUTION = 16;

/**
 * The range of pitches the roll draws, as MIDI numbers: A0 to C8.
 *
 * The 88 keys of a piano, so the roll never runs out from under a part. It is
 * far taller than any window, which is the point — the roll scrolls, and opens
 * centred on whatever the pattern already holds rather than at one end of a
 * compass nobody uses all of.
 */
export const ROLL_LOW = 21;
export const ROLL_HIGH = 108;

/** Where an empty roll opens: middle C, in the middle of the view. */
export const MIDDLE_C = 60;

/** What the backend will accept as a name it can bind. Mirrors `check_names`
 *  in `src-tauri/src/pattern/graphical.rs`. */
const IDENT = /^[a-zA-Z_]\w*$/;

/**
 * Why this pattern's name cannot be used, or null when it can.
 *
 * `all` is every pattern including this one, so the duplicate test is against
 * the others by id rather than by position.
 */
export function nameError(p: GraphicalPattern, all: GraphicalPattern[]): string | null {
  if (p.name.trim() === "") return "needs a name";
  if (!IDENT.test(p.name)) return "letters, digits and _ only, not starting with a digit";
  if (all.some((q) => q.id !== p.id && q.name === p.name)) return "already taken";
  return null;
}

/** How many cells wide a pattern's grid is: its steps always sum to it. */
export function resolution(p: GraphicalPattern): number {
  const total = p.steps.reduce((n, s) => n + s.length, 0);
  return total > 0 ? total : DEFAULT_RESOLUTION;
}

let counter = 0;

/** A new pattern, named so it does not collide with the ones already there. */
export function makePattern(existing: GraphicalPattern[], mode: Mode = "roll"): GraphicalPattern {
  const taken = new Set(existing.map((p) => p.name));
  let name: string;
  do {
    counter += 1;
    name = `pat${counter}`;
  } while (taken.has(name));

  return { id: `gp-${counter}-${Date.now()}`, name, mode, steps: emptyRow(DEFAULT_RESOLUTION) };
}

/** A row of nothing, at the given width. */
export function emptyRow(cells: number): PatternStep[] {
  return Array.from({ length: cells }, () => ({ kind: "rest", length: 1 }) as PatternStep);
}

/**
 * The steps as one cell per entry, which is what a grid draws against.
 *
 * A note four cells wide becomes its head followed by three `null`s — the head
 * carries the note and its width, and the nulls mark cells that are spoken for.
 * Drawing wants to ask "what is in cell 7", and the stored form answers "what
 * is the 7th step", which is a different question once widths differ.
 */
export function toCells(steps: PatternStep[]): (PatternStep | null)[] {
  const out: (PatternStep | null)[] = [];
  for (const step of steps) {
    out.push(step);
    for (let i = 1; i < Math.round(step.length); i += 1) out.push(null);
  }
  return out;
}

/**
 * The inverse: a grid of cells back into steps.
 *
 * A head's length is *recounted* from the run of `null`s after it rather than
 * trusted, which is what makes this a true inverse of `toCells` — the head
 * arrives still carrying the length that produced those nulls, so adding to it
 * would count every cell twice. Recounting also means a grid assembled by
 * hand, as `draw` and `resize` do, needs no separate bookkeeping to stay
 * consistent: the nulls are the single source of truth about width.
 *
 * A leading `null` — which should not happen — is treated as a rest, so a
 * corrupt grid still reads as something.
 */
export function fromCells(cells: (PatternStep | null)[]): PatternStep[] {
  const out: PatternStep[] = [];
  for (const cell of cells) {
    if (cell === null && out.length > 0) {
      out[out.length - 1] = { ...out[out.length - 1], length: out[out.length - 1].length + 1 };
      continue;
    }
    out.push(cell === null ? { kind: "rest", length: 1 } : { ...cell, length: 1 });
  }
  return out;
}

/**
 * Put `step` at `at`, `width` cells wide, overwriting whatever it covers.
 *
 * Anything the new note lands on is cut back to a rest rather than trimmed:
 * drawing over a note replaces it, which is what a roll does, and half a note
 * left behind is rarely what anyone meant. The grid keeps its width, so what
 * gets pushed off the end is lost — the bar is a fixed length.
 */
export function draw(
  steps: PatternStep[],
  at: number,
  width: number,
  step: StepKind,
): PatternStep[] {
  const cells = toCells(steps);
  const size = cells.length;
  const span = Math.max(1, Math.min(width, size - at));

  // Anything whose head is covered loses its tail too, or the tail would be
  // orphaned `null`s belonging to a note that is no longer there.
  for (let i = at; i < at + span && i < size; i += 1) {
    if (cells[i] === null) {
      // Walk back to the head that owns this cell and clear its whole run.
      let head = i;
      while (head > 0 && cells[head] === null) head -= 1;
      for (let j = head; j < size && (j === head || cells[j] === null); j += 1) {
        cells[j] = { kind: "rest", length: 1 };
      }
      continue;
    }
    // Clear this head's own tail before overwriting it.
    for (let j = i + 1; j < size && cells[j] === null; j += 1) {
      cells[j] = { kind: "rest", length: 1 };
    }
    cells[i] = { kind: "rest", length: 1 };
  }

  cells[at] = { ...step, length: span };
  for (let i = at + 1; i < at + span; i += 1) cells[i] = null;
  return fromCells(cells);
}

/** Clear the note covering `at`, head and tail together. */
export function erase(steps: PatternStep[], at: number): PatternStep[] {
  const cells = toCells(steps);
  let head = at;
  while (head > 0 && cells[head] === null) head -= 1;
  for (let j = head; j < cells.length && (j === head || cells[j] === null); j += 1) {
    cells[j] = { kind: "rest", length: 1 };
  }
  return fromCells(cells);
}

/** Grow or shrink the grid, keeping whatever fits. Notes crossing the new end
 *  are cut back so nothing claims a cell that no longer exists. */
export function resize(steps: PatternStep[], cells: number): PatternStep[] {
  const grid = toCells(steps).slice(0, cells);
  while (grid.length < cells) grid.push({ kind: "rest", length: 1 });
  // A `null` at the very front means its head was cut off the end of the slice.
  while (grid.length > 0 && grid[0] === null) grid[0] = { kind: "rest", length: 1 };
  return fromCells(grid);
}

/** Sharps rather than flats, matching how `lang::note` spells them: `cs4`. */
const NOTE_NAMES = ["c", "cs", "d", "ds", "e", "f", "fs", "g", "gs", "a", "as", "b"];

/** The lowest and highest MIDI numbers `lang::note` can spell: `c0` to `g9`. */
const MIN_NOTE = 12;
const MAX_NOTE = 127;

/** True for the black keys, which the roll shades. */
export function isAccidental(note: number): boolean {
  return NOTE_NAMES[((note % 12) + 12) % 12].length === 2;
}

/** A MIDI number as the language would write it: `60` is `c4`. */
export function noteName(note: number): string {
  if (note < MIN_NOTE || note > MAX_NOTE || !Number.isInteger(note)) return String(note);
  return `${NOTE_NAMES[note % 12]}${Math.floor(note / 12) - 1}`;
}

const SPELLED = /^([a-g])([sf]?)([0-9])$/;
const OFFSETS: Record<string, number> = { c: 0, d: 2, e: 4, f: 5, g: 7, a: 9, b: 11 };

/**
 * Read a cell's text as a MIDI note, or null if it is not one.
 *
 * Both spellings the language accepts work — a note name (`a2`, `fs3`, `ef4`)
 * or a bare MIDI number — because both are things a user of this language
 * already writes.
 */
export function parseNote(text: string): number | null {
  const s = text.trim().toLowerCase();
  if (s === "") return null;

  if (/^\d+(\.\d+)?$/.test(s)) {
    const n = Number(s);
    return n >= 0 && n <= MAX_NOTE ? n : null;
  }

  const m = SPELLED.exec(s);
  if (!m) return null;
  const [, letter, accidental, octave] = m;
  const semitone = OFFSETS[letter] + (accidental === "s" ? 1 : accidental === "f" ? -1 : 0);
  const note = (Number(octave) + 1) * 12 + semitone;
  return note >= MIN_NOTE && note <= MAX_NOTE ? note : null;
}

/** The shape `run_code` deserializes. Ids and edit state stay on this side. */
export interface WirePattern {
  name: string;
  steps: PatternStep[];
  mode: Mode;
}

/** Everything the backend can bind. A pattern whose name it would refuse is
 *  left out, so a half-typed name cannot fail an otherwise good eval — and, for
 *  the same reason, cannot reach the project's patterns file. */
export function toWire(patterns: GraphicalPattern[]): WirePattern[] {
  return patterns
    .filter((p) => nameError(p, patterns) === null)
    .map((p) => ({ name: p.name, steps: p.steps, mode: p.mode }));
}

/**
 * Rows read back from the project's `patterns.scree`.
 *
 * The ids are minted here because they never existed on disk: they are this
 * side's handle on a row across a rename, and the file has no use for them.
 *
 * `held` is what the panel is showing already, and a row in it that this file
 * also names keeps the id it has. The file is read again far more often than
 * it changes — the project's folder is watched, so anything appearing in it
 * re-reads this — and a composer tab holds its pattern by id. Minting a new
 * one for a row that is the same row would take the pattern out from under an
 * open tab, which is how "This pattern no longer exists" appears over a
 * pattern that plainly does.
 *
 * By name, because that is what the file records and what the tab is restored
 * by across a launch. A row renamed outside the app is a new row by that
 * measure, which is the honest answer: nothing on disk connects it to the old
 * one.
 */
export function fromWire(
  wire: WirePattern[],
  held: GraphicalPattern[] = [],
): GraphicalPattern[] {
  return wire.map((p) => {
    const already = held.find((h) => h.name === p.name);
    if (!already) counter += 1;
    return {
      id: already?.id ?? `gp-${counter}-${Date.now()}`,
      name: p.name,
      // A file written before lengths existed has none; one cell each.
      steps: p.steps.map((s) => ({ ...s, length: s.length ?? 1 })),
      mode: p.mode ?? "roll",
    };
  });
}
