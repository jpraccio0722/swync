import { EditorView, Decoration, type DecorationSet } from "@codemirror/view";
import { StateEffect, StateField, type Extension } from "@codemirror/state";

/**
 * Marking, in the editor, the place the last run went wrong.
 *
 * Carried by an effect into a state field rather than by an extension the app
 * rebuilds: the extension array's identity has to stay put — CodeMirror
 * reconfigures whenever it changes, which would throw away completion state —
 * so the marks have to arrive through a transaction instead.
 */

/** Replace every error mark. The payload is 1-based line numbers, which is
 *  what a diagnostic carries and what the document is addressed by. */
export const setErrorLines = StateEffect.define<number[]>();

const errorLine = Decoration.line({ class: "cm-swyncErrorLine" });

const errorField = StateField.define<DecorationSet>({
  create: () => Decoration.none,

  update(marks, tr) {
    // Mapped through edits rather than dropped: an error stays under the code
    // that caused it while that code is being fixed, and the next run replaces
    // the whole set anyway.
    marks = marks.map(tr.changes);

    for (const effect of tr.effects) {
      if (!effect.is(setErrorLines)) continue;
      marks = Decoration.set(
        effect.value
          // A diagnostic's line comes from the source the backend was given,
          // which may be longer than the document is now.
          .filter((line) => line >= 1 && line <= tr.state.doc.lines)
          .map((line) => errorLine.range(tr.state.doc.line(line).from)),
        // The lines arrive in whatever order the diagnostics did; ranges have
        // to be sorted.
        true,
      );
    }

    return marks;
  },

  provide: (field) => EditorView.decorations.from(field),
});

const errorTheme = EditorView.baseTheme({
  ".cm-swyncErrorLine": {
    // Tinted rather than underlined: the position a parser reports is where it
    // stopped, which is not always where the mistake was typed, and a line is
    // an honest width for that. The left rule is what survives being read at a
    // glance against the selection highlight.
    backgroundColor: "rgba(220, 38, 38, 0.16)",
    boxShadow: "inset 2px 0 0 0 #ef4444",
  },
});

/** The error-marking extension. Stable across renders; drive it with
 *  `showErrorLines` and `revealPosition`. */
export function errorMarks(): Extension {
  return [errorField, errorTheme];
}

/** Put the marks on exactly these 1-based lines, replacing any others. */
export function showErrorLines(view: EditorView, lines: number[]) {
  view.dispatch({ effects: setErrorLines.of(lines) });
}

/**
 * Put the cursor at a 1-based line and column and scroll it into view.
 *
 * Both are clamped: the diagnostic describes the text as it was run, and the
 * document may have been edited since.
 */
export function revealPosition(view: EditorView, line: number, column: number | null) {
  const doc = view.state.doc;
  if (doc.lines === 0) return;

  const target = doc.line(Math.min(Math.max(line, 1), doc.lines));
  const pos = Math.min(target.from + Math.max((column ?? 1) - 1, 0), target.to);

  view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
  view.focus();
}
