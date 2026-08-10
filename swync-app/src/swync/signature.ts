import { StateField, type EditorState, type Extension } from "@codemirror/state";
import { EditorView, showTooltip, type Tooltip } from "@codemirror/view";
import { isCallable, requiredArgs, type Builtin, type BuiltinIndex } from "./metadata";
import { callAt, lookback, stripComments } from "./callsite";

/**
 * Parameter hints: while the cursor sits inside a call, show that call's
 * signature with the argument being typed picked out.
 *
 * CodeMirror ships completion and linting but no signature help, so this is a
 * small hand-rolled one. Finding the call the cursor is in is [callsite]'s
 * job, since completion asks the same question.
 */

/** `lowpass(audio, `**`cutoff`**`, q)` as DOM, with the active param marked. */
function render(b: Builtin, argIndex: number): HTMLElement {
  const dom = document.createElement("div");
  dom.className = "cm-swync-signature";

  const name = dom.appendChild(document.createElement("span"));
  name.className = "cm-swync-signature-name";
  name.textContent = b.name;

  dom.appendChild(document.createTextNode("("));

  const required = requiredArgs(b);
  b.params.forEach((param, i) => {
    if (i > 0) dom.appendChild(document.createTextNode(", "));
    const span = dom.appendChild(document.createElement("span"));
    span.textContent = i < required ? param : `${param}?`;
    if (i === argIndex) span.className = "cm-swync-signature-active";
  });

  if (b.variadic) {
    dom.appendChild(document.createTextNode(b.params.length ? ", ..." : "..."));
  }

  dom.appendChild(document.createTextNode(")"));

  if (b.doc) {
    const doc = dom.appendChild(document.createElement("div"));
    doc.className = "cm-swync-signature-doc";
    doc.textContent = b.doc;
  }

  return dom;
}

function tooltips(state: EditorState, index: BuiltinIndex): readonly Tooltip[] {
  const { main } = state.selection;
  if (!main.empty) return [];

  const back = lookback(state, main.head);
  const call = callAt(back.text, back.pos);
  if (!call) return [];

  const builtin = index.get(call.name);
  if (!builtin || !isCallable(builtin)) return [];

  // Past the last parameter there is nothing left to point at — the call is
  // over-applied, which the lowerer will report on its own.
  if (call.argIndex >= builtin.params.length && !builtin.variadic) return [];

  return [
    {
      pos: main.head,
      above: true,
      create: () => ({ dom: render(builtin, call.argIndex) }),
    },
  ];
}

const signatureTheme = EditorView.baseTheme({
  ".cm-swync-signature": {
    padding: "4px 8px",
    fontFamily: "monospace",
    fontSize: "12px",
    maxWidth: "42em",
    borderRadius: "4px",
  },
  ".cm-swync-signature-name": { fontWeight: "bold" },
  ".cm-swync-signature-active": {
    fontWeight: "bold",
    textDecoration: "underline",
  },
  ".cm-swync-signature-doc": {
    marginTop: "4px",
    fontFamily: "sans-serif",
    opacity: "0.75",
    whiteSpace: "normal",
  },
});

/** Exposed for tests; not part of the extension's public surface. */
export const __test = { callAt, stripComments };

export function signatureHelp(index: BuiltinIndex): Extension {
  const field = StateField.define<readonly Tooltip[]>({
    create: (state) => tooltips(state, index),
    update(value, tr) {
      if (!tr.docChanged && !tr.selection) return value;
      return tooltips(tr.state, index);
    },
    provide: (f) => showTooltip.computeN([f], (state) => state.field(f)),
  });

  return [field, signatureTheme];
}
