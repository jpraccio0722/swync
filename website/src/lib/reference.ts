import metadata from "../data/metadata.json";

/**
 * The language's callable surface.
 *
 * Nothing here is hand-maintained: `src/data/metadata.json` is written by
 * `cargo run --bin dump-metadata` in `swync-app/src-tauri`, out of the same
 * `lang::metadata()` the editor's docs panel reads. A UGen added to `UGENS` in
 * Rust reaches this site by re-running that command — the reference is
 * generated from the tables the lowerer dispatches on, never transcribed from
 * them.
 */

export type BuiltinCategory = "ugen" | "list" | "math" | "random" | "special";

/** A kind of value, as `lang::ValueKind` in the backend defines it. */
export type ValueKind =
	| "nothing"
	| "any"
	| "signal"
	| "number"
	| "list"
	| "pattern"
	| "play"
	| "section"
	| "buffer"
	| "duration"
	| "text"
	| "rate";

export interface Builtin {
	name: string;
	/** Parameter names, in order. Long enough to cover the largest arity. */
	params: string[];
	/** Every accepted argument count. Empty means the name is not callable. */
	arities: number[];
	/** True when any count above the largest arity is also accepted. */
	variadic: boolean;
	category: BuiltinCategory;
	/** What the first parameter accepts — so, what may be written before the dot. */
	receives: ValueKind;
	/** What the name answers with, for the next dot in a chain. */
	returns: ValueKind;
	doc: string;
}

export const builtins = metadata.builtins as Builtin[];

/**
 * The categories, in the order the docs panel lists them — the order they are
 * worth reading rather than alphabetical, and the same one a reader who knows
 * the app will expect.
 *
 * `slug` is what the URL says; the panel needs no such thing, so this is the
 * one piece of presentation the table cannot supply.
 */
export interface Category {
	key: BuiltinCategory;
	slug: string;
	label: string;
	/** What the category is for, in a sentence. The panel has room for a
	 *  heading and nothing else; a page can say why the group exists. */
	blurb: string;
}

export const CATEGORIES: Category[] = [
	{
		key: "special",
		slug: "patterns-and-playback",
		label: "Patterns and Playback",
		blurb:
			"Starting and stopping sound, and the pattern combinators that arrange it in time.",
	},
	{
		key: "list",
		slug: "lists",
		label: "Lists",
		blurb: "Building and reshaping lists — of notes, of steps, of anything else.",
	},
	{
		key: "math",
		slug: "math",
		label: "Math",
		blurb: "Arithmetic and conversion, over numbers and over signals alike.",
	},
	{
		key: "random",
		slug: "random",
		label: "Random",
		blurb: "Chance: drawn once as the program is lowered, or running as a signal.",
	},
	{
		key: "ugen",
		slug: "signals",
		label: "Signals",
		blurb:
			"The unit generators — oscillators, filters, envelopes and effects — that lower to nodes in the audio graph.",
	},
];

export function categoryBySlug(slug: string): Category | undefined {
	return CATEGORIES.find((c) => c.slug === slug);
}

/** The members of one category, alphabetical, as the panel sorts them. */
export function membersOf(key: BuiltinCategory): Builtin[] {
	return builtins
		.filter((b) => b.category === key)
		.sort((a, b) => a.name.localeCompare(b.name));
}

/** The smallest number of arguments a call can be made with. */
export function requiredArgs(b: Builtin): number {
	return b.arities.length === 0 ? 0 : Math.min(...b.arities);
}

export function isCallable(b: Builtin): boolean {
	return b.arities.length > 0;
}

/**
 * The display signature: `lowpass(audio, cutoff, q)`.
 *
 * Parameters past the smallest arity are optional and marked `?`; a variadic
 * builtin gets a trailing `...`. `dur`, `qvs` and `qvh` take no arguments at
 * all and are rendered bare, since they are bindings rather than functions.
 *
 * Ported from the app's `metadata.ts` rather than reinvented, so a signature
 * reads the same in the editor and on the page.
 */
export function signature(b: Builtin): string {
	if (!isCallable(b)) return b.name;

	const required = requiredArgs(b);
	const parts = b.params.map((p, i) => (i < required ? p : `${p}?`));
	if (b.variadic) parts.push("...");
	return `${b.name}(${parts.join(", ")})`;
}

/**
 * Whether the name can be written after a dot — which is what `receives` and
 * `returns` are really for, and worth saying out loud on the page.
 */
export function isChainable(b: Builtin): boolean {
	return b.arities.length > 0 && b.params.length > 0;
}

/** Where a name's entry lives, for cross-links and for the search results. */
export function hrefFor(b: Builtin): string {
	const category = CATEGORIES.find((c) => c.key === b.category);
	return `/docs/${category?.slug ?? b.category}/#${b.name}`;
}
