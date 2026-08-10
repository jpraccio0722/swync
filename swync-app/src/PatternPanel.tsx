import { PatternMini } from "./PatternMini";
import {
  makePattern,
  nameError,
  resolution,
  type GraphicalPattern,
} from "./patterns";

/**
 * The patterns list, in the right panel.
 *
 * Read-only by design. A pattern is drawn in the composer, which opens as a
 * tab — the panel's job is to say which patterns exist and what each one
 * roughly sounds like, so you can find the one you want without the panel
 * becoming a second, narrower editor competing with the first.
 *
 * The name is the whole reason this connects to anything: it is bound in the
 * program's environment before the program runs, so `play(hats, hat)` in the
 * editor reads the row below.
 */

interface PatternRowProps {
  pattern: GraphicalPattern;
  error: string | null;
  onOpen: () => void;
  onRemove: () => void;
  /** Beats in a bar, for the picture's guide lines. */
  beatsPerBar: number;
}

function PatternRow({ pattern, error, onOpen, onRemove, beatsPerBar }: PatternRowProps) {
  return (
    <div className="group rounded-md border border-neutral-800 bg-neutral-900/60 p-2 transition-colors hover:border-neutral-700">
      <div className="flex items-center gap-1">
        <button
          onClick={onOpen}
          title="Open in the composer"
          className="min-w-0 flex-1 truncate text-left font-mono text-sm text-neutral-100 hover:text-blue-400"
        >
          {pattern.name}
        </button>
        <span className="shrink-0 text-[10px] uppercase tracking-wide text-neutral-600">
          {pattern.mode === "drums" ? "drums" : "roll"} · {resolution(pattern)}
        </span>
        <button
          onClick={onRemove}
          title="Delete pattern"
          className="rounded p-1 text-neutral-600 opacity-0 transition-opacity hover:bg-neutral-800 hover:text-red-400 group-hover:opacity-100"
        >
          <svg viewBox="0 0 24 24" fill="currentColor" className="h-3.5 w-3.5">
            <path d="M18.3 5.7 12 12l6.3 6.3-1.4 1.4L10.6 13.4 4.3 19.7 2.9 18.3 9.2 12 2.9 5.7 4.3 4.3l6.3 6.3 6.3-6.3z" />
          </svg>
        </button>
      </div>

      {error && <p className="mt-1 text-[11px] text-red-400">{error}</p>}

      {/* The picture is a button too: it is the biggest thing in the row and
          the most obvious thing to click. */}
      <button onClick={onOpen} title="Open in the composer" className="mt-2 block w-full">
        <PatternMini pattern={pattern} beatsPerBar={beatsPerBar} />
      </button>
    </div>
  );
}

interface PatternPanelProps {
  patterns: GraphicalPattern[];
  onChange: (patterns: GraphicalPattern[]) => void;
  /** Open one in the composer, as a tab. */
  onOpen: (id: string) => void;
  /** Open the patterns file itself, as a tab. */
  onOpenFile: (path: string) => void;
  /** False when no folder is open. Patterns still draw and still play — they
   *  travel with the eval — but there is nowhere to write them, so the panel
   *  has to say so rather than let the work look saved. */
  hasProject: boolean;
  /** The project's patterns file, once it is known. Named on screen because a
   *  panel that quietly writes a file into someone's project should say so —
   *  and because it is a file they can open, edit and keep. */
  path: string | null;
  /** Beats in a bar, from the project's time signature. The cells are shares
   *  of a bar and so mean the same thing in any signature; what this moves is
   *  the beat lines they are read against. */
  beatsPerBar: number;
}

export function PatternPanel({
  patterns,
  onChange,
  beatsPerBar,
  onOpen,
  onOpenFile,
  path,
  hasProject,
}: PatternPanelProps) {
  const add = () => {
    const pattern = makePattern(patterns);
    onChange([...patterns, pattern]);
    // Straight into the composer: a new pattern is empty, and an empty row in
    // the list is nothing to look at.
    onOpen(pattern.id);
  };

  return (
    <section className="border-t border-neutral-800 px-4 py-4">
      <div className="flex items-center justify-between">
        {/* The heading opens the file behind it. There is a real file on disk
            holding these, the panel already says so below, and the name of a
            thing is the obvious handle for the thing itself. Without a project
            there is no file to open, so it stays a plain heading. */}
        {hasProject && path !== null ? (
          <button
            onClick={() => onOpenFile(path)}
            title={`Open ${path}`}
            className="text-xs font-semibold uppercase tracking-wide text-neutral-400 transition-colors hover:text-blue-400"
          >
            Patterns
          </button>
        ) : (
          <h2 className="text-xs font-semibold uppercase tracking-wide text-neutral-400">
            Patterns
          </h2>
        )}
        <button
          onClick={add}
          title="New pattern"
          className="rounded-md p-1 text-neutral-400 transition-colors hover:bg-neutral-800 hover:text-neutral-100"
        >
          <svg viewBox="0 0 24 24" fill="currentColor" className="h-4 w-4">
            <path d="M11 11V5h2v6h6v2h-6v6h-2v-6H5v-2z" />
          </svg>
        </button>
      </div>

      {patterns.length === 0 ? (
        <p className="mt-2 text-xs leading-relaxed text-neutral-500">
          None yet. Add one, draw it, and play it from the editor:{" "}
          <span className="font-mono text-neutral-400">play(name, kick)</span>
        </p>
      ) : (
        <div className="mt-3 flex flex-col gap-2">
          {patterns.map((p) => (
            <PatternRow
              key={p.id}
              pattern={p}
              error={nameError(p, patterns)}
              onOpen={() => onOpen(p.id)}
              onRemove={() => onChange(patterns.filter((q) => q.id !== p.id))}
              beatsPerBar={beatsPerBar}
            />
          ))}
        </div>
      )}

      {!hasProject && (
        <p className="mt-3 rounded border border-amber-900/50 bg-amber-950/20 px-2 py-1.5 text-[11px] leading-relaxed text-amber-500/90">
          No folder open, so these are not being saved. They still play — an
          eval carries them — but they go when the app does.{" "}
          <span className="text-amber-400">File ▸ New Project…</span> picks one.
        </p>
      )}

      {hasProject && path !== null && (
        <p className="mt-3 text-[11px] leading-relaxed text-neutral-600" title={path}>
          Saved in{" "}
          <span className="font-mono text-neutral-500">{path.split(/[\\/]/).pop()}</span>, and
          in scope for every file in this project.
        </p>
      )}
    </section>
  );
}
