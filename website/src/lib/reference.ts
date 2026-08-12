import { getCollection, type CollectionEntry } from "astro:content";

/**
 * The language's callable surface.
 *
 * The pages live in `src/content/docs/` as MDX — see `src/content.config.ts`
 * for the shape — and this is the reading of them: the categories in order,
 * the members of one, and the small derivations (a signature, an anchor) that
 * more than one page wants and none of them should own.
 *
 * The prose in those files is hand-written. Their frontmatter is not: it is
 * rewritten from `lang::metadata()` by `scripts/sync-reference.mjs`, out of the
 * same tables the lowerer dispatches on and the editor's docs panel reads. A
 * UGen added to `UGENS` in Rust reaches this site by running `npm run
 * reference`, which seeds its page; a parameter renamed there corrects every
 * signature here without anybody retyping one.
 */

export type Builtin = CollectionEntry<"docs">;
export type Category = CollectionEntry<"docCategories">;
export type ValueKind = Builtin["data"]["receives"];

/**
 * The categories, in the order the docs panel lists them — the order they are
 * worth reading rather than alphabetical, and the same one a reader who knows
 * the app will expect.
 */
export async function categories(): Promise<Category[]> {
	const all = await getCollection("docCategories");
	return all.sort((a, b) => a.data.order - b.data.order);
}

export async function categoryBySlug(slug: string): Promise<Category | undefined> {
	return (await categories()).find((c) => c.id === slug);
}

/** Every name, alphabetical, as the panel sorts them. */
export async function builtins(): Promise<Builtin[]> {
	const all = await getCollection("docs");
	return all.sort((a, b) => a.data.name.localeCompare(b.data.name));
}

/** Which category a page belongs to is which directory it is in — the one fact
 *  about an entry that is its path rather than its frontmatter. */
export function categoryOf(b: Builtin): string {
	return b.id.split("/")[0]!;
}

/** The members of one category, alphabetical. */
export async function membersOf(slug: string): Promise<Builtin[]> {
	return (await builtins()).filter((b) => categoryOf(b) === slug);
}

/** The smallest number of arguments a call can be made with. */
export function requiredArgs(b: Builtin): number {
	return b.data.arities.length === 0 ? 0 : Math.min(...b.data.arities);
}

export function isCallable(b: Builtin): boolean {
	return b.data.arities.length > 0;
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
	if (!isCallable(b)) return b.data.name;

	const required = requiredArgs(b);
	const parts = b.data.params.map((p, i) => (i < required ? p : `${p}?`));
	if (b.data.variadic) parts.push("...");
	return `${b.data.name}(${parts.join(", ")})`;
}

/**
 * Whether the name can be written after a dot — which is what `receives` and
 * `returns` are really for, and worth saying out loud on the page.
 */
export function isChainable(b: Builtin): boolean {
	return b.data.arities.length > 0 && b.data.params.length > 0;
}

/** Where a name's entry lives, for cross-links and for the search results. */
export function hrefFor(b: Builtin): string {
	return `/docs/${categoryOf(b)}/#${b.data.name}`;
}

/**
 * One name as the search island needs it.
 *
 * Rendered on the server and handed over whole: the island is a text box over a
 * list, and shipping the reference's markup to it so it can re-derive a
 * signature would be shipping a second copy of this file to the browser.
 */
export interface SearchEntry {
	name: string;
	params: string[];
	signature: string;
	href: string;
	category: string;
	/** Name, parameters and prose, flattened — see `plainText`. */
	text: string;
}

/**
 * An MDX body as the words in it.
 *
 * A search matches prose, and the prose here is now marked up: backticks,
 * emphasis, links, fenced examples. None of that is worth matching — someone
 * looking for `cutoff` means the word — so the syntax is dropped and the text
 * it wraps is kept. Rough on purpose: a false positive in a search box costs a
 * reader one glance, and a markdown parser to avoid it costs every build.
 */
function plainText(body = ""): string {
	return body
		.replace(/^import\s.+$/gm, "")
		.replace(/```[\s\S]*?```/g, " ")
		.replace(/<[^>]*>/g, " ")
		.replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
		.replace(/[`*_#>]/g, " ")
		.replace(/\s+/g, " ")
		.trim();
}

export async function searchEntries(): Promise<SearchEntry[]> {
	const groups = new Map((await categories()).map((c) => [c.id, c.data.label]));

	return (await builtins()).map((b) => {
		const slug = categoryOf(b);
		return {
			name: b.data.name,
			params: b.data.params,
			signature: signature(b),
			href: hrefFor(b),
			category: groups.get(slug) ?? slug,
			// The same haystack the app's docs panel builds, so a query that works
			// beside the editor works here. `cutoff` should find the filters even
			// though none of them is called that, which is why the parameters are in.
			text: `${b.data.name} ${b.data.params.join(" ")} ${plainText(b.body)}`.toLowerCase(),
		};
	});
}
