import type {
  Completion,
  CompletionContext,
  CompletionResult,
  CompletionSource,
} from "@codemirror/autocomplete";
import type { EditorView } from "@codemirror/view";
import { argumentsIn, callAt, stripComments } from "./callsite";
import {
  buildIndex,
  isCallable,
  requiredArgs,
  signature,
  type Builtin,
  type BuiltinIndex,
  type Duration,
  type LanguageMetadata,
  type ValueKind,
} from "./metadata";
import type { PortInfo, Ports } from "./ports";
import type { Symbol as ModuleSymbol, Symbols } from "./symbols";

/**
 * Identifier completion for swync: backend builtins, keywords, and whatever the
 * current buffer defines.
 *
 * Document symbols are scraped with regexes rather than parsed. The text being
 * completed is, by definition, half-written — the real parser would reject it,
 * and a round trip to Rust per keystroke would be worse than imprecise.
 */

const FN = /\bfn\s+([a-zA-Z_]\w*)\s*\(([^)]*)\)/g;
/** A `let`, with as much of its value as fits on the line — enough to guess
 *  whether the name holds a list. */
const LET = /\blet\s+([a-zA-Z_]\w*)\s*(?:=\s*([^\n]*))?/g;

/** `enum Scale { major = [0, 2], minor }` — the name, and the braces whole.
 *
 *  A member's value may hold anything but a `}`, which is what the inner class
 *  says: a list, a call, arithmetic. Nesting braces inside one would need the
 *  real parser, and a member whose value is a block is not something anybody
 *  writes — the value of a member is a constant. */
const ENUM = /\benum\s+([a-zA-Z_]\w*)\s*\{([^}]*)\}/g;

/**
 * The member names inside an `enum`'s braces, ignoring their values.
 *
 * Splits only on the commas and line breaks that separate *members*, which
 * means tracking bracket depth: `major = [0, 2, 4]` holds two commas that
 * separate nothing. Elsewhere in this file over-offering a name is the accepted
 * trade, but not here — after `Scale.` this list is presented as the closed set
 * of what may be written, so a `2` from inside a list would be a wrong answer
 * rather than a surplus one.
 */
function enumMembers(body: string): string[] {
  const entries: string[] = [];
  let depth = 0;
  let entry = "";

  for (const ch of body) {
    if (ch === "[" || ch === "(") depth++;
    else if (ch === "]" || ch === ")") depth--;

    if (depth === 0 && (ch === "," || ch === "\n")) {
      entries.push(entry);
      entry = "";
    } else {
      entry += ch;
    }
  }
  entries.push(entry);

  return entries
    .map((e) => e.split("=")[0].trim())
    .filter((name) => /^[a-zA-Z_]\w*$/.test(name));
}
const FOR = /\bfor\s+([a-zA-Z_]\w*)\s+in\b/g;
/**
 * A `use`, split into its path and whatever follows it.
 *
 * What a `use` introduces is readable from the line in every case but the
 * glob, whose names live in a file this side has never read — those appear
 * once the program is run and the name resolves, which is the same moment a
 * mistyped one is caught.
 */
const USE =
  /\buse\s+([a-zA-Z_]\w*(?:::[a-zA-Z_]\w*)*)\s*(?:(::\*)|as\s+([a-zA-Z_]\w*)|::\{([^}]*)\})?/g;
/** Strips `//` comments so definitions inside them are not offered. */
const COMMENT = /\/\/[^\n]*/g;

/** Local symbols rank above builtins; there are fewer and they are yours. The
 *  drawn patterns rank highest: there are fewer still, and the panel is the
 *  only place their names are written down. */
const BOOST = { pattern: 3, local: 2, builtin: 1, keyword: 0 } as const;

// ---------------------------------------------------------------------------
// Argument position.
//
// `play` is the one call whose slots are not interchangeable, and the rules it
// enforces are all discoverable only by running the program: the second
// argument must be a plain `fn` name and nothing else, and everything after it
// must name one of that instrument's own parameters. Both are worth knowing
// while writing rather than after playing.
// ---------------------------------------------------------------------------

/** The calls whose second argument is an instrument. `playn` puts its repeat
 *  count *after* that, so the instrument is in the same slot for all three. */
const PLAYS = new Set(["play", "play_once", "playn"]);

/** Which argument of a `play` names the instrument. */
const INSTRUMENT_ARG = 1;

/** The name that puts gear where an instrument would go. `lang.rs`'s. */
const MIDI_OUT = "midiout";

/** Which argument of a `midiout` names the port. */
const DEVICE_ARG = 0;

/** The two lanes that never reach the instrument, and so are writable whatever
 *  it is. Their meanings are `pattern/patterns.rs`'s. */
const RESERVED_LANES: { name: string; info: string }[] = [
  { name: "legato", info: "Scales how long each note is held, against its step." },
  { name: "pan", info: "Where the note sits between the speakers, -1 to 1." },
];

interface LocalSymbol {
  name: string;
  detail: string;
  type: "function" | "variable";
  /** A `fn`'s parameter names. Absent for anything that is not one. */
  params?: string[];
  /** Which of those were written with a default, and so may be left out. */
  optional?: boolean[];
  /** True for a name a `use` introduced. It may be a function or a module, and
   *  both are writable after a `.`, so the two cases are not worth splitting. */
  imported?: boolean;
  /** A `let`'s value, as written. Resolved on demand to work out what a dot on
   *  this name may reach — `let riff = [60, 63]` makes `riff.` a list. */
  value?: string;
  /** An enum's members. Absent for everything else, and never empty for an
   *  enum, so its presence is what marks one. */
  members?: string[];
}

function scrapeLocals(doc: string): LocalSymbol[] {
  const text = doc.replace(COMMENT, "");
  const found = new Map<string, LocalSymbol>();

  for (const [, name, rawParams] of text.matchAll(FN)) {
    const written = rawParams.split(",").map((p) => p.trim()).filter(Boolean);
    const params = written.map((p) => p.split("=")[0].trim());
    found.set(name, {
      name,
      detail: `${name}(${params.join(", ")})`,
      type: "function",
      params,
      // Which of them a `play` must be given a lane for, and which it may
      // leave to the default written here.
      optional: written.map((p) => p.includes("=")),
    });

    // A function's own parameters are worth offering while writing its body.
    // Scoping them properly would need the real parser; over-offering a name
    // from a sibling function is a smaller cost than missing the one you want.
    for (const p of params) {
      if (!found.has(p)) found.set(p, { name: p, detail: "parameter", type: "variable" });
    }
  }

  for (const [, name, value] of text.matchAll(LET)) {
    if (!found.has(name)) {
      found.set(name, { name, detail: "binding", type: "variable", value });
    }
  }

  // Ahead of the `for` variables and the imports for the same reason `fn` is
  // ahead of `let`: a name can only be one of these, and the lowerer refuses a
  // file where an enum shares a name with anything else outright.
  for (const [, name, body] of text.matchAll(ENUM)) {
    const members = enumMembers(body);
    if (members.length > 0) {
      found.set(name, { name, detail: "enum", type: "variable", members });
    }
  }

  // A loop variable holds one element, so it is not the list being walked.
  for (const [, name] of text.matchAll(FOR)) {
    if (!found.has(name)) {
      found.set(name, { name, detail: "loop variable", type: "variable" });
    }
  }

  for (const name of importedNames(text)) {
    // A module's name completes as a variable: what follows it is `::`, not
    // `(`, so inserting a call would be inserting the wrong thing.
    if (!found.has(name)) {
      found.set(name, { name, detail: "imported", type: "variable", imported: true });
    }
  }

  return [...found.values()];
}

/** Every name the file's `use` lines make writable, in the spelling it is
 *  written in here — which is the alias, wherever there is one. */
function importedNames(text: string): string[] {
  const names: string[] = [];

  for (const [, path, glob, alias, list] of text.matchAll(USE)) {
    if (list !== undefined) {
      for (const entry of list.split(",")) {
        // `kick` or `kick as thump`; the last word is what it is called here.
        const words = entry.trim().split(/\s+as\s+/);
        const name = words[words.length - 1].trim();
        if (/^[a-zA-Z_]\w*$/.test(name)) names.push(name);
      }
      continue;
    }
    if (glob !== undefined) continue;
    // `use a::b` puts `b` in scope, whether it turns out to be a module or a
    // single name; `as` renames it.
    names.push(alias ?? path.split("::").pop()!);
  }

  return names;
}

/**
 * Insert `name()` and put the cursor where the next thing typed should go:
 * between the parens when there are arguments to write, after them when there
 * are not.
 */
function applyCall(name: string, cursorInside: boolean) {
  return (view: EditorView, _completion: Completion, from: number, to: number) => {
    const insert = `${name}()`;
    view.dispatch({
      changes: { from, to, insert },
      selection: { anchor: from + insert.length - (cursorInside ? 1 : 0) },
    });
  };
}

function builtinCompletion(b: Builtin): Completion {
  const option: Completion = {
    label: b.name,
    detail: signature(b),
    info: b.doc,
    type: isCallable(b) ? "function" : "variable",
    boost: BOOST.builtin,
  };
  if (isCallable(b)) {
    option.apply = applyCall(b.name, requiredArgs(b) > 0 || b.variadic);
  }
  return option;
}

// ---------------------------------------------------------------------------
// Method position.
//
// `a.f(b)` is the parser's spelling of `a >> f(b)`, which is the spelling of
// `f(a, b)` — the receiver fills the first parameter. So what may follow a `.`
// is every function of at least one argument, and what should be offered is
// that function minus the parameter already filled: `push(list, value)` reads
// `.push(value)` here, and `rev(list)` needs no parentheses at all.
// ---------------------------------------------------------------------------

/**
 * After a `.` the file's usual local-first order is inverted for the functions
 * that exactly suit the receiver. A list in front of the dot means `push` and
 * `rev` are what is being reached for, whereas a swync file's own `fn`s are
 * instruments — played with `play(pattern, kick)` rather than called on a list.
 */
const METHOD_BOOST = { suited: 3, local: 2, other: 1 } as const;

/** `c4`, `af3`, `gs9` — a note name, which is a number everywhere downstream. */
const NOTE = /^[a-g][sf]?\d$/;

/**
 * Whether a receiver of kind `receiver` may be written to the left of a name
 * declaring `receives`.
 *
 * This mirrors `accepts` in `lang.rs` exactly. That function is the one under
 * test — `every_builtin_receives_what_it_declares` compiles every name in the
 * tables against every kind of receiver — so any disagreement here is this
 * file's bug.
 */
function accepts(receives: ValueKind, receiver: ValueKind): boolean {
  // A receiver the tables cannot pin down rules nothing out.
  if (receiver === "any") return true;
  switch (receives) {
    case "signal":
      // A constant is a node input like any other, so a number reads here.
      return receiver === "signal" || receiver === "number";
    case "number":
      return receiver === "number";
    case "list":
      return receiver === "list";
    case "pattern":
      // A bare value is a one-step pattern: `60.play(inst)` sounds once.
      return receiver === "list" || receiver === "number";
    case "play":
      return receiver === "play";
    case "section":
      return receiver === "section";
    case "buffer":
      return receiver === "buffer";
    case "duration":
      // Only `dot`, and only a written note value can be dotted.
      return receiver === "duration";
    default:
      // "nothing" takes no argument at all; "any", "rate", "lane" and
      // "destination" are only ever results; and "text" is written out at the
      // call, never produced, so nothing can stand to the left of a name that
      // wants one.
      return false;
  }
}

/** The context an expression's kind is resolved against. */
interface Scope {
  index: BuiltinIndex;
  locals: Map<string, LocalSymbol>;
  patterns: Set<string>;
  /** The written note values, so `q.` can offer `dot`. */
  durations: Set<string>;
}

/** The index of the bracket matching the one `text` ends with, or null. */
function matchingOpen(text: string, open: string, close: string): number | null {
  let depth = 0;
  for (let i = text.length - 1; i >= 0; i--) {
    if (text[i] === close) depth++;
    else if (text[i] === open && --depth === 0) return i;
  }
  return null;
}

/**
 * What kind of value an expression produces, read from its last few characters.
 *
 * Like the rest of this file this reads text rather than a tree: the expression
 * is half-written by definition, and the real parser would reject it. So it
 * recognises the shapes that carry their kind on their surface and answers
 * `"any"` — which rules nothing out — to everything else.
 *
 * `depth` bounds the walk through `let` bindings, so a file where two names
 * refer to each other cannot spin.
 */
function kindOf(expr: string, scope: Scope, depth = 4): ValueKind {
  const text = expr.trimEnd();
  if (depth <= 0 || text === "") return "any";

  // `rev([1, 2])` — a call, whose kind is whatever the name answers with.
  if (text.endsWith(")")) {
    const open = matchingOpen(text, "(", ")");
    if (open === null) return "any";
    const name = text.slice(0, open).trimEnd().match(/[a-zA-Z_]\w*$/)?.[0];
    return name === undefined ? "any" : (scope.index.get(name)?.returns ?? "any");
  }

  // `[1, 2, 3]` is a list; `xs[0]` is one element of one, of no known kind.
  // They differ only in whether something indexable sits before the bracket.
  if (text.endsWith("]")) {
    const open = matchingOpen(text, "[", "]");
    if (open === null) return "any";
    return /[a-zA-Z0-9_)\]]$/.test(text.slice(0, open)) ? "any" : "list";
  }

  // `0..=7` is a list of steps, like the literal it stands in for. A range
  // inside brackets or a call was already consumed above, so a `..=` still
  // visible here spans the whole expression.
  if (text.includes("..=")) return "list";

  const word = text.match(/[a-zA-Z_]\w*$/)?.[0];
  if (word === undefined) {
    // `60.m2h`, and `0.5.db`.
    return /\d$/.test(text) ? "number" : "any";
  }

  // A name written after a dot is a call with its arguments omitted, so it is
  // that name's result: in `riff.rev.push(72)`, `rev` is what `push` receives.
  const before = text.slice(0, text.length - word.length);
  if (before.endsWith(".") && !before.endsWith("..")) {
    return scope.index.get(word)?.returns ?? "any";
  }

  // A bare builtin name is that name too — the bound values, `dur` and the
  // beat, are the ones that matter.
  const builtin = scope.index.get(word);
  if (builtin && !scope.locals.has(word)) return builtin.returns;

  if (scope.patterns.has(word)) return "list";

  const local = scope.locals.get(word);
  // A `fn` named rather than called is a section, which is what `seq` takes on
  // its left. Only a no-parameter one: everything that sequences sections
  // inlines them with no arguments, so a `fn` that declares any could not be
  // written there.
  if (local?.type === "function" && (local.params?.length ?? 0) === 0) {
    return "section";
  }
  if (local?.value !== undefined) return kindOf(local.value, scope, depth - 1);
  // A `for` variable holds one element, and a parameter could be anything.
  if (local !== undefined) return "any";

  if (NOTE.test(word)) return "number";
  // A written note value, resolved last for the same reason a note name is:
  // anything bound wins, exactly as the lowerer resolves it.
  if (scope.durations.has(word)) return "duration";
  return "any";
}

/**
 * The members of the enum the text ends in, or null if it does not end in one.
 *
 * The shape recognised here is the shape the lowerer recognises: a bare name,
 * bound to an enum, with the dot straight after it. An indexed or computed
 * receiver is deliberately not it, and neither is a qualified `kit::Scale` —
 * that one arrives through `imported` instead, spelled the way this file writes
 * it, which is why both lists are asked.
 *
 * Scraped names win over imported ones, matching the lowerer: a file's own
 * definitions beat what it brought in.
 */
function enumAt(
  before: string,
  scraped: LocalSymbol[],
  imported: ModuleSymbol[],
): string[] | null {
  const name = before.match(/([a-zA-Z_][\w:]*)$/)?.[1];
  if (name === undefined) return null;

  const local = scraped.find((s) => s.name === name);
  if (local?.members?.length) return local.members;

  const module = imported.find((s) => s.name === name);
  return module?.members.length ? module.members : null;
}

/**
 * The offset of the `.` a name is being written after, or null when the cursor
 * is not in method position.
 *
 * A range's `..=` is the one other place two of these characters meet, and its
 * second dot is followed by `=` rather than by a name — but a range is written
 * one character at a time, so `1..` is a state the completer really sees.
 */
function methodDot(doc: string, from: number): number | null {
  const dot = from - 1;
  if (doc[dot] !== "." || doc[dot - 1] === ".") return null;
  return dot;
}

// ---------------------------------------------------------------------------
// Length position.
//
// A `;` is the one character in the grammar with nothing else it could be —
// the parser uses it for a step's length and for nothing at all besides — so
// what may follow it is a short, closed list. It is also the least memorable
// thing in the language: five single letters whose spelling carries none of
// their meaning. Both of those argue for offering it the moment the `;` lands.
// ---------------------------------------------------------------------------

/**
 * The offset of the `;` a length is being written after, or null when the
 * cursor is somewhere else.
 *
 * `from` is the start of the word being completed, so the walk back only has
 * to clear whitespace — `[c4; q]` is written by people and lexes fine.
 */
function lengthSemi(doc: string, from: number): number | null {
  let i = from - 1;
  while (i >= 0 && /[ \t]/.test(doc[i])) i--;
  return i >= 0 && doc[i] === ";" ? i : null;
}

/**
 * Whether the step this `;` belongs to is a bracketed group.
 *
 * It decides the whole menu: a group takes `;t` and nothing else, and every
 * other step takes a written value and never `t`. Offering both everywhere
 * would put the one refused answer next to the four working ones.
 */
function marksGroup(doc: string, semi: number): boolean {
  let i = semi - 1;
  while (i >= 0 && /\s/.test(doc[i])) i--;
  return i >= 0 && doc[i] === "]";
}

/** Compact enough to read down a menu; the doc says the rest. */
function beatsDetail(beats: number): string {
  if (beats === 0.5) return "½ beat";
  if (beats === 0.25) return "¼ beat";
  if (beats === 1) return "1 beat";
  return `${beats} beats`;
}

/**
 * What may follow a `;`.
 *
 * Ordered as the table is — longest value first — rather than alphabetically
 * or by likelihood, because a scale from a whole note down is the order a
 * musician already holds these in. `boost` descends to hold it against
 * CodeMirror's own ranking.
 */
function lengthOptions(durations: Duration[], group: boolean): Completion[] {
  const offered = durations.filter((d) => d.marksGroup === group);
  return offered.map((d, i) => ({
    label: d.name,
    detail: d.beats === null ? "tuplet" : beatsDetail(d.beats),
    info: d.doc,
    type: d.marksGroup ? "keyword" : "constant",
    boost: offered.length - i,
  }));
}

/**
 * Insert a method: bare when the receiver fills every parameter that must be
 * filled — `xs.rev`, `60.m2h` — and `name()` with the cursor between the
 * parentheses when something is still owed.
 */
function applyMethod(name: string, takesArgs: boolean) {
  return (view: EditorView, _completion: Completion, from: number, to: number) => {
    const insert = takesArgs ? `${name}()` : name;
    view.dispatch({
      changes: { from, to, insert },
      selection: { anchor: from + insert.length - (takesArgs ? 1 : 0) },
    });
  };
}

/** `push(list, value)` as it reads after a dot: `.push(value)`. */
function methodCompletion(b: Builtin, boost: number): Completion {
  // One fewer of each: the receiver has filled the first parameter.
  const rest = b.params.slice(1);
  const required = Math.max(requiredArgs(b) - 1, 0);
  const parts = rest.map((p, i) => (i < required ? p : `${p}?`));
  if (b.variadic) parts.push("...");

  return {
    label: b.name,
    detail: parts.length ? `.${b.name}(${parts.join(", ")})` : `.${b.name}`,
    info: b.doc,
    type: "method",
    boost,
    apply: applyMethod(b.name, required > 0 || b.variadic),
  };
}

/** The same, for a `fn` in the buffer or a name a `use` brought in. */
function localMethodCompletion(s: LocalSymbol): Completion {
  // An imported name may be a module, whose `::` the user writes themselves.
  if (s.params === undefined) {
    return { label: s.name, detail: s.detail, type: "variable", boost: METHOD_BOOST.local };
  }
  const rest = s.params.slice(1);
  return {
    label: s.name,
    detail: rest.length ? `.${s.name}(${rest.join(", ")})` : `.${s.name}`,
    type: "method",
    boost: METHOD_BOOST.local,
    apply: applyMethod(s.name, rest.length > 0),
  };
}

/**
 * Everything writable as `play`'s instrument: a `fn` with somewhere for the
 * pattern to go.
 *
 * Only that. A UGen, a keyword, a drawn pattern and a plain binding are each a
 * compile error in this slot — the lowerer wants a plain function name and
 * refuses anything else before it evaluates a single argument — so offering
 * them would be offering the wrong answer to a question with a short list of
 * right ones.
 */
function instrumentOptions(locals: LocalSymbol[], imported: ModuleSymbol[]): Completion[] {
  // Gear is the other thing that can play a pattern, and unlike an instrument
  // it is not a name anywhere in the document — so nothing else in this menu
  // would ever mention that the slot takes it. Offered first because it is one
  // fixed name against however many instruments a program has, so it is the
  // one that can be found by reading rather than by knowing.
  const options: Completion[] = [
    {
      label: MIDI_OUT,
      detail: "send this pattern out as MIDI",
      info:
        "Play the pattern on gear outside the machine instead of an " +
        "instrument in this program. The pattern's values are MIDI note " +
        "numbers, which is what c4, semi and scale already count in.",
      type: "function",
      boost: BOOST.builtin,
      apply: applyCall(MIDI_OUT, true),
    },
  ];
  const taken = new Set<string>();

  for (const s of locals) {
    // The pattern fills the first parameter, so a `fn` with none cannot be
    // played at all.
    if (s.type !== "function" || (s.params?.length ?? 0) === 0) continue;
    taken.add(s.name);
    options.push({
      label: s.name,
      detail: s.detail,
      type: "function",
      boost: BOOST.local,
    });
  }

  for (const s of imported) {
    if (!s.callable || s.params.length === 0 || taken.has(s.name)) continue;
    options.push({
      label: s.name,
      detail: `${s.name}(${s.params.join(", ")})`,
      type: "function",
      boost: BOOST.builtin,
    });
  }

  return options;
}

/** The lanes a `midiout` destination takes. There is no instrument for one to
 *  reach, so unlike an instrument's these are a fixed set — `play.rs` refuses
 *  anything outside it. Their meanings are `pattern/patterns.rs`'s. */
const MIDI_LANES: { name: string; info: string }[] = [
  { name: "vel", info: "How hard the note is struck, 0 to 1. Defaults to 100 on the wire." },
  { name: "chan", info: "Which MIDI channel this note goes out on, 1 to 16, overriding the destination's." },
  { name: "legato", info: "Scales how long each note is held, against its step." },
];

/** Whether a `play`'s instrument argument is gear rather than a named `fn`. */
function isDestination(instrument: string): boolean {
  return new RegExp(`^\\s*${MIDI_OUT}\\s*\\(`).test(instrument);
}

/**
 * The lanes an instrument will take: its own parameters after the first, and
 * the two reserved ones that never reach it.
 *
 * The rules are `play.rs`'s. The pattern fills the first parameter, so naming
 * it as a lane is refused; a lane already written cannot be written twice; and
 * `legato`/`pan` are handled before the instrument sees them, so an instrument
 * that declares a parameter of either name cannot be given one.
 */
function laneOptions(
  instrument: string,
  written: string[],
  locals: Map<string, LocalSymbol>,
  imported: Map<string, ModuleSymbol>,
): Completion[] {
  const already = new Set(
    written
      .map((arg) => arg.match(/^\s*([a-zA-Z_]\w*)\s*:/)?.[1])
      .filter((name): name is string => name !== undefined),
  );

  // Gear has no parameters to read lanes off, so its set is fixed rather than
  // derived. Answered before the lookup below, which would find nothing for a
  // `midiout(..)` — it is a call, not a name.
  if (isDestination(instrument)) {
    return MIDI_LANES.filter(({ name }) => !already.has(name)).map(({ name, info }) => ({
      label: name,
      detail: "lane",
      info,
      type: "property",
      boost: BOOST.builtin,
      apply: applyLane(name),
    }));
  }

  const local = locals.get(instrument);
  const symbol = imported.get(instrument);
  const params = symbol?.params ?? local?.params;
  if (!params) return [];
  const optional = symbol?.optional ?? local?.optional;

  const options: Completion[] = params.slice(1).flatMap((param, i) => {
    if (already.has(param)) return [];
    return [
      {
        label: param,
        // A parameter with no default has to be filled — by a lane, since the
        // pattern is spoken for — so it is the likelier next word.
        detail: optional?.[i + 1] === false ? `${instrument} needs this` : "lane",
        type: "property",
        boost: optional?.[i + 1] === false ? BOOST.pattern : BOOST.local,
        apply: applyLane(param),
      },
    ];
  });

  for (const { name, info } of RESERVED_LANES) {
    // Reserved lanes never reach the instrument, so one that declares a
    // parameter of the name cannot be given either.
    if (already.has(name) || params.includes(name)) continue;
    options.push({
      label: name,
      detail: "lane",
      info,
      type: "property",
      boost: BOOST.builtin,
      apply: applyLane(name),
    });
  }

  return options;
}

/**
 * The ports this machine has, offered inside `midiout(`.
 *
 * The answer to "how would anybody know what to type here". A port's name is
 * not in the document, not in the project, and not guessable — it is whatever
 * the vendor and the operating system between them decided to call it, and on
 * this desk that includes `Euphonix MIDI Euphonix Port 1`. Nobody types that
 * from memory.
 *
 * Written with the quotes when the quotes are not there yet, and without them
 * when the cursor is already inside a string — CodeMirror replaces the range
 * it was given, and a name inserted over its own opening quote would leave a
 * dangling one behind it.
 *
 * The number goes in `detail` rather than being offered as a completion of its
 * own. Both spellings work, but they are not equally good advice: a number
 * moves when something else is plugged in, and a name does not. Showing it
 * beside the name is how somebody who wants the short form finds it without
 * the menu recommending it.
 */
function portOptions(ports: PortInfo[], quoted: boolean): Completion[] {
  return ports.map((port) => ({
    label: quoted ? port.name : `"${port.name}"`,
    detail: `port ${port.number}`,
    info:
      "A MIDI output on this machine. Any part of the name will do, and case " +
      "does not matter — `midiout(\"deluge\")` finds `Deluge MIDI 1`.",
    type: "constant",
    boost: BOOST.local,
  }));
}

/** A lane is written `name: value`, and the value is what comes next. */
function applyLane(name: string) {
  return (view: EditorView, _completion: Completion, from: number, to: number) => {
    const insert = `${name}: `;
    view.dispatch({
      changes: { from, to, insert },
      selection: { anchor: from + insert.length },
    });
  };
}

/** Exposed for tests; not part of the extension's public surface. */
export const __test = {
  scrapeLocals,
  methodDot,
  lengthSemi,
  marksGroup,
  lengthOptions,
  kindOf,
  accepts,
  instrumentOptions,
  laneOptions,
  portOptions,
  isDestination,
};

/**
 * @param patternNames The drawn patterns' names, read at completion time
 * rather than captured. The panel changes them constantly, and rebuilding this
 * source per edit would mean reconfiguring the editor per keystroke.
 * @param symbols What the file's `use` lines brought in, read the same way and
 * for the same reason — it is filled by a round trip to the backend that
 * finishes long after this source is built.
 * @param ports The MIDI ports this machine has, by the same rule again — and
 * for a reason of the same shape as `symbols`: the names are not in the
 * document, so nothing this side could scrape them from.
 */
export function swyncCompletions(
  meta: LanguageMetadata,
  patternNames: () => string[],
  symbols: Symbols,
  ports: Ports,
): CompletionSource {
  const builtins = meta.builtins.map(builtinCompletion);
  const keywords: Completion[] = meta.keywords.map((label) => ({
    label,
    type: "keyword",
    boost: BOOST.keyword,
  }));

  /** Callable at all, and with a first parameter for a receiver to fill. */
  const methods = meta.builtins.filter((b) => isCallable(b) && b.params.length > 0);
  const index = buildIndex(meta);
  // The written values, minus the tuplet marker: `t` is not a value and there
  // is nothing to write after it.
  const durationNames = new Set(
    meta.durations.filter((d) => !d.marksGroup).map((d) => d.name),
  );

  return (context: CompletionContext): CompletionResult | null => {
    const doc = context.state.doc.toString();
    const word = context.matchBefore(/[a-zA-Z_]\w*/);
    const at = word?.from ?? context.pos;
    const dot = methodDot(doc, at);
    const from = word?.from ?? context.pos;

    // A length is offered before anything else, and before the no-word guard
    // below: `;` is not a word character, so the moment it is typed there is
    // nothing matched behind the cursor and the general path would decline.
    // Nothing else can follow a `;`, so there is nothing to weigh it against.
    const semi = dot === null ? lengthSemi(doc, at) : null;
    if (semi !== null) {
      return {
        from,
        options: lengthOptions(meta.durations, marksGroup(doc, semi)),
        validFor: /^\w*$/,
      };
    }

    // Look for names the file's `use` lines bring in, if they have changed.
    // Cheap on the common edit, which changes none of them.
    symbols.refresh(doc);
    const imported = symbols.current();

    // A `play`'s slots are not interchangeable, and both of the rules about
    // them are otherwise invisible until the program is run. Checked before
    // anything else, including the dot: a `.` inside `play(pat, ` would be part
    // of an instrument's name, and there is no method position here.
    const stripped = dot === null ? stripComments(doc) : "";
    const call = dot === null ? callAt(stripped, at) : null;
    // Inside `midiout(`, which is where a port's name is written and the one
    // place in the language where the thing being named is not in the document
    // — see `ports.ts`. Checked before `play`, though the order does not
    // decide it: `callAt` walks to the *innermost* unclosed paren, so inside
    // `play(pat, midiout("` the call is already this one.
    if (call && call.name === MIDI_OUT && call.argIndex === DEVICE_ARG) {
      ports.refresh();
      const written = argumentsIn(stripped, call.open, at)[DEVICE_ARG] ?? "";
      // Where the opening quote is, if the string has been started. Measured
      // from the call rather than searched backwards from the cursor, so a
      // quote belonging to something else cannot be mistaken for this one.
      const quote = written.indexOf('"');
      const inside = quote !== -1;
      const options = portOptions(ports.current(), inside);
      if (options.length > 0) {
        return {
          // Replacing from just after the quote rather than from the word:
          // a port name has spaces in it, and the word regex would leave
          // everything before the last one behind.
          from: inside ? call.open + 1 + quote + 1 : from,
          options,
          // Anything but the closing quote, since a name is words and spaces.
          validFor: inside ? /^[^"]*$/ : /^\w*$/,
        };
      }
    }

    if (call && PLAYS.has(call.name)) {
      const scraped = scrapeLocals(doc);

      if (call.argIndex === INSTRUMENT_ARG) {
        // A qualified name is one name, so the `::` in `kit::kick` has to be
        // inside what is being matched — the plain word regex would restart
        // after it and leave the label unmatchable.
        const qualified = context.matchBefore(/[a-zA-Z_][\w:]*/);
        return {
          from: qualified?.from ?? context.pos,
          options: instrumentOptions(scraped, imported),
          validFor: /^[\w:]*$/,
        };
      }

      if (call.argIndex > INSTRUMENT_ARG) {
        const args = argumentsIn(stripped, call.open, at);
        // The instrument is the argument before the lanes, and is a plain
        // name — the lowerer accepts nothing else there.
        const instrument = args[INSTRUMENT_ARG]?.trim() ?? "";
        const options = laneOptions(
          instrument,
          args.slice(INSTRUMENT_ARG + 1),
          new Map(scraped.map((s) => [s.name, s])),
          new Map(imported.map((s) => [s.name, s])),
        );
        // An instrument this side cannot identify has no lanes to offer, and
        // the ordinary list below is a better answer than none.
        if (options.length > 0) return { from, options, validFor: /^\w*$/ };
      }
    }

    // A `.` is worth completing on its own: it is the one character after
    // which the set of writable names is both small and hard to remember.
    if (dot === null) {
      // An explicit invocation on empty space should still list everything.
      if (!word && !context.explicit) return null;
      if (word?.from === word?.to && !context.explicit) return null;
    }

    const patterns: Completion[] = patternNames().map((name) => ({
      label: name,
      detail: "graphical pattern",
      info: "Drawn in the side panel. Bound as a list, so it plays like one: play(name, kick)",
      type: "variable",
      boost: BOOST.pattern,
    }));

    // `call_with` tries the builtin tables before the environment, so a local
    // `fn saw` never runs — the UGen does. Drop the shadowed local rather than
    // offering a name whose completion would describe the wrong thing.
    const taken = new Set([
      ...builtins.map((b) => b.label),
      ...keywords.map((k) => k.label),
      // A `let` of a pattern's name shadows the panel, but it is still one
      // name, and the drawn one already says where to go and change it.
      ...patterns.map((p) => p.label),
    ]);
    const scraped = scrapeLocals(doc);
    const visible = scraped.filter((s) => !taken.has(s.name));

    const options =
      dot === null
        ? [
            ...patterns,
            ...visible.map(
              (s): Completion => ({
                label: s.name,
                detail: s.detail,
                type: s.type,
                boost: BOOST.local,
                apply: s.type === "function" ? applyCall(s.name, true) : undefined,
              }),
            ),
            ...keywords,
            ...builtins,
          ]
        : methodOptions(dot);

    return {
      from: word?.from ?? context.pos,
      options,
      validFor: /^\w*$/,
    };

    /**
     * What may be written after the `.` at `dot`.
     *
     * Keywords, patterns and plain bindings are all absent: none of them is a
     * function, so none can take the receiver. Of the functions, only those
     * whose first parameter accepts what is in front of the dot survive — on a
     * list that drops the 58 UGens and the arithmetic, every one of which would
     * be a compile error rather than merely an unlikely thing to write.
     *
     * A receiver this cannot identify reads as `"any"`, which accepts
     * everything: an unrecognised expression must not hide working names.
     */
    function methodOptions(dot: number): Completion[] {
      const scope: Scope = {
        index,
        locals: new Map(scraped.map((s) => [s.name, s])),
        patterns: new Set(patternNames()),
        durations: durationNames,
      };

      // An enum receiver is answered first and answered alone. What follows
      // `Scale.` is a member and can be nothing else — the lowerer reads it
      // straight off the enum without ever calling anything — so every builtin
      // and every local `fn` is wrong here. Without this the receiver reads as
      // an unidentifiable expression, which is treated as `"any"`, and `"any"`
      // accepts everything: the menu would offer `Scale.reverb`.
      const members = enumAt(doc.slice(0, dot), scraped, imported);
      if (members !== null) {
        return members.map((name, i) => ({
          label: name,
          detail: "member",
          type: "enum",
          // In the order they were written. An enum is a short list somebody
          // chose an order for, and alphabetising it would lose that.
          boost: METHOD_BOOST.suited - i,
        }));
      }

      const receiver = kindOf(doc.slice(0, dot), scope);

      const fromBuiltins = methods
        .filter((b) => accepts(b.receives, receiver))
        .map((b) =>
          methodCompletion(
            b,
            // An exact match ranks above one that merely accepts: a list takes
            // both `push` and `play`, and `push` is the likelier next word.
            b.receives === receiver ? METHOD_BOOST.suited : METHOD_BOOST.other,
          ),
        );
      // A user `fn` has no declared parameter kinds, so it is always offered.
      const fromLocals = visible
        .filter((s) => (s.params?.length ?? 0) > 0 || s.imported)
        .map(localMethodCompletion);

      return [...fromLocals, ...fromBuiltins];
    }
  };
}
