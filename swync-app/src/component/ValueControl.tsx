import { useCallback, useState } from "react";
import * as Popover from "@radix-ui/react-popover";
import * as RadixSlider from "@radix-ui/react-slider";

interface ValueControlProps {
  /** What the value is, shown on the button and read out to screen readers. */
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  /** Written after the number, on the button and under the slider. */
  unit: string;
  onChange: (value: number) => void;
}

/** Round to the control's step, so a dragged value reads the way it is typed. */
function quantise(value: number, step: number): number {
  const snapped = Math.round(value / step) * step;
  // Steps below 1 leave binary fractions behind; the decimals the step needs
  // are all the precision there is to keep.
  const decimals = Math.max(0, Math.ceil(-Math.log10(step)));
  return Number(snapped.toFixed(decimals));
}

/**
 * A number in the title bar, editable in place, with a slider behind a chevron.
 *
 * The number on the button *is* the field: reading it and typing over it are
 * the same gesture, so the common edit — you know the tempo you want — costs
 * no clicks at all. The chevron opens the slider for the other kind of edit,
 * the one where you are listening rather than deciding.
 *
 * The field cannot be inside the button element itself: a text input nested in
 * a button is invalid, and the two would fight over every click. So the button
 * look belongs to the row, and the chevron is the real control — which also
 * keeps typing from opening a panel nobody asked for.
 *
 * Both edits report every change as it happens; the engine takes them
 * mid-performance, so there is nothing to commit and no OK to press.
 *
 * The field keeps its own text while it is being typed in: a half-finished
 * number is not a number, and rewriting the box under the cursor after every
 * keystroke is what makes such fields impossible to type in. What parses is
 * sent on, what doesn't is left alone until the field is done being edited,
 * and leaving it puts the real value back.
 *
 * The popover and the slider are Radix primitives rather than a positioned
 * `<div>` and an `<input type="range">`. The popover is the reason: it flips
 * and shifts to stay on screen, which a control sitting this close to the
 * window's edge needs, and which hand-rolled dismissal never gave. Escape, a
 * click outside, and handing focus back to the chevron afterwards come with
 * it. The slider follows because its track, fill and thumb are ordinary
 * elements there — the parts a range input only exposes through vendor
 * pseudo-elements, and whose fill cannot be recoloured at all without giving
 * up the native widget.
 */
export function ValueControl({
  label,
  value,
  min,
  max,
  step,
  unit,
  onChange,
}: ValueControlProps) {
  const [open, setOpen] = useState(false);
  // The text in the field while it has focus; null when it is showing `value`.
  const [draft, setDraft] = useState<string | null>(null);

  const shown = quantise(value, step);

  const commit = useCallback(
    (next: number) => onChange(Math.min(max, Math.max(min, quantise(next, step)))),
    [onChange, min, max, step],
  );

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      {/* Styled as the button it reads as, though the parts inside it are what
          you actually hit: the field, and the chevron. */}
      <div
        className={
          "flex flex-col items-center gap-1.5 rounded-md py-2.5 text-xs transition-colors " +
          (open ? "bg-neutral-700" : "hover:bg-neutral-800")
        }
      >
        <div className="flex items-center">
          <input
            type="text"
            inputMode="decimal"
            aria-label={`${label} in ${unit.trim() || "units"}`}
            value={draft ?? String(shown)}
            onChange={(e) => {
              setDraft(e.target.value);
              const parsed = Number(e.target.value);
              // Blank and half-typed entries wait for the field to be left,
              // rather than jumping the engine to zero on the way to 120.
              if (e.target.value.trim() !== "" && Number.isFinite(parsed)) {
                commit(parsed);
              }
            }}
            onFocus={(e) => e.currentTarget.select()}
            onBlur={() => setDraft(null)}
            onKeyDown={(e) => {
              // The arrows nudge, as they would on the slider — this field is
              // the only thing on screen when the slider is shut.
              if (e.key === "ArrowUp" || e.key === "ArrowDown") {
                e.preventDefault();
                const by = step * (e.shiftKey ? 10 : 1) * (e.key === "ArrowUp" ? 1 : -1);
                setDraft(null);
                commit(shown + by);
              } else if (e.key === "Enter" || e.key === "Escape") {
                e.currentTarget.blur();
              }
            }}
            // Sized to the widest value it can hold, so the row beside it does
            // not shuffle sideways as the number goes from 99 to 100. The
            // padding is added on top of that: box-sizing is border-box here,
            // so folding it into the width would eat the digits instead.
            //
            // Only on the left. Padding on the right would push the unit off
            // the number it belongs to, and the focus background would then
            // run underneath it.
            style={{ width: `calc(${String(max).length + 0.5}ch + 0.375rem)` }}
            className="rounded bg-transparent py-0.5 pl-1.5 text-right font-mono tabular-nums text-neutral-200 outline-none focus:bg-neutral-950 focus:text-neutral-100 focus:ring-1 focus:ring-blue-400"
          />
          {/* Spaced only as the unit itself asks to be: "120 bpm", but "80%". */}
          <span className="whitespace-pre font-mono text-neutral-500">{unit}</span>
          <Popover.Trigger
            title={open ? `Hide the ${label} slider` : `Drag ${label}`}
            className="rounded p-0.5 text-neutral-500 transition-colors hover:text-neutral-100"
          >
            <svg
              viewBox="0 0 24 24"
              fill="currentColor"
              className={"h-3 w-3 transition-transform " + (open ? "rotate-180" : "")}
            >
              <path d="M12 15.5 5.5 9l1.4-1.4L12 12.7l5.1-5.1L18.5 9z" />
            </svg>
          </Popover.Trigger>
        </div>
        <div className="tracking-wide text-neutral-400">{label}</div>
      </div>

      <Popover.Portal>
        <Popover.Content
          aria-label={label}
          side="bottom"
          align="start"
          sideOffset={4}
          // Enough to keep it clear of the window's edge, which is where the
          // controls that use this sit.
          collisionPadding={8}
          className="z-20 w-48 rounded-md border border-neutral-800 bg-neutral-900 p-3 shadow-lg shadow-black/40"
        >
          <RadixSlider.Root
            value={[value]}
            min={min}
            max={max}
            step={step}
            onValueChange={([next]) => {
              // The field is showing this value again, whatever was half-typed
              // in it before the slider was reached for.
              setDraft(null);
              commit(next);
            }}
            className="relative flex h-4 w-full touch-none select-none items-center"
          >
            <RadixSlider.Track className="relative h-1.5 w-full grow rounded-full bg-neutral-700">
              <RadixSlider.Range className="absolute h-full rounded-full bg-blue-400" />
            </RadixSlider.Track>
            <RadixSlider.Thumb
              aria-label={label}
              className="block h-3.5 w-3.5 cursor-grab rounded-full border border-neutral-700 bg-neutral-700 outline-none transition-shadow focus-visible:ring-2 focus-visible:ring-neutral-400/50 active:cursor-grabbing"
            />
          </RadixSlider.Root>
          {/* The ends, so the range the slider covers is not a guess. */}
          <div className="mt-1 flex justify-between font-mono text-[10px] text-neutral-500">
            <span>
              {min}
              {unit}
            </span>
            <span>
              {max}
              {unit}
            </span>
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
