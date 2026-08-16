import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";
// `astro:content` re-exports this too, but deprecated: Astro 7 wants it from
// here, and warns on every check otherwise.
import { z } from "astro/zod";

/**
 * The function reference, as files.
 *
 * `src/content/docs/` holds both halves of it, told apart by depth: a file at
 * the top is a category, and a file inside a directory of that name is one of
 * its builtins. So `signals.mdx` names the group and `signals/saw.mdx` is the
 * page for `saw`, and adding a name is adding a file next to its neighbours.
 *
 * The bodies are MDX rather than a string in a table because they are prose —
 * they want paragraphs, a ```swync example, a link to a tutorial chapter — and
 * because a doc string is the thing here most often rewritten, so it should be
 * the thing easiest to open. What a name *is* stays in the frontmatter, and
 * `scripts/sync-reference.mjs` rewrites that from the language's own tables;
 * see `src/lib/reference.ts`.
 */

/** A kind of value, as `lang::ValueKind` in the backend defines it. */
const valueKind = z.enum([
	"nothing",
	"any",
	"signal",
	"number",
	"list",
	"pattern",
	"play",
	"section",
	"buffer",
	"duration",
	"text",
	"rate",
	"lane",
]);

const docs = defineCollection({
	loader: glob({ base: "./src/content/docs", pattern: "*/*.mdx" }),
	schema: z.object({
		name: z.string(),
		/** Parameter names, in order. Long enough to cover the largest arity. */
		params: z.array(z.string()).default([]),
		/** Every accepted argument count. Empty means the name is not callable. */
		arities: z.array(z.number().int()).default([]),
		/** True when any count above the largest arity is also accepted. */
		variadic: z.boolean().default(false),
		/** What the first parameter accepts — so, what may be written before the dot. */
		receives: valueKind,
		/** What the name answers with, for the next dot in a chain. */
		returns: valueKind,
	}),
});

const docCategories = defineCollection({
	loader: glob({ base: "./src/content/docs", pattern: "*.mdx" }),
	schema: z.object({
		/** The `lang::metadata()` category this directory stands for. The sync
		 *  script files a builtin by it; nothing on the page shows it. */
		key: z.string(),
		label: z.string(),
		/** Where the category falls in the reading order the docs panel uses —
		 *  what is worth reading first rather than alphabetical. */
		order: z.number().int(),
		/** What the category is for, in a sentence: the index's summary, and the
		 *  page's meta description. The body is free to say more. */
		blurb: z.string(),
	}),
});

export const collections = { docs, docCategories };
