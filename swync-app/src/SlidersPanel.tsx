import * as RadixSlider from "@radix-ui/react-slider";

/** One control, as the backend's `sliders` command reports it. */
export interface Slider {
  name: string;
  slot: number;
  lo: number;
  hi: number;
  /** Where the program says it starts. Only the first sight of a name uses it. */
  start: number;
  at: number;
  /** Read as a number somewhere, so what the graph holds was baked at the last
   *  run. See the badge below, and `controls.rs`. */
  baked: boolean;
}

interface SlidersPanelProps {
  /** What to draw, and where each one is. The caller owns the position: it is
   *  the only writer between runs, which is what keeps a poll for the badge
   *  below from dragging the thumb back under somebody's finger. */
  sliders: Slider[];
  /** Every position as it happens: the engine takes them mid-performance. */
  onChange: (name: string, value: number) => void;
  /** The drag is over. Only a baked slider does anything with this. */
  onCommit: (slider: Slider) => void;
  /** Show me where this was written. Null when there is no project to search. */
  onReveal: ((name: string) => void) | null;
  /** Whether a program has been run at all this session, which is the
   *  difference between "this piece has no sliders" and "nothing has looked". */
  hasRun: boolean;
}

/**
 * How finely a slider moves under the pointer.
 *
 * A thousandth of the travel, rounded down to a power of ten — so 0 to 1 moves
 * in thousandths and 200 to 5000 moves in ones. A step of the *same* size for
 * every range would be either uselessly coarse at the small end or an
 * unreadable pile of decimals at the large one, and the number of decimals to
 * print falls out of the same figure.
 */
function stepFor(lo: number, hi: number): number {
  const span = Math.abs(hi - lo);
  if (!(span > 0)) return 1;
  return Math.pow(10, Math.floor(Math.log10(span / 1000)));
}

/** The decimals a step is worth printing to, so a readout reads as it drags. */
function decimalsFor(step: number): number {
  return Math.max(0, Math.min(6, Math.ceil(-Math.log10(step))));
}

/**
 * The sliders a program declares, in the right panel.
 *
 * The panel draws and does not decide: there is no way to add a control here,
 * because a control exists by being written into the program at the place it
 * is used — see `src-tauri/src/controls.rs`, which is where that argument is
 * made. What is left for this to do is put them under a finger.
 *
 * Sliders are listed in the order the program writes them, which is the only
 * order that is a fact about the piece. Sorting them by name would be tidier
 * and would move a control every time one was renamed.
 *
 * A drag reports every position as it happens, like the tempo and the volume
 * in the title bar and for the same reason: the engine takes them
 * mid-performance, so there is nothing to commit and no OK to press. The one
 * thing that waits for the end of the drag is the re-evaluation a **baked**
 * slider needs, and it waits because a run crossfades the graph — asking for
 * one per pointer event would stutter the music continuously rather than
 * making the slider feel live.
 */
export function SlidersPanel({
  sliders,
  onChange,
  onCommit,
  onReveal,
  hasRun,
}: SlidersPanelProps) {
  if (sliders.length === 0) {
    return (
      <div className="px-3 py-4 text-xs leading-relaxed text-neutral-500">
        {hasRun ? (
          <>
            <p>This program declares no sliders.</p>
            <p className="mt-2">
              Write one where you want it and it appears here:
            </p>
          </>
        ) : (
          <p>Play something, and the sliders it declares appear here.</p>
        )}
        {/* Short enough to read at the panel's narrowest, which is the width
            this is most likely to be seen at: an example that needs scrolling
            sideways is one nobody reads. */}
        <pre className="mt-2 overflow-x-auto rounded bg-neutral-900/70 p-2 font-mono text-[11px] text-neutral-400">
          slider("cutoff", 200, 5000)
        </pre>
        <p className="mt-2">
          Anywhere a signal goes. The range defaults to 0 to 1, and a fourth
          number says where it starts.
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4 px-3 py-3">
      {sliders.map((slider) => {
        const step = stepFor(slider.lo, slider.hi);
        const decimals = decimalsFor(step);
        const value = slider.at;

        return (
          <div key={slider.name} className="flex flex-col gap-1">
            <div className="flex items-baseline justify-between gap-2">
              {/* The name is the control's identity in the program, so it is
                  also the way back to the line that wrote it. */}
              <button
                type="button"
                disabled={onReveal === null}
                onClick={() => onReveal?.(slider.name)}
                title={
                  onReveal === null
                    ? slider.name
                    : `Go to where ${slider.name} is written`
                }
                className="min-w-0 truncate text-left text-xs text-neutral-300 transition-colors enabled:hover:text-blue-400 enabled:hover:underline"
              >
                {slider.name}
              </button>
              <div className="flex shrink-0 items-baseline gap-1.5">
                {slider.baked && (
                  // Not a warning: it is a fact about where this slider was
                  // written, and the only thing that would make it go away is
                  // writing it somewhere else. What it saves is the minute
                  // spent dragging a control that is not connected to
                  // anything until the program runs again.
                  <span
                    title="Read as a number, so its value was baked in at the last run. Letting go of the slider runs the program again."
                    className="rounded bg-neutral-800 px-1 text-[10px] uppercase tracking-wide text-amber-500/90"
                  >
                    on run
                  </span>
                )}
                <span className="font-mono text-xs tabular-nums text-neutral-400">
                  {value.toFixed(decimals)}
                </span>
              </div>
            </div>

            {/* Radix rather than an `input[type=range]`, as in the title bar's
                controls: the track, the fill and the thumb are ordinary
                elements here, which is the only way this fill can be coloured
                at all. */}
            <RadixSlider.Root
              value={[value]}
              min={slider.lo}
              max={slider.hi}
              step={step}
              aria-label={slider.name}
              onValueChange={([next]) => onChange(slider.name, next)}
              onValueCommit={() => onCommit(slider)}
              className="relative flex h-4 w-full touch-none select-none items-center"
            >
              <RadixSlider.Track className="relative h-1.5 w-full grow rounded-full bg-neutral-700">
                <RadixSlider.Range className="absolute h-full rounded-full bg-blue-400" />
              </RadixSlider.Track>
              <RadixSlider.Thumb
                className="block h-3.5 w-3.5 rounded-full bg-neutral-200 shadow transition-colors hover:bg-white focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-400"
              />
            </RadixSlider.Root>

            {/* The ends, so the range the slider covers is not a guess — the
                same thing the title bar's popover says under its slider. */}
            <div className="flex justify-between font-mono text-[10px] text-neutral-600">
              <span>{slider.lo}</span>
              <span>{slider.hi}</span>
            </div>
          </div>
        );
      })}
    </div>
  );
}
