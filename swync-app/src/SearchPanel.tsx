import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { parentOf } from "./projectTree";

/** One occurrence, as `search_project` reports it. */
interface Match {
  /** 1-based, for the editor's own jump. */
  line: number;
  /** 1-based, counted in characters. */
  column: number;
  /** The snippet, in three pieces so the middle one can be marked without this
   *  side counting characters — which it would count differently. */
  before: string;
  hit: string;
  after: string;
}

interface FileMatches {
  path: string;
  name: string;
  matches: Match[];
}

interface Results {
  files: FileMatches[];
  total: number;
  /** True when the search stopped at its own ceiling rather than at the end of
   *  the project. */
  truncated: boolean;
}

/** How long the panel waits after a keystroke before searching.
 *
 *  A search walks and reads the project, so one per character typed would have
 *  the disk chasing a query nobody has finished writing. Short enough that
 *  stopping to think produces the answer without anyone asking for it. */
const SEARCH_DELAY = 250;

const EMPTY: Results = { files: [], total: 0, truncated: false };

interface SearchPanelProps {
  /** The project to search, or null when none is open. */
  root: string | null;
  /** Bumped when the project is reloaded, so a stale result set is searched
   *  again rather than left describing files that may have moved. */
  version: number;
  /** Show a hit: open its file and put the cursor on it. */
  onOpen: (path: string, line: number, column: number) => void;
}

/**
 * Find a piece of text anywhere in the project.
 *
 * The tree next door answers "where is the file called this"; this answers
 * "where is the thing I wrote", which is the question that gets asked in a
 * project whose files are a dozen names you chose yourself — where a `fn` is
 * defined, everywhere a sample is loaded, what still calls something by the
 * name it had last week.
 *
 * Results are grouped by file and each one is a link into the editor, so the
 * panel is a way of getting somewhere rather than a report to read.
 */
export function SearchPanel({ root, version, onOpen }: SearchPanelProps) {
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [wholeWord, setWholeWord] = useState(false);

  const [results, setResults] = useState<Results>(EMPTY);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());

  // Which search the panel is showing. Typing fast starts several, and they
  // come back in whatever order the disk gives them up.
  const latest = useRef(0);

  const run = useCallback(
    (query: string, caseSensitive: boolean, wholeWord: boolean) => {
      if (root === null || query === "") {
        latest.current += 1; // abandon anything in flight
        setResults(EMPTY);
        setSearching(false);
        setError(null);
        return;
      }

      const token = (latest.current += 1);
      setSearching(true);
      invoke<Results>("search_project", { root, query, caseSensitive, wholeWord })
        .then((found) => {
          if (latest.current !== token) return;
          setResults(found);
          setError(null);
          setSearching(false);
          // A new set of results is a new set of files, and none of them
          // inherit the last search's folded-away state.
          setCollapsed(new Set());
        })
        .catch((e) => {
          if (latest.current !== token) return;
          setResults(EMPTY);
          setError(String(e));
          setSearching(false);
        });
    },
    [root],
  );

  useEffect(() => {
    const timer = setTimeout(() => run(query, caseSensitive, wholeWord), SEARCH_DELAY);
    return () => clearTimeout(timer);
  }, [query, caseSensitive, wholeWord, version, run]);

  if (root === null) {
    return (
      <div className="px-4 py-4 text-xs leading-relaxed text-neutral-500">
        <p>
          No project open. Search looks through the project's folder, so there
          has to be one — pick it with{" "}
          <span className="text-neutral-400">File ▸ Open Project…</span>.
        </p>
      </div>
    );
  }

  return (
    <>
      <div className="flex items-center gap-1 border-b border-neutral-800 px-3 py-1.5">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          // Enter asks again rather than doing nothing: the project may have
          // changed under a result set that is still on screen.
          onKeyDown={(e) => {
            if (e.key === "Enter") run(query, caseSensitive, wholeWord);
            if (e.key === "Escape") setQuery("");
          }}
          placeholder="Search project"
          spellCheck={false}
          aria-label="Search project"
          className="min-w-0 flex-1 rounded bg-neutral-800/60 px-2 py-1 text-xs text-neutral-100 placeholder-neutral-600 outline-none transition-colors focus:bg-neutral-800 focus:outline focus:outline-1 focus:outline-blue-500"
        />
        <Toggle
          label="Aa"
          title="Match case"
          on={caseSensitive}
          onClick={() => setCaseSensitive((on) => !on)}
        />
        <Toggle
          label="ab"
          title="Match whole word"
          on={wholeWord}
          onClick={() => setWholeWord((on) => !on)}
        />
      </div>

      <div className="min-h-0 flex-1">
        <Summary
          query={query}
          searching={searching}
          error={error}
          results={results}
        />

        {results.files.map((file) => {
          const shut = collapsed.has(file.path);
          const folder = parentOf(file.path);
          return (
            <div key={file.path}>
              <button
                onClick={() =>
                  setCollapsed((prev) => {
                    const next = new Set(prev);
                    if (!next.delete(file.path)) next.add(file.path);
                    return next;
                  })
                }
                title={file.path}
                aria-expanded={!shut}
                className="flex w-full items-center gap-1.5 px-2 py-0.5 text-left text-xs text-neutral-300 transition-colors hover:bg-neutral-900"
              >
                <svg
                  viewBox="0 0 24 24"
                  fill="currentColor"
                  className={
                    "h-3 w-3 shrink-0 text-neutral-500 transition-transform " +
                    (shut ? "" : "rotate-90")
                  }
                >
                  <path d="M9 6l6 6-6 6z" />
                </svg>
                <span className="shrink-0 font-medium">{file.name}</span>
                {folder && (
                  <span className="min-w-0 flex-1 truncate text-[10px] text-neutral-600">
                    {folder}
                  </span>
                )}
                <span className="ml-auto shrink-0 rounded-full bg-neutral-800 px-1.5 text-[10px] leading-4 text-neutral-400">
                  {file.matches.length}
                </span>
              </button>

              {!shut &&
                file.matches.map((match, i) => (
                  <button
                    // Two hits can sit on one line, so the position is part of
                    // what tells them apart.
                    key={`${match.line}:${match.column}:${i}`}
                    onClick={() => onOpen(file.path, match.line, match.column)}
                    title={`${file.path}:${match.line}`}
                    className="flex w-full items-baseline gap-2 py-0.5 pl-7 pr-2 text-left font-mono text-[11px] text-neutral-400 transition-colors hover:bg-neutral-900 hover:text-neutral-200"
                  >
                    <span className="w-8 shrink-0 text-right text-neutral-600">
                      {match.line}
                    </span>
                    <span className="min-w-0 flex-1 truncate">
                      {match.before}
                      <mark className="rounded-sm bg-amber-500/30 text-amber-200">
                        {match.hit}
                      </mark>
                      {match.after}
                    </span>
                  </button>
                ))}
            </div>
          );
        })}
      </div>
    </>
  );
}

/** What the panel says above the list, which is different in every state it
 *  can be in — and never nothing, since an empty panel below an empty list
 *  does not say whether the search ran. */
function Summary({
  query,
  searching,
  error,
  results,
}: {
  query: string;
  searching: boolean;
  error: string | null;
  results: Results;
}) {
  if (error !== null) {
    return (
      <p className="px-3 py-2 text-xs leading-snug text-red-400">{error}</p>
    );
  }
  if (query === "") {
    return (
      <p className="px-3 py-2 text-xs text-neutral-600">
        Type to search every file in the project.
      </p>
    );
  }
  if (searching && results.files.length === 0) {
    return <p className="px-3 py-2 text-xs text-neutral-600">searching…</p>;
  }
  if (results.total === 0) {
    return (
      <p className="px-3 py-2 text-xs text-neutral-600">No results.</p>
    );
  }

  const results_ = results.total === 1 ? "result" : "results";
  const files = results.files.length === 1 ? "file" : "files";
  return (
    <p className="px-3 py-2 text-xs text-neutral-500">
      {results.total} {results_} in {results.files.length} {files}
      {results.truncated && (
        <span className="text-neutral-600">
          {" "}
          — stopped counting there, narrow the search to see the rest
        </span>
      )}
    </p>
  );
}

function Toggle({
  label,
  title,
  on,
  onClick,
}: {
  label: string;
  title: string;
  on: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      aria-label={title}
      aria-pressed={on}
      className={
        "shrink-0 rounded px-1.5 py-0.5 font-mono text-[11px] transition-colors " +
        (on
          ? "bg-blue-600/30 text-blue-200"
          : "text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200")
      }
    >
      {label}
    </button>
  );
}
