/**
 * Reconciles `src/content/docs/` against the language's own tables.
 *
 * The prose belongs to whoever writes it — every MDX body here is left exactly
 * as it was found. What a name *is*, though, is not prose: its parameters, its
 * arities, what it receives and returns and which category it sits in are all
 * decided in `swync-app/src-tauri/src/lang.rs`, and this rewrites the
 * frontmatter from `src/data/metadata.json` so a signature on the page cannot
 * quietly disagree with the one the lowerer accepts.
 *
 * So: adding a UGen in Rust and re-running `npm run reference` seeds a stub
 * page for it, and changing an existing one's parameters corrects the page
 * without touching a word of what was written about it.
 *
 *     npm run reference     # dumps metadata.json from cargo, then runs this
 *     node scripts/sync-reference.mjs
 *
 * Names that have left the tables are reported rather than deleted: prose is
 * expensive and a rename should not silently throw it away.
 */

import { readFileSync, writeFileSync, mkdirSync, readdirSync, renameSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const METADATA = join(ROOT, "src/data/metadata.json");
const DOCS = join(ROOT, "src/content/docs");

/** Frontmatter, and everything after it. A file with no frontmatter is all
 *  body, which is what lets a hand-written stub be adopted on the next run. */
function split(source) {
	const match = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/.exec(source);
	if (!match) return { body: source };
	return { front: match[1], body: source.slice(match[0].length) };
}

/** The one field this script reads back out of a category page. A whole YAML
 *  parser to find `key:` would be a dependency for a line. */
function keyOf(front = "") {
	return /^key:\s*(\S+)\s*$/m.exec(front)?.[1];
}

/**
 * A YAML scalar.
 *
 * Quoted unless it is plainly safe: unquoted `y`, `no` and `on` are booleans
 * to a YAML reader, and there is no promise a builtin will never be called one
 * of them.
 */
function scalar(value) {
	if (typeof value === "boolean" || typeof value === "number") return String(value);
	return /^[A-Za-z_][A-Za-z0-9_]*$/.test(value) && !/^(y|n|yes|no|on|off|true|false|null)$/i.test(value)
		? value
		: JSON.stringify(value);
}

function frontmatter(builtin) {
	const lines = [
		`name: ${scalar(builtin.name)}`,
		`params: [${builtin.params.map(scalar).join(", ")}]`,
		`arities: [${builtin.arities.join(", ")}]`,
		`variadic: ${builtin.variadic}`,
		`receives: ${scalar(builtin.receives)}`,
		`returns: ${scalar(builtin.returns)}`,
	];
	return `---\n${lines.join("\n")}\n---\n`;
}

/**
 * A doc string as an MDX body, for a name that has no page yet.
 *
 * The tables write code in backticks already, which is markdown's own spelling
 * for it, so the text carries over as-is. `<` and `{` do not: MDX reads them as
 * the start of an element and of an expression, and a doc string means neither.
 */
function seed(doc) {
	const escaped = doc
		.split(/(`[^`]*`)/)
		.map((part, i) => (i % 2 === 1 ? part : part.replace(/([<{])/g, "\\$1")))
		.join("");
	return `\n${escaped}\n`;
}

const metadata = JSON.parse(readFileSync(METADATA, "utf8"));

// The category pages are the map from a Rust category to a directory: the
// slugs are the site's business, so they are declared there rather than here.
const categories = new Map();
for (const file of readdirSync(DOCS, { withFileTypes: true })) {
	if (!file.isFile() || !file.name.endsWith(".mdx")) continue;
	const slug = file.name.slice(0, -".mdx".length);
	const key = keyOf(split(readFileSync(join(DOCS, file.name), "utf8")).front);
	if (key) categories.set(key, slug);
}

/** Every page that exists now, by name — so a builtin that has changed
 *  category is moved rather than duplicated. */
const existing = new Map();
for (const dir of readdirSync(DOCS, { withFileTypes: true })) {
	if (!dir.isDirectory()) continue;
	for (const file of readdirSync(join(DOCS, dir.name))) {
		if (file.endsWith(".mdx")) {
			existing.set(file.slice(0, -".mdx".length), join(DOCS, dir.name, file));
		}
	}
}

const counts = { created: 0, updated: 0, moved: 0, unchanged: 0 };
const problems = [];

for (const builtin of metadata.builtins) {
	const slug = categories.get(builtin.category);
	if (!slug) {
		problems.push(`${builtin.name}: no page for category "${builtin.category}" — add src/content/docs/<slug>.mdx with \`key: ${builtin.category}\``);
		continue;
	}

	const dir = join(DOCS, slug);
	const path = join(dir, `${builtin.name}.mdx`);
	const was = existing.get(builtin.name);
	existing.delete(builtin.name);

	mkdirSync(dir, { recursive: true });

	if (was && was !== path) {
		renameSync(was, path);
		counts.moved += 1;
	}

	const before = was ? readFileSync(path, "utf8") : undefined;
	const body = before === undefined ? seed(builtin.doc) : split(before).body;
	const after = frontmatter(builtin) + body;

	if (before === after) {
		counts.unchanged += 1;
		continue;
	}
	writeFileSync(path, after);
	if (before === undefined) counts.created += 1;
	else if (was === path) counts.updated += 1;
}

for (const [name, path] of existing) {
	problems.push(`${name}: no longer in the language tables — ${path.slice(ROOT.length + 1)} is now orphaned`);
}

console.log(
	`reference: ${counts.created} created, ${counts.updated} updated, ${counts.moved} moved, ${counts.unchanged} unchanged`,
);
for (const problem of problems) console.warn(`  ! ${problem}`);
if (problems.length > 0) process.exitCode = 1;
