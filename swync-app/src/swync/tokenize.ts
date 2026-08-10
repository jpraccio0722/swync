/**
 * The swync tokenizer, mirroring `src-tauri/src/parser/lex.rs`.
 *
 * A stream tokenizer rather than a Lezer grammar: swync's lexical structure is
 * flat enough that a generated parser would buy nothing but a build step.
 *
 * The one rule worth stating explicitly — **swync's only string is the path in
 * `load("...")`, and it has no escape sequences**. The JavaScript mode this
 * replaces treated the backtick rest token as the start of a template literal,
 * which swallowed the remainder of the file, and would read a backslash trigger
 * as escaping the next character. Both are ordinary one-character tokens here,
 * and a string runs to the next `"` or the end of the line — never past it.
 *
 * **This file imports nothing**, and describes its two inputs and its stream
 * structurally rather than by the types `metadata.ts` and CodeMirror give them.
 * That is what lets the tutorial site run this tokenizer over its code blocks
 * at build time: the editor's colours and the website's are one tokenizer, and
 * a website that has no editor in it pays for none of CodeMirror or Tauri to
 * say so. What is CodeMirror's about highlighting — the tag table, the theme,
 * the language — is in `language.ts`, which wraps what is here.
 */

/** As much of CodeMirror's `StringStream` as the tokenizer reads, written out
 *  so a caller outside the editor knows what it has to hand over. Signatures
 *  are `StringStream`'s exactly, since the editor passes the real thing. */
export interface TokenStream {
  eatSpace(): boolean;
  match(
    pattern: string | RegExp,
    consume?: boolean,
    caseInsensitive?: boolean,
  ): boolean | RegExpMatchArray | null;
  next(): string | void;
  skipToEnd(): void;
}

/** The metadata the tokenizer reads: the keyword list, and nothing else.
 *  `LanguageMetadata` satisfies it. */
export interface TokenizerMetadata {
  keywords: readonly string[];
}

/** Membership in the backend's builtin table. `BuiltinIndex` satisfies it, and
 *  so does a bare `Set` of the names — which is all the website has. */
export interface TokenizerIndex {
  has(name: string): boolean;
}

/** Matches `lex.rs`'s number regex, anchored for `StringStream.match`. */
const NUMBER = /^(\d+(\.\d+)?|\.\d+)([eE][+-]?\d+)?/;
/**
 * Mirrors `lex.rs`'s string regex: double quotes, no escapes, and no newline
 * inside — so an unclosed quote colours to the end of its line rather than
 * swallowing the rest of the file.
 */
const STRING = /^"[^"\n]*"?/;
const IDENT = /^[a-zA-Z_][a-zA-Z0-9_]*/;
/**
 * Mirrors `lang::note` with its octave spelled. A binding may still shadow a
 * note, which the highlighter cannot know, so this is a spelling test rather
 * than a resolution.
 */
const NOTE = /^[a-g][sf]?[0-9]$/;
/**
 * Mirrors `lang::note` without one: a pitch class, which is a note only where a
 * sequence has already given an octave — see `octaves` below for how that is
 * tracked. Checked *after* {@link DURATION}, since `e` is the eighth note and
 * the lowerer refuses it as a pitch rather than choosing.
 */
const PITCH_CLASS = /^[a-g][sf]?$/;
/**
 * Mirrors `lang::DURATIONS` and `lang::TUPLET`. Whole-string and single-letter.
 * A binding shadows one exactly as it shadows a note, which the highlighter
 * cannot know, so this is a spelling test rather than a resolution.
 */
const DURATION = /^[whqest]$/;
/**
 * Used until the backend's keyword list arrives, so a file is highlighted
 * correctly on the very first frame rather than rendering every keyword as a
 * plain variable. The backend list wins once loaded.
 */
const FALLBACK_KEYWORDS = ["fn", "let", "if", "else", "for", "in", "use", "as"];
/** Longest first, so `..=` beats `.`, `>>` and `>=` beat `>`, `==` beats `=`. */
const OPERATORS = [
  "..=",
  ">>",
  "==",
  "!=",
  "<=",
  ">=",
  "+",
  "-",
  "*",
  "/",
  "%",
  "=",
  "<",
  ">",
];

export interface TokenizerState {
  /** The previous token was `fn`, so the next identifier is a definition. */
  afterFn: boolean;
  /**
   * One entry per open `[`: whether a note in that list has spelled an octave
   * yet, which is what makes a following bare letter a pitch rather than a
   * name. Pushed on `[` and popped on `]`, so a group inherits nothing and a
   * `let a = 3` two lines down is not coloured by a list that has closed.
   *
   * A stack rather than a boolean because the lowerer restores the register at
   * a closing bracket and the colours should go back with it, and each level
   * opens on whatever the one outside it had — the same inheritance
   * `Expr::List` does, so a group's notes colour like the line they sit in.
   *
   * CodeMirror's default `copyState` slices arrays, so the per-line copies do
   * not share this one.
   */
  octaves: boolean[];
}

export function startState(): TokenizerState {
  return { afterFn: false, octaves: [] };
}

/**
 * The token function, bound to one metadata snapshot.
 *
 * Names are CodeMirror's — `comment`, `number`, `keyword` and the rest resolve
 * through `StreamLanguage`'s built-in tag table, and the `swync`-prefixed ones
 * through the `tokenTable` in `language.ts`. See the note there for why the
 * swync-specific names must carry that prefix.
 */
export function swyncTokenizer(
  meta: TokenizerMetadata,
  index: TokenizerIndex,
): (stream: TokenStream, state: TokenizerState) => string | null {
  const keywords = new Set(meta.keywords.length ? meta.keywords : FALLBACK_KEYWORDS);

  return (stream, state) => {
    if (stream.eatSpace()) return null;

    // Comments run to end of line; there is no block comment form.
    if (stream.match("//")) {
      stream.skipToEnd();
      return "comment";
    }

    // Before operators: a number may begin with `.` (`.5`).
    if (stream.match(NUMBER)) {
      state.afterFn = false;
      return "number";
    }

    // The path in a `load`. Matched before the trigger branch so the
    // backslash in a Windows-style path is part of the string rather than a
    // pattern step.
    if (stream.match(STRING)) {
      state.afterFn = false;
      return "string";
    }

    // The two pattern literals. Neither can start an identifier, so their
    // position relative to the IDENT branch does not matter — they are kept
    // together because they are read together.
    if (stream.match("`")) {
      state.afterFn = false;
      return "swyncRest";
    }

    if (stream.match("\\")) {
      state.afterFn = false;
      return "swyncTrigger";
    }

    // `match` is typed as `RegExpMatchArray | true | null`; a regex argument
    // always yields the array form.
    const ident = stream.match(IDENT) as RegExpMatchArray | null;
    if (ident) {
      const name = ident[0];

      if (state.afterFn) {
        state.afterFn = false;
        return "def";
      }
      if (keywords.has(name)) {
        state.afterFn = name === "fn";
        return "keyword";
      }
      if (NOTE.test(name)) {
        // A spelled octave opens the register for the letters after it.
        if (state.octaves.length) state.octaves[state.octaves.length - 1] = true;
        return "swyncNote";
      }
      if (DURATION.test(name)) return "swyncDuration";
      // Only a pitch once something has said which octave — before that it is
      // an ordinary name, and colouring it otherwise would turn every
      // parameter called `f` pink.
      if (PITCH_CLASS.test(name) && state.octaves[state.octaves.length - 1]) {
        return "swyncNote";
      }
      if (index.has(name)) return "swyncBuiltin";
      // A name in call position is a user function; anything else is a value.
      return stream.match(/^\s*\(/, false) ? "fnName" : "variable";
    }

    state.afterFn = false;

    // A `[` opens an octave register inheriting the one around it, and `]`
    // closes it — see `TokenizerState.octaves`. Only the list brackets: the
    // others group expressions rather than steps.
    if (stream.match("[")) {
      state.octaves.push(state.octaves[state.octaves.length - 1] ?? false);
      return "bracket";
    }
    if (stream.match("]")) {
      state.octaves.pop();
      return "bracket";
    }
    if (stream.match(/^[{}()]/)) return "bracket";
    // Before the single-character rule, and before the operators: `::*` and
    // `::` are their own tokens in `lex.rs`, and `*` is not a multiplication
    // when it follows one.
    if (stream.match("::*") || stream.match("::")) return "punctuation";
    if (stream.match(/^[,;:]/)) return "punctuation";

    for (const op of OPERATORS) {
      if (stream.match(op)) return "operator";
    }

    // Unknown character: consume it so the tokenizer always advances.
    stream.next();
    return null;
  };
}
