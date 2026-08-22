import * as RadixSlider from "@radix-ui/react-slider";

/** Which control this is, and so which way the panel draws it. */
export type Kind = "slider" | "toggle" | "trigger";

/** One control, as the backend's `controls` command reports it. */
export interface Control {
  name: string;
  kind: Kind;
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

interface ControlsPanelProps {
  /** What to draw, and where each one is. The caller owns the position: it is
   *  the only writer between runs, which is what keeps a poll for the badge
   *  below from dragging the thumb back under somebody's finger. */
  controls: Control[];
  /** Every position as it happens: the engine takes them mid-performance. */
  onChange: (name: string, value: number) => void;
  /** The gesture is over. Only a baked control does anything with this. */
  onCommit: (control: Control) => void;
  /** A trigger was hit: fire its section. */
  onPress: (name: string) => void;
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
 * The controls a program declares, in the right panel.
 *
 * The panel draws and does not decide: there is no way to add a control here,
 * because a control exists by being written into the program at the place it
 * is used — see `src-tauri/src/controls.rs`, which is where that argument is
 * made. What is left for this to do is put them under a finger.
 *
 * Controls are listed in the order the program writes them, which is the only
 * order that is a fact about the piece. Sorting them by name, or grouping the
 * toggles apart from the sliders, would be tidier and would move a control
 * every time one was renamed or its kind changed.
 *
 * A drag reports every position as it happens, like the tempo and the volume
 * in the title bar and for the same reason: the engine takes them
 * mid-performance, so there is nothing to commit and no OK to press. The one
 * thing that waits for the end of the drag is the re-evaluation a **baked**
 * slider needs, and it waits because a run crossfades the graph — asking for
 * one per pointer event would stutter the music continuously rather than
 * making the slider feel live.
 */
export function ControlsPanel({
  controls,
  onChange,
  onCommit,
  onPress,
  onReveal,
  hasRun,
}: ControlsPanelProps) {
  if (controls.length === 0) {
    return (
      <div className="px-3 py-4 text-xs leading-relaxed text-neutral-500">
        {hasRun ? (
          <>
            <p>This program declares no controls.</p>
            <p className="mt-2">
              Write one where you want it and it appears here:
            </p>
          </>
        ) : (
          <p>Play something, and the controls it declares appear here.</p>
        )}
        {/* Short enough to read at the panel's narrowest, which is the width
            this is most likely to be seen at: an example that needs scrolling
            sideways is one nobody reads. */}
        <pre className="mt-2 overflow-x-auto rounded bg-neutral-900/70 p-2 font-mono text-[11px] text-neutral-400">
          {'slider("cutoff", 200, 5000)\ntoggle("mute")\ntrigger("fill", fill)'}
        </pre>
        <p className="mt-2">
          A slider or a toggle goes anywhere a signal goes. A trigger names a
          `fn` and plays it when hit.
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4 px-3 py-3">
      {controls.map((control) => {
        const step = stepFor(control.lo, control.hi);
        const decimals = decimalsFor(step);
        const value = control.at;
        const on = value >= 0.5;

        return (
          <div key={control.name} className="flex flex-col gap-1">
            {/* Skipped for a trigger, which wears its name on the button
                itself — a second copy of it up here would be the same word
                twice with nothing to tell the two apart. */}
            {control.kind !== "trigger" && (
            <div className="flex items-baseline justify-between gap-2">
              {/* The name is the control's identity in the program, so it is
                  also the way back to the line that wrote it. */}
              <button
                type="button"
                disabled={onReveal === null}
                onClick={() => onReveal?.(control.name)}
                title={
                  onReveal === null
                    ? control.name
                    : `Go to where ${control.name} is written`
                }
                className="min-w-0 truncate text-left text-xs text-neutral-300 transition-colors enabled:hover:text-blue-400 enabled:hover:underline"
              >
                {control.name}
              </button>
              <div className="flex shrink-0 items-baseline gap-1.5">
                {control.baked && (
                  // Not a warning: it is a fact about where this control was
                  // written, and the only thing that would make it go away is
                  // writing it somewhere else. What it saves is the minute
                  // spent moving a control that is not connected to anything
                  // until the program runs again.
                  <span
                    title="Read as a number, so its value was baked in at the last run. Letting go of the control runs the program again."
                    className="rounded bg-neutral-800 px-1 text-[10px] uppercase tracking-wide text-amber-500/90"
                  >
                    on run
                  </span>
                )}
                <span className="font-mono text-xs tabular-nums text-neutral-400">
                  {control.kind === "toggle" ? (on ? "on" : "off") : value.toFixed(decimals)}
                </span>
              </div>
            </div>
            )}

            {control.kind === "trigger" ? (
              // Wide, and wearing its own name: a trigger is hit rather than
              // set, so the whole row is the target and can be found without
              // looking while something else is being played. The name is what
              // the program calls it, which is the only label that could tell
              // one button from the next.
              <button
                type="button"
                onClick={() => onPress(control.name)}
                title={`Play ${control.name}`}
                className="w-full truncate rounded bg-neutral-800 px-3 py-1.5 text-xs text-neutral-200 transition-colors hover:bg-blue-500 hover:text-white active:bg-blue-600 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-400"
              >
                {control.name}
              </button>
            ) : control.kind === "toggle" ? (
              // A switch rather than a two-step slider, because the two mean
              // different things: a slider asks how much and a toggle asks
              // whether. Drawn as the track a thumb slides across so it reads
              // as a sibling of the sliders above and below it rather than as
              // a stray checkbox.
              <button
                type="button"
                role="switch"
                aria-checked={on}
                aria-label={control.name}
                onClick={() => {
                  onChange(control.name, on ? 0 : 1);
                  onCommit(control);
                }}
                className={
                  "relative my-0.5 h-5 w-9 shrink-0 rounded-full transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-400 " +
                  (on ? "bg-blue-400" : "bg-neutral-700")
                }
              >
                <span
                  className={
                    "absolute top-1 h-3 w-3 rounded-full bg-neutral-200 transition-all " +
                    (on ? "left-5" : "left-1")
                  }
                />
              </button>
            ) : (
              <>
                {/* Radix rather than an `input[type=range]`, as in the title
                    bar's controls: the track, the fill and the thumb are
                    ordinary elements here, which is the only way this fill can
                    be coloured at all. */}
                <RadixSlider.Root
                  value={[value]}
                  min={control.lo}
                  max={control.hi}
                  step={step}
                  aria-label={control.name}
                  onValueChange={([next]) => onChange(control.name, next)}
                  onValueCommit={() => onCommit(control)}
                  className="relative flex h-4 w-full touch-none select-none items-center"
                >
                  <RadixSlider.Track className="relative h-1.5 w-full grow rounded-full bg-neutral-700">
                    <RadixSlider.Range className="absolute h-full rounded-full bg-blue-400" />
                  </RadixSlider.Track>
                  <RadixSlider.Thumb className="block h-3.5 w-3.5 rounded-full bg-neutral-200 shadow transition-colors hover:bg-white focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-400" />
                </RadixSlider.Root>

                {/* The ends, so the range the slider covers is not a guess —
                    the same thing the title bar's popover says under its
                    slider. A toggle needs none: its ends are its two states,
                    and the readout above already names the one it is in. */}
                <div className="flex justify-between font-mono text-[10px] text-neutral-600">
                  <span>{control.lo}</span>
                  <span>{control.hi}</span>
                </div>
              </>
            )}
          </div>
        );
      })}
    </div>
  );
}
