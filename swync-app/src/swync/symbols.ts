import { invoke } from "@tauri-apps/api/core";

/**
 * What the file's `use` lines actually brought in.
 *
 * Completion scrapes the buffer with regexes, which reaches every spelling of a
 * `use` but one: a glob's names live in a file this side has never read. That
 * one case is the whole point of an installed library — `use kit::*` is how you
 * write with one — so the names have to come from the backend, which resolves
 * them exactly as running the file would.
 *
 * Asked when the file's `use` lines change rather than as it is typed. Nothing
 * else in the document can change the answer, and expansion reads files off a
 * disk.
 */

/** One name the file may write, as `module_symbols` reports it. */
export interface Symbol {
  /** In the spelling this file uses: `kick`, `kit::kick`, `k::kick`. */
  name: string;
  /** A `fn`'s parameters, in order. Empty for anything that is not one. */
  params: string[];
  /** Which of those have a default, and so may be left out — which is also
   *  what makes one usable as a `play` lane. */
  optional: boolean[];
  /** True for a `fn`: something a `play` could name as an instrument. */
  callable: boolean;
}

/** Where the file being edited sits, which is what a `use` resolves against. */
export interface Workspace {
  path: string | null;
  root: string | null;
}

/** Every `use` line in a document, which is the whole of what the answer
 *  depends on. */
const USE_LINE = /^[ \t]*use[ \t][^\n]*/gm;

function useLines(doc: string): string {
  return (doc.match(USE_LINE) ?? []).join("\n");
}

/**
 * A live set of symbols for one editor.
 *
 * Held as a mutable object rather than passed in, because completion reads it
 * at the moment a menu opens and the fetch that fills it is asynchronous — the
 * same reason the drawn patterns are read through a getter.
 */
export class Symbols {
  private symbols: Symbol[] = [];
  /** The `use` lines the current set was fetched for, so an edit that leaves
   *  them alone costs nothing. Null before the first fetch. */
  private fetchedFor: string | null = null;
  private workspace: Workspace = { path: null, root: null };
  /** Bumped per request, so a slow answer cannot overwrite a newer one. */
  private generation = 0;

  /** What the file may write, as of the last answer. */
  current(): Symbol[] {
    return this.symbols;
  }

  /** Where the file being edited is. A move changes what its `use`s reach. */
  setWorkspace(workspace: Workspace) {
    if (
      workspace.path === this.workspace.path &&
      workspace.root === this.workspace.root
    ) {
      return;
    }
    this.workspace = workspace;
    this.fetchedFor = null;
  }

  /**
   * Look again if this document's imports differ from the last one's.
   *
   * Safe to call on every keystroke: the common edit changes no `use` line and
   * returns before touching anything.
   */
  refresh(doc: string) {
    const lines = useLines(doc);
    if (lines === this.fetchedFor) return;
    this.fetchedFor = lines;

    // Nothing imported is nothing to ask about — and the file's own
    // definitions are already scraped from the buffer, which is both faster
    // and correct while it is half-written.
    if (!lines) {
      this.symbols = [];
      return;
    }

    const generation = ++this.generation;
    void invoke<Symbol[]>("module_symbols", {
      // The `use` lines alone, not the buffer: they are the whole of what the
      // answer depends on, and a file mid-edit would parse to nothing and take
      // its imports down with it.
      code: lines,
      workspace: this.workspace,
    })
      .then((symbols) => {
        if (generation === this.generation) this.symbols = symbols;
      })
      .catch(() => {
        // A `use` that names nothing yet is the ordinary state of a line being
        // typed. What was scraped from the buffer still completes.
        if (generation === this.generation) this.symbols = [];
      });
  }
}
