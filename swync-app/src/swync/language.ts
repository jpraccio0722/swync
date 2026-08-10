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
import { INDENT_ON_INPUT, swyncIndent } from "./indent";
import type { BuiltinIndex, LanguageMetadata } from "./metadata";
import { startState, swyncTokenizer, type TokenizerState } from "./tokenize";

/**
 * The swync language for CodeMirror: the tokenizer in `tokenize.ts`, with the
 * tags its token names resolve to and the colours those tags are drawn in.
 *
 * The tokenizer itself is next door because the tutorial site runs it too —
 * see the note at the top of that file.
 */

/** The rest token: `` ` `` inside a pattern. Its own tag, because it is the
 *  most swync-specific thing on screen and deserves to stand out. */
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

function parser(meta: LanguageMetadata, index: BuiltinIndex): StreamParser<TokenizerState> {
  return {
    name: "swync",

    startState,
    token: swyncTokenizer(meta, index),

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
      swyncBuiltin: builtinTag,
      swyncNote: noteTag,
      swyncDuration: durationTag,
      swyncRest: restTag,
      swyncTrigger: triggerTag,
      fnName: t.function(t.variableName),
    },
  };
}

/**
 * Colours for the dark editor theme. Deliberately gives `rest` a loud, warm
 * colour: it is a single character that changes what a pattern plays.
 */
export const swyncHighlightStyle = HighlightStyle.define([
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
 * The swync language, with completion attached as language data.
 *
 * Attached here rather than through a second `autocompletion()` extension
 * because the editor's `basicSetup` already installs one; adding another would
 * mount two completion tooltips.
 */
export function swyncLanguage(
  meta: LanguageMetadata,
  index: BuiltinIndex,
  autocomplete: CompletionSource,
): LanguageSupport {
  const language = StreamLanguage.define(parser(meta, index));
  return new LanguageSupport(language, [
    language.data.of({ autocomplete }),
    // Attached to the language rather than the extension bundle so it applies
    // where swync does, and stops where a future embedded language would.
    swyncIndent(),
    // The editor's `theme="dark"` ships its own highlight style, and the first
    // highlighter with a rule for a tag wins. Without raising precedence only
    // the tags that theme does not define (`rest`, `builtin`) would be ours,
    // leaving the palette half applied.
    Prec.high(syntaxHighlighting(swyncHighlightStyle)),
  ]);
}
