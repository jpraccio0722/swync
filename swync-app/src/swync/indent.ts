import { type IndentContext, indentService } from "@codemirror/language";
import type { Extension } from "@codemirror/state";

/**
 * Indentation for continued lines, mirroring `insert_terminators` in
 * `src-tauri/src/parser/lex.rs`.
 *
 * The lexer decides where a statement ends: a line break becomes a terminator
 * unless the next line opens with a token that carries the statement on — a
 * `.`, a `>>`, an operator. Those are exactly the lines that are *not* new
 * statements, so those are the lines that read better one step in:
 *
 * ```
 * playn(riff, lead, 4)
 *   .then_fill([1, 1, 1, 1])
 *   .then(outro)
 * ```
 *
 * One step, once. Every continued line is measured from the line that *began*
 * the statement rather than from the line above it, so a chain steps in at its
 * first link and stays there — it is one statement written down the page, not
 * a staircase. That is what the README's chains already look like by hand.
 *
 * Nothing here fires on Enter, because at Enter there is no line yet to judge:
 * `some()` followed by a break is a finished statement until a `.` is typed.
 * The `.` is the moment the meaning changes, so `indentOnInput` is what applies
 * this — see `INDENT_ON_INPUT`. Lines this has no opinion about are answered
 * with `undefined` rather than `null`, which leaves CodeMirror's own "carry the
 * previous line's indentation" behaviour in place for the rest of the language.
 */

/**
 * A line that continues the one above it: `cont_next` in `lex.rs`, spelled as
 * a regex against the start of the line.
 *
 * Two exclusions carry the lexer's longest-match rule. `.` followed by a digit
 * is the number `.5`, which begins a statement rather than continuing one, and
 * `//` is a comment rather than a division. `[` is absent for the reason given
 * in `lex.rs`: a line starting with `[` is a new pattern, not an index into the
 * line above.
 */
const CONTINUES =
  /^\s*(\.\.=|\.(?![0-9])|::\*?|>>|[<>!=]=|[<>=]|[-+*%,{:]|\/(?!\/)|else\b)/;

/** A line opening with a bare `.5`-style number — the one thing that starts
 *  with a dot and is *not* a continuation, so it is put back where a statement
 *  belongs rather than left with the indent the dot briefly earned it. */
const LEADING_NUMBER = /^\s*\.[0-9]/;

/**
 * When to re-indent as the line is typed. Matches the whole of what stands
 * before the cursor, so it only fires while the continuation token is the only
 * thing on the line — typing on past it never moves the text again. The one
 * token allowed to grow is the leading number, because `.` and `.5` want
 * opposite answers and the digit is what tells them apart.
 *
 * A deliberately smaller set than `CONTINUES`. `/` is left out because the
 * first character of a `//` comment would match it and pull the comment in;
 * `else` because it is a prefix of names like `elsewhere`, and the indent would
 * land four characters into a word; a lone `:` because `::` reaches here on its
 * second character anyway.
 */
export const INDENT_ON_INPUT =
  /^\s*(\.[0-9]*|\.\.=?|::\*?|>>?|[<>!=]=|[<>=]|[-+*%,{])$/;

/** Blank, or nothing but a comment — invisible to the lexer, so invisible to
 *  the question of what the line above was. */
const IGNORED = /^\s*(\/\/.*)?$/;

/**
 * The line that opened the statement running above `from`: the nearest line
 * carrying tokens, and then back past any continuations of its own.
 *
 * Read through `cx.lineAt` rather than the document so that a pending line
 * break is honoured — when this runs from Enter, the line above is the text
 * before the break, not the whole line the break is splitting.
 */
function statementHead(cx: IndentContext, from: number) {
  let pos = from;
  let head = null;
  while (pos > 0) {
    const line = cx.lineAt(pos - 1, -1);
    pos = line.from;
    if (IGNORED.test(line.text)) continue;
    head = line;
    if (!CONTINUES.test(line.text)) break;
  }
  return head;
}

/** The indent service described above. */
export function swyncIndent(): Extension {
  return indentService.of((cx, pos) => {
    // Bias forward: at a line break this is the text that will land on the new
    // line, which is the line being indented.
    const line = cx.lineAt(pos, 1);
    const continues = CONTINUES.test(line.text);
    if (!continues && !LEADING_NUMBER.test(line.text)) return undefined;

    const head = statementHead(cx, line.from);
    if (!head) return undefined;

    const base = cx.lineIndent(head.from, -1);
    return continues ? base + cx.unit : base;
  });
}
