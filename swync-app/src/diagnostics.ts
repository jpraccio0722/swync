/**
 * What comes back when a program does not run.
 *
 * Mirrors `src-tauri/src/diagnostic.rs`. Every `invoke` rejection reaches the
 * problems panel through `toDiagnostic`, including the ones the backend never
 * shaped — an IPC failure, a command that is not registered — because the
 * panel's job is that nothing fails without being shown.
 */

/** Which pass refused the program. */
export type Stage =
  | "lex"
  | "parse"
  | "import"
  | "lower"
  | "realize"
  | "scheduler"
  | "engine";

export interface Diagnostic {
  stage: Stage;
  message: string;
  /** 1-based, or null when the failing pass had no position to give. */
  line: number | null;
  /** 1-based, counted in characters. Null exactly when `line` is. */
  column: number | null;
  /** The source line the position falls on, as it was when the run happened. */
  snippet: string | null;
  /** The imported file the position is in, absolute. Null — which is every
   *  diagnostic a program without imports can produce — means the file that
   *  was run, which the editor is already showing. */
  file: string | null;
}

const STAGES: Stage[] = [
  "lex",
  "parse",
  "import",
  "lower",
  "realize",
  "scheduler",
  "engine",
];

/**
 * The badge each stage wears in the panel.
 *
 * Lexing and parsing share one: the difference between "that character is not
 * in the language" and "those tokens are not a program" is real to the
 * compiler and noise to the person typing.
 */
export const STAGE_LABEL: Record<Stage, string> = {
  lex: "syntax",
  parse: "syntax",
  import: "import",
  lower: "compile",
  realize: "audio graph",
  // A note that failed while the pattern was playing, rather than a program
  // that was refused before it ever started.
  scheduler: "playback",
  // Broader here than the backend's own name for it: this side also files a
  // failed save, a failed open and a dead IPC call under the stage that is not
  // a compiler pass, and none of those are the audio engine.
  engine: "runtime",
};

function isStage(value: unknown): value is Stage {
  return typeof value === "string" && (STAGES as string[]).includes(value);
}

/** Something readable for a rejection that is neither a string nor an Error.
 *  `String(value)` on an object is "[object Object]", which tells nobody
 *  anything; JSON does, unless the value is cyclic, in which case nothing can. */
function describe(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

/**
 * Read an unknown rejection as a diagnostic.
 *
 * Tauri hands back whatever the command's error serialized to, so the happy
 * path is a `Diagnostic` already. Anything else — a bare string from a command
 * that still returns one, an `Error` thrown before the call left the webview —
 * becomes an engine-stage diagnostic rather than being dropped.
 *
 * @param fallback Prefixed to a message that arrived with no shape of its own,
 * to say which action failed: the raw text of, say, a filesystem error does not
 * mention that it was a save.
 */
export function toDiagnostic(error: unknown, fallback?: string): Diagnostic {
  const raw = error as Partial<Diagnostic> | null | undefined;

  if (raw && typeof raw === "object" && typeof raw.message === "string") {
    return {
      stage: isStage(raw.stage) ? raw.stage : "engine",
      message: raw.message,
      // A position is only ever a matched pair; half of one would put the
      // panel's "line 4" link somewhere the error never claimed to be.
      line: typeof raw.line === "number" ? raw.line : null,
      column: typeof raw.line === "number" && typeof raw.column === "number"
        ? raw.column
        : null,
      snippet: typeof raw.snippet === "string" ? raw.snippet : null,
      file: typeof raw.file === "string" ? raw.file : null,
    };
  }

  const text =
    typeof error === "string"
      ? error
      : error instanceof Error
        ? error.message
        : describe(error);

  return {
    stage: "engine",
    message: fallback ? `${fallback}: ${text}` : text,
    line: null,
    column: null,
    snippet: null,
    file: null,
  };
}
