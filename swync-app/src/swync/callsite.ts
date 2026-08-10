import type { EditorState } from "@codemirror/state";

/**
 * Which call the cursor is inside, and which of its arguments.
 *
 * Two features ask this and get the same answer: the signature tooltip, which
 * points at the parameter being typed, and completion, which offers different
 * names in different argument slots. It works on raw text rather than the
 * syntax tree because a half-typed `play(pat, ` has no tree node to hang off.
 */

export interface CallSite {
  name: string;
  /** Zero-based index of the argument the cursor is in. */
  argIndex: number;
  /** Where the call's `(` is, so a caller can read its earlier arguments. */
  open: number;
}

/** Blank out `//` comments, preserving offsets so positions stay valid. */
export function stripComments(text: string): string {
  return text.replace(/\/\/[^\n]*/g, (m) => " ".repeat(m.length));
}

/**
 * How far back to look for the opening parenthesis. Runs on every cursor move,
 * so it reads a window rather than the whole document; no real call site is
 * anywhere near this long.
 */
const WINDOW = 4000;

/** Text before the cursor, comment-stripped, with the cursor's offset in it.
 *  Starts at a line boundary so a truncated `//` cannot be misread. */
export function lookback(
  state: EditorState,
  pos: number,
): { text: string; pos: number } {
  const from = pos > WINDOW ? state.doc.lineAt(pos - WINDOW).from : 0;
  return { text: stripComments(state.doc.sliceString(from, pos)), pos: pos - from };
}

/**
 * Walk backwards from `pos` to the innermost unclosed `(`, counting the commas
 * that separate it from the cursor.
 *
 * Returns null when the cursor is not inside a call — including when it is
 * inside a list literal, since a `[` reached at depth zero means the nearest
 * enclosing bracket is not a call's parenthesis.
 */
export function callAt(text: string, pos: number): CallSite | null {
  let depth = 0;
  let commas = 0;

  for (let i = pos - 1; i >= 0; i--) {
    const c = text[i];

    if (c === ")" || c === "]") {
      depth++;
    } else if (c === "[") {
      // At depth zero the nearest enclosing bracket is a list literal, not a
      // call — `[sin(1), 2]` puts the cursor in a list, not in `sin`.
      if (depth === 0) return null;
      depth--;
    } else if (c === "(") {
      if (depth > 0) {
        depth--;
        continue;
      }

      // Read the identifier immediately before the paren.
      let j = i - 1;
      while (j >= 0 && /\s/.test(text[j])) j--;
      const end = j + 1;
      while (j >= 0 && /[a-zA-Z0-9_]/.test(text[j])) j--;
      const name = text.slice(j + 1, end);
      if (!name || /^[0-9]/.test(name)) return null;

      // `lhs >> f(a)` and its tighter-binding twin `lhs.f(a)` both pass lhs as
      // f's first argument, so what the user is typing after the paren is
      // really the second parameter.
      let k = j;
      while (k >= 0 && /\s/.test(text[k])) k--;
      const piped =
        (k >= 1 && text[k - 1] === ">" && text[k] === ">") ||
        (text[k] === "." && text[k - 1] !== ".");

      return { name, argIndex: commas + (piped ? 1 : 0), open: i };
    } else if (c === "," && depth === 0) {
      commas++;
    }
  }

  return null;
}

/**
 * The call's arguments so far, as written — one string per comma-separated
 * slot between `open` and the cursor, the last of them half-typed.
 *
 * Split at depth zero, so a list or a nested call inside one argument stays in
 * one piece.
 */
export function argumentsIn(text: string, open: number, pos: number): string[] {
  const args: string[] = [];
  let depth = 0;
  let start = open + 1;

  for (let i = open + 1; i < pos; i++) {
    const c = text[i];
    if (c === "(" || c === "[") depth++;
    else if (c === ")" || c === "]") depth--;
    else if (c === "," && depth === 0) {
      args.push(text.slice(start, i));
      start = i + 1;
    }
  }
  args.push(text.slice(start, pos));

  return args;
}
