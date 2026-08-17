import { StringStream } from "@codemirror/language";
import type { Element, Root, Text } from "hast";
import { visit } from "unist-util-visit";
import {
	startState,
	swyncTokenizer,
} from "../../../swync-app/src/swync/tokenize";
import metadata from "../data/metadata.json";

/**
 * Colour for the tutorial's ```swync fences, at build time.
 *
 * Nothing here knows what swync looks like: the tokenizer is the app's own —
 * `swync-app/src/swync/tokenize.ts`, the one the editor runs, mirroring
 * `lex.rs` — and this walks its output into spans. A reader following the
 * tutorial with the app open therefore sees one set of colours, and a token
 * the editor learns to tell apart is told apart here on the next build with no
 * change to the site. That is the whole reason a highlighter was not written
 * for this page: a second one would be wrong about the language within a
 * release, and quietly.
 *
 * The classes are named in `src/styles/global.css`, which is also where the
 * light half of the palette is decided — the editor is dark-only and so says
 * nothing about it.
 *
 * `@codemirror/language` is a dependency for `StringStream` alone. It is the
 * reader the tokenizer was written against, down to what a `match` against a
 * plain string does, and this file runs in Node during the build rather than in
 * a browser, so none of it reaches anybody's page.
 */

/** Which builtin names resolve — `swync-app`'s `BuiltinIndex`, minus the
 *  bodies, since colouring only ever asks whether a name is one. */
const builtins = new Set(metadata.builtins.map((b) => b.name));

const tokenize = swyncTokenizer(metadata, builtins);

/**
 * A class per token name the tokenizer returns.
 *
 * Brackets and punctuation share one, as they share a colour in the editor:
 * both are the frame a line is read inside rather than anything to look at.
 * A name absent from here is drawn as plain text, which is what makes the
 * tokenizer's `null` — whitespace, and a character it did not recognise —
 * render as itself.
 */
const CLASSES: Record<string, string> = {
	comment: "tok-comment",
	number: "tok-number",
	string: "tok-string",
	keyword: "tok-keyword",
	def: "tok-def",
	variable: "tok-variable",
	fnName: "tok-fn",
	swyncBuiltin: "tok-builtin",
	swyncNote: "tok-note",
	swyncDuration: "tok-duration",
	swyncEnum: "tok-enum",
	swyncRest: "tok-rest",
	swyncTrigger: "tok-trigger",
	swyncQuote: "tok-quote",
	operator: "tok-operator",
	bracket: "tok-punct",
	punctuation: "tok-punct",
};

/**
 * One fenced block, as the spans it is drawn with.
 *
 * The tokenizer reads a line at a time and its state carries across the
 * breaks, exactly as it does in the editor: an octave spelled on one line of a
 * pattern still colours the bare pitches under it.
 */
function spans(code: string): Array<Element | Text> {
	// Neither is read by this tokenizer — no branch of it asks for a column —
	// but `StringStream` takes them, and these are the editor's values.
	const TAB_SIZE = 4;
	const INDENT_UNIT = 2;

	const state = startState();
	const out: Array<Element | Text> = [];

	code.split("\n").forEach((line, index) => {
		if (index) out.push({ type: "text", value: "\n" });

		const stream = new StringStream(line, TAB_SIZE, INDENT_UNIT);
		while (!stream.eol()) {
			const start = stream.pos;
			const name = tokenize(stream, state);
			// The tokenizer always consumes something. Making sure of it here is
			// what keeps a future mistake in it from hanging the build instead of
			// mis-colouring a word.
			if (stream.pos === start) stream.next();

			const value = line.slice(start, stream.pos);
			const className = name ? CLASSES[name] : undefined;
			out.push(
				className
					? {
							type: "element",
							tagName: "span",
							properties: { className: [className] },
							children: [{ type: "text", value }],
						}
					: { type: "text", value },
			);
		}
	});

	return out;
}

/** The text of a `<code>`, which markdown gives as a single text child. */
function source(node: Element): string {
	return node.children
		.map((child) => (child.type === "text" ? child.value : ""))
		.join("");
}

/**
 * The rehype plugin, applied in `astro.config.mjs`.
 *
 * Only fenced blocks are touched: a language class is the one thing a fence
 * has and a backtick in the prose has not, so `` `sin` `` mid-sentence is left
 * as the plain code span it should be.
 */
export function rehypeSwync() {
	return (tree: Root) => {
		visit(tree, "element", (node: Element) => {
			if (node.tagName !== "code") return;

			const classes = node.properties?.className;
			const named = Array.isArray(classes)
				? classes.includes("language-swync")
				: false;
			if (!named) return;

			node.children = spans(source(node));
		});
	};
}
