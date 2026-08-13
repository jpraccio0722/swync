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
  /**
   * Cells pushed off the end by shrinking the grid, kept so that growing it
   * back brings them with it.
   *
   * Raw cells rather than steps, and that is the whole reason this is
   * recoverable: `steps` and `parked` laid end to end are one continuous cell
   * array, and a grid width is nothing but where that array is divided. Moving
   * the divider is therefore exactly reversible — including through a note
   * that straddles it, whose surviving `null`s stay on this side and rejoin
   * their head when the divider moves back.
   *
   * It never reaches the backend: `toWire` sends `steps` alone, so what is
   * parked cannot sound and cannot reach `patterns.swync`. That file is real
   * swync — anything inside the list literal *plays* — so the only honest
   * place for a cell that is deliberately silent is this side of the wire.
   * The cost is that it lives no longer than the session, which is the right
   * trade for what it is: protection against a slip of the stepper, not a
   * property of the piece.
   */
  parked?: (PatternStep | null)[];
}

/** How wide a new pattern's grid is, in cells per bar. A cell is a share of
 *  the bar rather than a note value, so in 4/4 sixteen cells are sixteenth
 *  notes and in 3/4 the same grid is sixteen to a three-beat bar. */
export const DEFAULT_RESOLUTION = 16;

/**
 * What the grid's stepper will go to.
 *
 * Any whole number, not just the powers of two the composer used to offer,
 * which makes 3, 5, 6 and 12 reachable — the grids a triplet or a quintuplet
 * wants. Nothing in the drawing had to change for that: the beat lines were
 * already placed at their true fraction of the bar rather than every nth cell,
 * precisely so the grid and the meter could disagree.
 *
 * The ceiling is where a bar stops being legible at one cell per 22px rather
 * than anywhere the model strains; the floor is one, since a grid of no cells
 * is not a bar.
 */
export const GRID_MIN = 1;
export const GRID_MAX = 64;

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
 * Put `step` at `at`, `width` cells wide, over whatever it covers.
 *
 * A note the new one lands on is *truncated* rather than removed: it keeps its
 * head and ends where the new note begins. Only a note whose head is itself
 * covered disappears, because there is nothing left of it to shorten. Grazing
 * the tail of a long note used to destroy the whole note, which is rarely what
 * the drag meant — and truncation is not a compromise here but the right
 * reading, since a row is monophonic and a note ending as the next begins is
 * exactly what it already sounds like.
 *
 * Truncating takes no work of its own. The head is left where it is and
 * `fromCells` recounts its length from the run of `null`s still in front of
 * the new note. What does need clearing is the run *after* it — the orphaned
 * tail of a note whose head was covered, or the right half of a note drawn
 * through the middle. Left alone those `null`s would read as part of the new
 * note and silently widen it past the drag.
 *
 * The grid keeps its width, so a note is clamped rather than pushed off the
 * end: the bar is a fixed length.
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

  cells[at] = { ...step, length: span };
  for (let i = at + 1; i < at + span; i += 1) cells[i] = null;
  for (let i = at + span; i < size && cells[i] === null; i += 1) {
    cells[i] = { kind: "rest", length: 1 };
  }
  return fromCells(cells);
}

/**
 * The note covering `cell`, or null where there is a rest.
 *
 * Walks back to the head, because only the head carries what the note is and
 * how wide it is — every other cell of it holds a `null`. Asking about the
 * body of a note therefore answers about the whole note, which is what every
 * caller wants: a pointer lands on a cell but grabs a note.
 */
export function noteAt(
  steps: PatternStep[],
  cell: number,
): { head: number; step: PatternStep } | null {
  const cells = toCells(steps);
  if (cell < 0 || cell >= cells.length) return null;
  let head = cell;
  while (head > 0 && cells[head] === null) head -= 1;
  const step = cells[head];
  if (!step || step.kind === "rest") return null;
  return { head, step };
}

/**
 * Lift the note whose head is at `head`, and put it down at `at`, `width`
 * cells wide, as `kind`.
 *
 * Moving a note and resizing one are the same act expressed twice — both lift
 * a note and put it somewhere — so they are one function, and neither had to
 * be taught what happens on landing. `draw` already knows: what the note comes
 * down on is truncated, and only what it covers outright is replaced. Lifting
 * first is what makes a move over a note's own old cells work, and what keeps
 * a note from truncating itself when it is only growing.
 */
export function relocate(
  steps: PatternStep[],
  head: number,
  at: number,
  width: number,
  kind: StepKind,
): PatternStep[] {
  return draw(erase(steps, head), at, width, kind);
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

/**
 * Set the grid to `cells` wide, parking whatever no longer fits.
 *
 * Growing and shrinking are the same act here, which is what makes the stepper
 * safe to hold down: the row's cells and its parked cells are one array, and
 * this only moves the divider between them. Nothing is discarded at either
 * end, so shrinking to 4 and back to 16 returns the row that was there —
 * including a note the divider passed through, whose head keeps the `null`s on
 * its own side and takes back the rest when the divider returns.
 *
 * Growing past everything there is pads with rests, since a bar has to be full
 * — the cells of a row always sum to its width, which is what lets
 * `resolution` be derived rather than stored.
 */
export function regrid(
  pattern: GraphicalPattern,
  cells: number,
): { steps: PatternStep[]; parked: (PatternStep | null)[] } {
  const width = Math.max(GRID_MIN, Math.min(GRID_MAX, Math.round(cells)));
  const full = toCells(pattern.steps).concat(pattern.parked ?? []);
  while (full.length < width) full.push({ kind: "rest", length: 1 });
  return { steps: fromCells(full.slice(0, width)), parked: full.slice(width) };
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
 * Rows read back from the project's `patterns.swync`.
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
 *
 * What is `parked` rides along for the same reason and by the same match. The
 * file cannot record it — those cells are deliberately not in the list — so a
 * re-read is exactly the moment it would otherwise be lost, and the project's
 * folder is watched, which makes re-reads far more common than edits. A row
 * edited by hand keeps whatever the stepper had parked for it; the two cannot
 * contradict each other, since a parked cell makes no claim about the row
 * beyond sitting past the end of it.
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
      parked: already?.parked,
    };
  });
}
