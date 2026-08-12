import { useEffect, useMemo, useRef, useState } from "react";
import type { SearchEntry } from "../lib/reference";

interface Props {
	/** The whole reference, flattened for matching by `searchEntries` — the
	 *  signature, the anchor and the haystack are all decided during the build,
	 *  so nothing that reads a content collection reaches the browser. */
	entries: SearchEntry[];
}

/**
 * Search across the whole reference.
 *
 * A name matched in its own right ranks above one matched only by its prose:
 * someone typing `saw` wants the oscillator first, not the six entries whose
 * descriptions mention a sawtooth.
 */
function rank(e: SearchEntry, q: string): number {
	const name = e.name.toLowerCase();
	if (name === q) return 0;
	if (name.startsWith(q)) return 1;
	if (name.includes(q)) return 2;
	if (e.params.some((p) => p.toLowerCase().includes(q))) return 3;
	return 4;
}

export default function ReferenceSearch({ entries }: Props) {
	const [query, setQuery] = useState("");
	const [active, setActive] = useState(0);
	const input = useRef<HTMLInputElement>(null);
	const listbox = useRef<HTMLUListElement>(null);

	const results = useMemo(() => {
		const q = query.trim().toLowerCase();
		if (q === "") return [];
		return entries
			.filter((e) => e.text.includes(q))
			.sort((a, b) => rank(a, q) - rank(b, q) || a.name.localeCompare(b.name))
			.slice(0, 40);
	}, [entries, query]);

	// A new query is a new list, and the selection belongs to the list rather
	// than to the box — leaving it where it was points at an unrelated row.
	useEffect(() => setActive(0), [query]);

	// `/` focuses the box from anywhere on the page, unless the reader is
	// already typing somewhere — in which case they meant the character.
	useEffect(() => {
		function onKey(e: KeyboardEvent) {
			if (e.key !== "/" || e.metaKey || e.ctrlKey || e.altKey) return;
			const el = document.activeElement;
			const typing =
				el instanceof HTMLInputElement ||
				el instanceof HTMLTextAreaElement ||
				(el instanceof HTMLElement && el.isContentEditable);
			if (typing) return;
			e.preventDefault();
			input.current?.focus();
		}
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, []);

	// Keep the selected row in view when the arrows walk past the edge of the
	// scrolling list.
	useEffect(() => {
		listbox.current
			?.querySelector('[data-active="true"]')
			?.scrollIntoView({ block: "nearest" });
	}, [active]);

	function go(e: SearchEntry) {
		window.location.href = e.href;
	}

	function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
		if (e.key === "Escape") {
			setQuery("");
			input.current?.blur();
			return;
		}
		if (results.length === 0) return;

		if (e.key === "ArrowDown") {
			e.preventDefault();
			setActive((i) => (i + 1) % results.length);
		} else if (e.key === "ArrowUp") {
			e.preventDefault();
			setActive((i) => (i - 1 + results.length) % results.length);
		} else if (e.key === "Enter") {
			e.preventDefault();
			const chosen = results[active];
			if (chosen) go(chosen);
		}
	}

	const open = query.trim() !== "";

	return (
		<div className="relative">
			<div className="relative">
				<input
					ref={input}
					type="search"
					value={query}
					onChange={(e) => setQuery(e.target.value)}
					onKeyDown={onKeyDown}
					placeholder="Search the reference"
					aria-label="Search the reference"
					aria-expanded={open}
					aria-controls="reference-results"
					role="combobox"
					autoComplete="off"
					className="w-full rounded-lg border border-neutral-300 bg-white py-2 pl-3 pr-10 text-sm text-neutral-900 placeholder:text-neutral-500 focus:border-blue-600 focus:outline-none focus:ring-1 focus:ring-blue-600 dark:border-neutral-800 dark:bg-neutral-950 dark:text-neutral-100 dark:placeholder:text-neutral-600 dark:focus:border-blue-700 dark:focus:ring-blue-700"
				/>
				{!open && (
					<kbd
						aria-hidden="true"
						className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 rounded border border-neutral-300 px-1.5 py-0.5 font-mono text-[10px] text-neutral-500 dark:border-neutral-700 dark:text-neutral-500"
					>
						/
					</kbd>
				)}
			</div>

			{open && (
				<div className="absolute z-20 mt-1 w-full overflow-hidden rounded-lg border border-neutral-300 bg-white shadow-lg dark:border-neutral-800 dark:bg-neutral-900">
					{results.length === 0 ? (
						<p className="px-3 py-2.5 text-sm text-neutral-500">
							Nothing matches{" "}
							<span className="font-mono text-neutral-700 dark:text-neutral-300">
								{query}
							</span>
							.
						</p>
					) : (
						<ul
							ref={listbox}
							id="reference-results"
							role="listbox"
							className="max-h-80 overflow-y-auto py-1"
						>
							{results.map((e, i) => (
								<li key={e.name} role="option" aria-selected={i === active}>
									<a
										href={e.href}
										data-active={i === active}
										onMouseEnter={() => setActive(i)}
										className={
											"flex items-baseline gap-3 px-3 py-1.5 " +
											(i === active ? "bg-neutral-100 dark:bg-neutral-800" : "")
										}
									>
										<span className="font-mono text-sm text-blue-700 dark:text-blue-400">
											{e.signature}
										</span>
										<span className="ml-auto shrink-0 text-[10px] uppercase tracking-wide text-neutral-500">
											{e.category}
										</span>
									</a>
								</li>
							))}
						</ul>
					)}
				</div>
			)}
		</div>
	);
}
