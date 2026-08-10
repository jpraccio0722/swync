import { StateEffect, StateField, type Extension } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  showTooltip,
  type DecorationSet,
  type Tooltip,
} from "@codemirror/view";
import { signature, type Builtin, type BuiltinIndex } from "./metadata";

/**
 * What the modifier key does to a builtin's name: hold it and the name becomes
 * a link. Pointing at one shows the help the completion list shows — its
 * signature and its one-line doc — and clicking one opens the reference in the
 * side panel, at that name.
 *
 * The docs are written once, in the Rust tables, and until now they were only
 * legible while a name was being typed. A name already in the file is the one
 * whose argument order you are most likely to have forgotten.
 *
 * Hand-rolled rather than `hoverTooltip` from `@codemirror/view`, for the same
 * reason `signature.ts` is: that one recomputes on mouse movement alone, so
 * pressing the modifier over a name already under a resting pointer — which is
 * how this gets used — would show nothing until the mouse was jogged.
 */

/** Command on a Mac, Control elsewhere; the pair `App.tsx` already treats as
 *  the modifier for its own shortcuts. */
function modHeld(event: MouseEvent | KeyboardEvent): boolean {
  return event.metaKey || event.ctrlKey;
}

/** A builtin's name in the document, under the pointer. */
interface Target {
  from: number;
  to: number;
  builtin: Builtin;
}

const setTarget = StateEffect.define<Target | null>();

/**
 * What the modifier and the pointer are pointing at, which is both what the
 * tooltip describes and what the underline is drawn under. One field for both,
 * so the two cannot disagree about which name is the link.
 */
const target = StateField.define<Target | null>({
  create: () => null,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setTarget)) return e.value;
    // An edit moves the text out from under the pointer, so the offsets this
    // holds are no longer the name it was pointing at.
    return tr.docChanged ? null : value;
  },
  provide: (f) => [
    showTooltip.from(f, tooltipFor),
    EditorView.decorations.from(f, linkMark),
  ],
});

const linkDecoration = Decoration.mark({ class: "cm-swync-help-link" });

function linkMark(t: Target | null): DecorationSet {
  return t === null ? Decoration.none : Decoration.set(linkDecoration.range(t.from, t.to));
}

function tooltipFor(t: Target | null): Tooltip | null {
  if (t === null) return null;
  return {
    pos: t.from,
    end: t.to,
    // Below the name, so the tooltip does not cover the line being read — and
    // so it does not land on top of the signature help, which sits above.
    above: false,
    create: () => ({ dom: render(t.builtin) }),
  };
}

/** `lowpass(audio, cutoff, q?)` over its doc, and how to read more. */
function render(b: Builtin): HTMLElement {
  const dom = document.createElement("div");
  dom.className = "cm-swync-help";

  const sig = dom.appendChild(document.createElement("div"));
  sig.className = "cm-swync-help-signature";
  sig.textContent = signature(b);

  if (b.doc) {
    const doc = dom.appendChild(document.createElement("div"));
    doc.className = "cm-swync-help-doc";
    doc.textContent = b.doc;
  }

  // The click is only discoverable from here: the modifier is already down,
  // and this is the moment the name looks like something that can be clicked.
  const hint = dom.appendChild(document.createElement("div"));
  hint.className = "cm-swync-help-hint";
  hint.textContent = "Click to open the reference";

  return dom;
}

/**
 * The document offset under the pointer, or null when the pointer is not over
 * text at all.
 *
 * `posAtCoords` answers with the nearest position rather than nothing, so the
 * blank space to the right of `saw(220)` would otherwise read as the `)` — and
 * the empty half of a short line as whatever ends it.
 */
function offsetAt(view: EditorView, x: number, y: number): number | null {
  const pos = view.posAtCoords({ x, y });
  if (pos === null) return null;

  const rect = view.coordsAtPos(pos);
  // A character's width of slack on each side: the position is a boundary
  // between two characters, so the pointer is never exactly on it.
  const slack = view.defaultCharacterWidth;
  if (
    rect === null ||
    y < rect.top ||
    y > rect.bottom ||
    x < rect.left - slack ||
    x > rect.right + slack
  ) {
    return null;
  }
  return pos;
}

const WORD = /\w/;

/** The name under `pos`, with the range it occupies, or null. */
function wordAt(view: EditorView, pos: number): { name: string; from: number; to: number } | null {
  const line = view.state.doc.lineAt(pos);
  const text = line.text;
  // `pos` is a boundary between characters. The one after it is what the
  // pointer is over, except at a word's right edge, where it is the one before.
  let i = pos - line.from;
  if (!WORD.test(text[i] ?? "")) i--;
  if (!WORD.test(text[i] ?? "")) return null;

  let from = i;
  while (from > 0 && WORD.test(text[from - 1])) from--;
  let to = i + 1;
  while (to < text.length && WORD.test(text[to])) to++;

  // A name inside a comment is prose; it names nothing that runs.
  const comment = text.indexOf("//");
  if (comment !== -1 && comment < from) return null;

  return { name: text.slice(from, to), from: line.from + from, to: line.from + to };
}

/**
 * Watches the pointer and the modifier, and keeps `target` holding whatever the
 * two of them are currently pointing at.
 *
 * The modifier is read from the keyboard as well as from the mouse because it
 * is usually pressed second: the pointer comes to rest on the name, and only
 * then does the hand reach for Command.
 */
class HelpWatcher {
  /** Where the pointer last was, in client coordinates, or null when it has
   *  left the editor. */
  private at: { x: number; y: number } | null = null;

  constructor(
    private readonly view: EditorView,
    private readonly index: BuiltinIndex,
    private readonly openDocs: (name: string) => void,
  ) {
    window.addEventListener("keydown", this.onKey);
    window.addEventListener("keyup", this.onKey);
  }

  destroy() {
    window.removeEventListener("keydown", this.onKey);
    window.removeEventListener("keyup", this.onKey);
  }

  move(event: MouseEvent) {
    this.at = { x: event.clientX, y: event.clientY };
    this.refresh(modHeld(event));
  }

  leave() {
    this.at = null;
    this.refresh(false);
  }

  /**
   * A modifier-click on a builtin opens the reference at it.
   *
   * Answers true only when it lands on one, which is what leaves the
   * modifier's other meaning intact: a ⌘-click on anything else still drops a
   * second cursor.
   */
  click(event: MouseEvent): boolean {
    if (!modHeld(event)) return false;
    const hit = this.targetAt({ x: event.clientX, y: event.clientY });
    if (hit === null) return false;

    event.preventDefault();
    this.openDocs(hit.builtin.name);
    // The panel now says everything the tooltip did, and stays put while it is
    // read; leaving the tooltip up would only cover the code underneath it.
    this.at = null;
    this.refresh(false);
    return true;
  }

  /** Both keydown and keyup: `metaKey` is true on the press of Command itself
   *  and false on its release, which is exactly the state being asked for. */
  private onKey = (event: KeyboardEvent) => this.refresh(modHeld(event));

  private refresh(held: boolean) {
    const next = held && this.at ? this.targetAt(this.at) : null;
    const current = this.view.state.field(target);

    // Moving within one name must not re-dispatch: the tooltip is rebuilt from
    // scratch each time the field changes, and it would flicker under the
    // pointer that is holding it open.
    if (next === null ? current === null : current?.from === next.from) return;
    this.view.dispatch({ effects: setTarget.of(next) });
  }

  private targetAt(at: { x: number; y: number }): Target | null {
    const pos = offsetAt(this.view, at.x, at.y);
    if (pos === null) return null;

    const word = wordAt(this.view, pos);
    if (word === null) return null;

    // Only the builtins carry a doc. A local `fn` is already written out in
    // the same file, a few lines up.
    const builtin = this.index.get(word.name);
    if (builtin === undefined) return null;

    return { from: word.from, to: word.to, builtin };
  }
}

const helpTheme = EditorView.baseTheme({
  ".cm-swync-help": {
    padding: "4px 8px",
    fontSize: "12px",
    maxWidth: "42em",
  },
  ".cm-swync-help-signature": { fontFamily: "monospace", fontWeight: "bold" },
  ".cm-swync-help-doc": {
    marginTop: "4px",
    fontFamily: "sans-serif",
    opacity: "0.75",
    whiteSpace: "normal",
  },
  ".cm-swync-help-hint": {
    marginTop: "6px",
    fontFamily: "sans-serif",
    fontSize: "10px",
    textTransform: "uppercase",
    letterSpacing: "0.05em",
    opacity: "0.45",
  },
  // Only ever on screen while the modifier is down, so it is free to say — in
  // both of the ways a link says it — that this name can be clicked.
  ".cm-swync-help-link": { textDecoration: "underline", cursor: "pointer" },
});

/** Exposed for tests; not part of the extension's public surface. */
export const __test = { wordAt, modHeld };

/**
 * @param openDocs Called with a builtin's name when one is modifier-clicked.
 * Its identity must hold: it goes into the extension array, which CodeMirror
 * reconfigures from whenever that array changes.
 */
export function builtinHelp(
  index: BuiltinIndex,
  openDocs: (name: string) => void,
): Extension {
  return [
    target,
    ViewPlugin.define((view) => new HelpWatcher(view, index, openDocs), {
      eventHandlers: {
        mousemove(event) {
          this.move(event);
        },
        mouseleave() {
          this.leave();
        },
        mousedown(event) {
          return this.click(event);
        },
      },
    }),
    helpTheme,
  ];
}
