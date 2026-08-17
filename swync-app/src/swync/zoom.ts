import type { Extension } from "@codemirror/state";
import { ViewPlugin, type EditorView } from "@codemirror/view";

/**
 * ⌘ and the wheel over the editor changes how large its text is, the way it
 * does in a JetBrains editor.
 *
 * The size belongs to the editor and to nothing else: the panels, the tabs and
 * the tree stay where they are, because what is being zoomed is the thing being
 * read closely, and a whole window that grows takes the rest of the app away
 * with it.
 *
 * The listener is put on the scroller by hand rather than through
 * `EditorView.domEventHandlers`, for two reasons. It has to be non-passive —
 * a wheel handler registered any other way cannot call `preventDefault`, and
 * without that the webview zooms itself on the same gesture, so both would
 * happen at once. And the scroller covers the gutter and the padding beside the
 * last line, which the content element does not: a gesture that did nothing
 * over the line numbers would read as one that had missed.
 */

/** Where the editor starts, in pixels. */
export const DEFAULT_FONT_SIZE = 14;

/** As small as it goes. Below this the ascenders close up and a line of
 *  `[\, `, \, [\, \]]` stops being readable, which is the whole point of it. */
const MIN_FONT_SIZE = 8;

/** As large as it goes. A number rather than no limit at all, so a gesture that
 *  ran away cannot leave two words on the screen. */
const MAX_FONT_SIZE = 40;

/** A size that can actually be shown, from any number at all. Whole pixels: the
 *  steps are whole, and a size that drifted onto a fraction would blur every
 *  glyph on the screen for no gain. */
export function clampFontSize(size: number): number {
  return Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, Math.round(size)));
}

/**
 * How much wheel travel one size costs.
 *
 * Wheels and trackpads disagree wildly about what a gesture is worth, and so do
 * the engines under them: one notch of the same mouse is reported as anything
 * from a handful of pixels to a hundred and twenty, while a trackpad sends a
 * stream of very small ones. So travel is accumulated rather than counted in
 * events, and crossing the threshold spends *one* size and clears what is
 * left — a notch is a size wherever it lands above this, and a trackpad drag
 * is a size for every small step of it.
 */
const TRAVEL_PER_STEP = 20;

/** What a line is worth when a wheel reports in lines rather than pixels.
 *  Firefox does, and the number it means is the size of a line of text. */
const LINE = 16;

class Zoom {
  /** Wheel travel that has not yet bought a step. Signed: the sign is the
   *  direction, and a reversal spends nothing and starts again. */
  private travel = 0;

  constructor(
    private readonly view: EditorView,
    private readonly zoom: (steps: number) => void,
  ) {
    // Not passive: this gesture is ours, and the webview's own zoom would
    // otherwise take it as well.
    this.view.scrollDOM.addEventListener("wheel", this.onWheel, { passive: false });
  }

  destroy() {
    this.view.scrollDOM.removeEventListener("wheel", this.onWheel);
  }

  /** Command on a Mac, Control elsewhere — the pair the rest of the app treats
   *  as the modifier. Control also arrives on its own from a pinch on a
   *  trackpad, which is the same request by another gesture. */
  private readonly onWheel = (event: WheelEvent) => {
    if (!event.metaKey && !event.ctrlKey) return;
    event.preventDefault();

    const travel = this.pixels(event);
    // A reversal is a change of mind, not a continuation: spending what was
    // accumulated in the other direction would make the first size back cost
    // more than the ones after it.
    if (travel * this.travel < 0) this.travel = 0;
    this.travel += travel;
    if (Math.abs(this.travel) < TRAVEL_PER_STEP) return;

    // Cleared rather than drawn down, so a mouse whose engine calls a notch a
    // hundred pixels moves by one size like a mouse whose engine calls it
    // twenty, and neither leaves a debt that steps again on the notch after.
    this.travel = 0;
    // Away from you is up, which is larger — the direction every other zoom on
    // the machine goes, and the opposite of what the wheel's own sign says.
    this.zoom(travel < 0 ? 1 : -1);
  };

  /** A wheel event's travel in pixels, whichever unit it chose to report. */
  private pixels(event: WheelEvent): number {
    switch (event.deltaMode) {
      case WheelEvent.DOM_DELTA_LINE:
        return event.deltaY * LINE;
      case WheelEvent.DOM_DELTA_PAGE:
        return event.deltaY * this.view.scrollDOM.clientHeight;
      default:
        return event.deltaY;
    }
  }
}

/**
 * The gesture, reporting in whole steps.
 *
 * It reports rather than sets: the size is the app's, not the document's — one
 * editor is mounted per tab and a size that lived in here would be forgotten
 * every time tabs were switched. What holds it is `App.tsx`, which also
 * remembers it between launches.
 *
 * @param zoom How many sizes to grow by, negative to shrink. Must be
 * identity-stable like everything else in the extension array.
 */
export function editorZoom(zoom: (steps: number) => void): Extension {
  return ViewPlugin.define((view) => new Zoom(view, zoom));
}
