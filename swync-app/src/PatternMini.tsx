import { isAccidental, resolution, ROLL_HIGH, ROLL_LOW, type GraphicalPattern } from "./patterns";

/**
 * A pattern at a glance: blocks laid out along one bar, sized by how long
 * each note actually is.
 *
 * This is a picture, not an editor — it exists so the right panel can show
 * *which* pattern is which without becoming a second place to change them.
 * Everything is proportional to the real lengths, so a dotted note looks
 * dotted; that is the whole point of showing it rather than naming it.
 */

/** Tall enough to tell two pitches apart, short enough to list a dozen. */
const HEIGHT = 34;
/** A note is drawn at least this wide however fine the grid is, or a 32nd on a
 *  narrow panel would be a sub-pixel sliver and read as nothing at all. */
const MIN_WIDTH_PCT = 1.5;

interface PatternMiniProps {
  pattern: GraphicalPattern;
  /** Beats in a bar, so the guide lines match the project's signature. */
  beatsPerBar: number;
}

export function PatternMini({ pattern, beatsPerBar }: PatternMiniProps) {
  const cells = resolution(pattern);
  const drums = pattern.mode === "drums";

  // Pitches are placed against the range actually used rather than the roll's
  // full compass: a bassline drawn across four octaves would otherwise sit in
  // the bottom eighth of the strip and read as one flat line.
  const pitches = pattern.steps.flatMap((s) => (s.kind === "pitch" ? [s.note] : []));
  const low = pitches.length > 0 ? Math.min(...pitches) : ROLL_LOW;
  const high = pitches.length > 0 ? Math.max(...pitches) : ROLL_HIGH;
  const span = Math.max(high - low, 1);

  let at = 0;
  const blocks = pattern.steps.map((step, i) => {
    const start = at;
    at += step.length;
    if (step.kind === "rest") return null;

    // A drum row is one lane; a roll puts pitch on the vertical.
    const top = drums || step.kind !== "pitch" ? 0.35 : 1 - (step.note - low) / span;
    return (
      <div
        key={i}
        style={{
          left: `${(start / cells) * 100}%`,
          width: `${Math.max((step.length / cells) * 100, MIN_WIDTH_PCT)}%`,
          top: `${top * (HEIGHT - 6)}px`,
        }}
        className={
          "absolute h-1.5 rounded-sm " +
          (step.kind === "trigger"
            ? "bg-green-500/80"
            : isAccidental(step.note)
              ? "bg-rose-400/70"
              : "bg-rose-400")
        }
      />
    );
  });

  return (
    <div
      style={{ height: HEIGHT }}
      className="relative w-full overflow-hidden rounded border border-neutral-800 bg-neutral-950"
    >
      {/* Beat lines, so the eye has something to place the blocks against.
          As many as the signature says are in a bar — three in 3/4, six in
          6/8 — since a strip drawn in four would claim a bar this project
          does not have. */}
      {Array.from({ length: Math.max(beatsPerBar - 1, 0) }, (_, i) => i + 1).map((n) => (
        <div
          key={n}
          style={{ left: `${(n / beatsPerBar) * 100}%` }}
          className="absolute inset-y-0 w-px bg-neutral-800/70"
        />
      ))}
      {blocks}
    </div>
  );
}
