import {
  HighlightStyle,
  LanguageSupport,
  StreamLanguage,
  type StreamParser,
  syntaxHighlighting,
} from "@codemirror/language";
import type { CompletionSource } from "@codemirror/autocomplete";
import { Prec } from "@codemirror/state";
import { Tag, tags as t } from "@lezer/highlight";
import { INDENT_ON_INPUT, screeIndent } from "./indent";
import type { BuiltinIndex, LanguageMetadata } from "./metadata";

/**
 * A scree tokenizer for CodeMirror, mirroring `src-tauri/src/parser/lex.rs`.
 *
 * A stream tokenizer rather than a Lezer grammar: scree's lexical structure is
 * flat enough that a generated parser would buy nothing but a build step.
 *
 * The one rule worth stating explicitly — **scree's only string is the path in
 * `load("...")`, and it has no escape sequences**. The JavaScript mode this
 * replaces treated the backtick rest token as the start of a template literal,
 * which swallowed the remainder of the file, and would read a backslash trigger
 * as escaping the next character. Both are ordinary one-character tokens here,
 * and a string runs to the next `"` or the end of the line — never past it.
 */

/** The rest token: `` ` `` inside a pattern. Its own tag, because it is the
 *  most scree-specific thing on screen and deserves to stand out. */
export const restTag = Tag.define();
/** The trigger token: a backslash — a step that sounds but carries no value.
 *  Tagged separately from `rest` so a pattern's rhythm is readable at a
 *  glance: hits and silences should never be the same colour. */
export const triggerTag = Tag.define();
/** A name resolved from the backend's builtin table. */
export const builtinTag = Tag.define();
/** A note name: letter, optional `s`/`f`, octave. Tagged apart from ordinary
 *  variables because pitch is the thing you scan a pattern for. */
export const noteTag = Tag.define();
/** A written note value — `q`, `e`, `h` — and the tuplet marker `t`. Tagged
 *  apart from both notes and variables: rhythm is the other thing you scan a
 *  pattern for, and it is read in a different place from pitch. */
export const durationTag = Tag.define();

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

interface State {
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

function parser(meta: LanguageMetadata, index: BuiltinIndex): StreamParser<State> {
  const keywords = new Set(meta.keywords.length ? meta.keywords : FALLBACK_KEYWORDS);

  return {
    name: "scree",

    startState: () => ({ afterFn: false, octaves: [] }),

    token(stream, state) {
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
        return "screeRest";
      }

      if (stream.match("\\")) {
        state.afterFn = false;
        return "screeTrigger";
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
          return "screeNote";
        }
        if (DURATION.test(name)) return "screeDuration";
        // Only a pitch once something has said which octave — before that it is
        // an ordinary name, and colouring it otherwise would turn every
        // parameter called `f` pink.
        if (PITCH_CLASS.test(name) && state.octaves[state.octaves.length - 1]) {
          return "screeNote";
        }
        if (index.has(name)) return "screeBuiltin";
        // A name in call position is a user function; anything else is a value.
        return stream.match(/^\s*\(/, false) ? "fnName" : "variable";
      }

      state.afterFn = false;

      // A `[` opens an octave register inheriting the one around it, and `]`
      // closes it — see `State.octaves`. Only the list brackets: the others
      // group expressions rather than steps.
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
    },

    languageData: {
      commentTokens: { line: "//" },
      // Read by `indentOnInput` in `basicSetup`, and the only thing that
      // triggers the continuation indent: see `indent.ts` for why it is typing
      // the `.` rather than pressing Enter that decides a line is continued.
      indentOnInput: INDENT_ON_INPUT,
    },

    // Only names that are NOT already resolvable are listed here.
    //
    // StreamLanguage resolves a token name against a built-in table of tag
    // names and CodeMirror 5 legacy aliases *before* consulting this one, and
    // the built-in entry wins. `builtin` and `rest` are why the custom names
    // above are prefixed: plain `builtin` is a legacy alias for
    // `variableName.standard`, so an entry for it here is silently ignored.
    // Everything the tokenizer returns unprefixed — `comment`, `number`,
    // `keyword`, `operator`, `bracket`, `punctuation`, `variable`, `def` —
    // resolves through that built-in table on purpose.
    tokenTable: {
      screeBuiltin: builtinTag,
      screeNote: noteTag,
      screeDuration: durationTag,
      screeRest: restTag,
      screeTrigger: triggerTag,
      fnName: t.function(t.variableName),
    },
  };
}

/**
 * Colours for the dark editor theme. Deliberately gives `rest` a loud, warm
 * colour: it is a single character that changes what a pattern plays.
 */
export const screeHighlightStyle = HighlightStyle.define([
  // `t.comment`, not `t.lineComment`: the tokenizer's `comment` token resolves
  // to the base tag, and fallback runs specific -> general, never the reverse.
  { tag: t.comment, color: "#6b7280", fontStyle: "italic" },
  { tag: t.number, color: "#f0abfc" },
  // A path is data on disk rather than a value in the program, so it is given
  // the calm green strings usually get and left out of the pattern palette.
  { tag: t.string, color: "#86efac" },
  { tag: t.keyword, color: "#c084fc" },
  { tag: builtinTag, color: "#5eead4" },
  // Rose: warm like a pitch, and clear of the fuchsia numbers, the amber
  // `def`, and the orange rest it will sit beside inside a pattern.
  { tag: noteTag, color: "#fda4af" },
  // Amber, and deliberately not the rose pitches take: a bar is read for its
  // rhythm or for its notes, rarely both at once, so the two want separating.
  { tag: durationTag, color: "#fbbf24" },
  { tag: t.function(t.variableName), color: "#93c5fd" },
  { tag: t.definition(t.variableName), color: "#fcd34d" },
  { tag: t.variableName, color: "#e5e7eb" },
  // Warm for silence, cool-bright for a hit: the pair has to be separable at
  // a glance inside a dense pattern like `[\, `, \, [\, \]]`.
  { tag: restTag, color: "#fb923c", fontWeight: "bold" },
  { tag: triggerTag, color: "#4ade80", fontWeight: "bold" },
  { tag: t.operator, color: "#94a3b8" },
  { tag: t.bracket, color: "#94a3b8" },
  { tag: t.punctuation, color: "#94a3b8" },
]);

/**
 * The scree language, with completion attached as language data.
 *
 * Attached here rather than through a second `autocompletion()` extension
 * because the editor's `basicSetup` already installs one; adding another would
 * mount two completion tooltips.
 */
export function screeLanguage(
  meta: LanguageMetadata,
  index: BuiltinIndex,
  autocomplete: CompletionSource,
): LanguageSupport {
  const language = StreamLanguage.define(parser(meta, index));
  return new LanguageSupport(language, [
    language.data.of({ autocomplete }),
    // Attached to the language rather than the extension bundle so it applies
    // where scree does, and stops where a future embedded language would.
    screeIndent(),
    // The editor's `theme="dark"` ships its own highlight style, and the first
    // highlighter with a rule for a tag wins. Without raising precedence only
    // the tags that theme does not define (`rest`, `builtin`) would be ours,
    // leaving the palette half applied.
    Prec.high(syntaxHighlighting(screeHighlightStyle)),
  ]);
}
