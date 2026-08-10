import { useEffect, useMemo, useRef, useState } from "react";
import { signature, type Builtin, type BuiltinCategory } from "./swync/metadata";

/** The name a ⌘-click asked for, with a count so asking twice scrolls twice —
 *  the second click on a name already on screen must still move to it. */
export interface DocsFocus {
  name: string;
  nonce: number;
}

interface DocsPanelProps {
  builtins: Builtin[];
  focus: DocsFocus | null;
}

/**
 * A heading for every category, in the order they are worth reading. The
 * reference splits the UGens further than the one `Signals` table does, which
 * is a distinction only prose can make.
 *
 * A record rather than a list because the panel renders these headings and
 * nothing else: a category with no entry here would be a whole table of names
 * silently missing from the reference — and unfindable by its search, which
 * only reaches what a heading holds. This way a new one in `BuiltinCategory`
 * is a compile error rather than a quietly shorter list.
 */
const HEADINGS: Record<BuiltinCategory, string> = {
  special: "Patterns and Playback",
  list: "Lists",
  math: "Math",
  random: "Random",
  ugen: "Signals",
};

/** The headings paired with their categories, in the order written above. */
const CATEGORIES = Object.entries(HEADINGS) as [BuiltinCategory, string][];

/** Everything a search can match: the name, the doc, and the parameter names —
 *  `cutoff` should find the filters even though none of them is called that. */
function haystack(b: Builtin): string {
  return `${b.name} ${b.params.join(" ")} ${b.doc}`.toLowerCase();
}

/**
 * The language reference, beside the code rather than in a browser.
 *
 * Everything here comes from the same `language_metadata` the completion list
 * and the signature help read, so a UGen added in Rust documents itself in all
 * three places at once. The panel is not a copy of the README; it is the table
 * the README is generated from.
 *
 * It opens as an index rather than as a document: every category shut, and
 * inside one a plain alphabetical list of signatures. There are over a hundred
 * names, and the panel is a column beside the editor — a reader who arrives
 * knowing roughly what they want should be able to see the shape of the whole
 * thing at once, and only then ask one entry to say more.
 *
 * One of the right panel's two tabs; the frame around it belongs to
 * `RightPanel`.
 */
export function DocsPanel({ builtins, focus }: DocsPanelProps) {
  const [query, setQuery] = useState("");
  /** The categories the reader has opened. Empty to begin with. */
  const [opened, setOpened] = useState<ReadonlySet<BuiltinCategory>>(new Set());
  /** The one name showing its description, if any. One at a time: the list is
   *  what is being read, and a column of expanded entries is not a list. */
  const [expanded, setExpanded] = useState<string | null>(null);
  const entries = useRef(new Map<string, HTMLElement>());

  const searchable = useMemo(
    () => builtins.map((b) => ({ builtin: b, text: haystack(b) })),
    [builtins],
  );

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q === "") return builtins;
    return searchable.filter((s) => s.text.includes(q)).map((s) => s.builtin);
  }, [builtins, searchable, query]);

  // A search reaches across the categories, so while one is running every
  // category holding a match is open — a result hidden behind a shut heading
  // is a result that was not found.
  const searching = query.trim() !== "";

  const groups = useMemo(
    () =>
      CATEGORIES.map(([key, label]) => ({
        key,
        label,
        members: matches
          .filter((b) => b.category === key)
          .sort((a, b) => a.name.localeCompare(b.name)),
      })).filter((g) => g.members.length > 0),
    [matches],
  );

  const categoryOf = useMemo(
    () => new Map(builtins.map((b) => [b.name, b.category])),
    [builtins],
  );

  /**
   * A ⌘-click is a request for one name: the filter that was hiding it goes,
   * its category opens, and it is the entry left expanded.
   *
   * Being sent to a page that does not contain what was asked for is worse
   * than losing a search someone can retype.
   */
  useEffect(() => {
    if (focus === null) return;
    const category = categoryOf.get(focus.name);
    if (category === undefined) return;

    setQuery("");
    setOpened((prev) => new Set(prev).add(category));
    setExpanded(focus.name);
  }, [focus, categoryOf]);

  // Once that has rendered, so the row is in the document — and open — by the
  // time it is scrolled to.
  useEffect(() => {
    if (focus === null) return;
    entries.current.get(focus.name)?.scrollIntoView({ block: "center" });
  }, [focus, matches, opened]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="shrink-0 border-b border-neutral-800 p-2">
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search the reference"
          aria-label="Search the reference"
          className="w-full rounded-md border border-neutral-800 bg-neutral-950 px-2 py-1 text-sm text-neutral-100 placeholder:text-neutral-600 focus:border-blue-700 focus:outline-none"
        />
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {builtins.length === 0 ? (
          <p className="px-4 py-3 text-sm text-neutral-500">
            The language tables have not arrived yet.
          </p>
        ) : groups.length === 0 ? (
          <p className="px-4 py-3 text-sm text-neutral-500">
            Nothing matches <span className="font-mono text-neutral-400">{query}</span>.
          </p>
        ) : (
          groups.map((group) => {
            const open = searching || opened.has(group.key);
            return (
              <section key={group.key}>
                {/* Sticky, because an open category is longer than the panel
                    and a signature read halfway down it means less without the
                    kind of thing it belongs to. */}
                <h3 className="sticky top-0 z-10">
                  <button
                    onClick={() =>
                      setOpened((prev) => {
                        const next = new Set(prev);
                        if (!next.delete(group.key)) next.add(group.key);
                        return next;
                      })
                    }
                    aria-expanded={open}
                    className="flex w-full items-center gap-1.5 border-y border-neutral-800 bg-neutral-900 px-3 py-1.5 text-left text-xs font-semibold uppercase tracking-wide text-neutral-400 transition-colors hover:text-neutral-200"
                  >
                    <Chevron open={open} />
                    <span className="flex-1">{group.label}</span>
                    <span className="font-mono text-[10px] normal-case text-neutral-600">
                      {group.members.length}
                    </span>
                  </button>
                </h3>

                {open &&
                  group.members.map((b) => (
                    <Entry
                      key={b.name}
                      builtin={b}
                      expanded={b.name === expanded}
                      onToggle={() => {
                        setExpanded((prev) => (prev === b.name ? null : b.name));
                        // An entry opened out of a search keeps its category
                        // open, so clearing the search does not take it away
                        // again the moment it has been read.
                        setOpened((prev) => new Set(prev).add(b.category));
                      }}
                      ref={(el) => {
                        if (el) entries.current.set(b.name, el);
                        else entries.current.delete(b.name);
                      }}
                    />
                  ))}
              </section>
            );
          })
        )}
      </div>
    </div>
  );
}

/** The disclosure triangle, pointing at what it will do. */
function Chevron({ open }: { open: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      className={"h-3 w-3 shrink-0 transition-transform " + (open ? "rotate-90" : "")}
    >
      <path d="M9 6l6 6-6 6z" />
    </svg>
  );
}

/**
 * One name in the list: its signature, and its description when it is the one
 * being read.
 *
 * The signature alone is enough to answer most of what the panel is opened
 * for — which arguments, in which order — so it is what the list is made of,
 * and the prose is a second question rather than the answer to the first.
 */
function Entry({
  builtin,
  expanded,
  onToggle,
  ref,
}: {
  builtin: Builtin;
  expanded: boolean;
  onToggle: () => void;
  ref: (el: HTMLElement | null) => void;
}) {
  // The kinds are what decide whether a name may be written after a dot, so
  // they are worth saying out loud — `push` is missing after `60.` for a
  // reason, and this is where that reason is written down.
  const chained = builtin.arities.length > 0 && builtin.params.length > 0;

  return (
    <div ref={ref} className={expanded ? "bg-neutral-900/60" : ""}>
      <button
        onClick={onToggle}
        aria-expanded={expanded}
        className={
          "block w-full px-4 py-1 text-left font-mono text-sm leading-relaxed transition-colors " +
          (expanded ? "text-[#66B9C1]" : "text-blue-400 hover:bg-neutral-900/60")
        }
      >
        {signature(builtin)}
      </button>

      {expanded && (
        <div className="px-4 pb-2">
          {builtin.doc && (
            <p className="text-xs leading-snug text-neutral-400">{builtin.doc}</p>
          )}
          {chained && (
            <div className="mt-1 font-mono text-[10px] text-neutral-600">
              {builtin.receives} → {builtin.returns}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
