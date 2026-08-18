import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { swyncCompletions } from "./complete";
import { errorMarks } from "./errors";
import { builtinHelp } from "./help";
import { swyncLanguage } from "./language";
import { buildIndex, type LanguageMetadata } from "./metadata";
import { signatureHelp } from "./signature";
import { Ports } from "./ports";
import { Symbols } from "./symbols";
import { editorZoom } from "./zoom";

export { EMPTY_METADATA, loadMetadata, type LanguageMetadata } from "./metadata";
export { revealPosition, showErrorLines } from "./errors";
export { Symbols } from "./symbols";
export { Ports } from "./ports";
export { clampFontSize, DEFAULT_FONT_SIZE } from "./zoom";

/**
 * Signature help is off for now: the hint sits above the cursor, over the line
 * being read, and is in the way more often than it is wanted. Everything it
 * needs is still here and still typechecked — turning it back on is this flag.
 */
const SIGNATURE_HELP = false;

/**
 * Every swync editor extension, built from one metadata snapshot.
 *
 * Call this once per load and memoize the result: CodeMirror reconfigures when
 * the extension array's identity changes, and rebuilding it per render would
 * throw away the completion state on every keystroke.
 *
 * Safe to call with `EMPTY_METADATA` before the backend answers — highlighting
 * works immediately, and only the builtin colouring and completions are absent.
 *
 * @param patternNames The names of the patterns drawn in the side panel, as a
 * getter rather than a value: they change with every edit to the panel, and
 * this array's identity must not.
 * @param openDocs Where a ⌘-clicked builtin goes. Bound by the same rule as
 * `patternNames`: its identity has to hold across renders.
 * @param symbols What the file's imports bring in, kept up to date by whoever
 * owns the workspace — the same rule again, and for the same reason.
 * @param ports The MIDI ports this machine has, so `midiout("` can offer them.
 * Bound by the same identity rule; see `ports.ts` for why they are refreshed
 * on a clock where imports are refreshed on an edit.
 * @param zoom What ⌘ and the wheel over the editor asks for, in whole sizes.
 * Held outside the editor because it outlives any one of them; bound by the
 * same identity rule as the rest.
 */
export function swyncExtensions(
  meta: LanguageMetadata,
  patternNames: () => string[],
  openDocs: (name: string) => void,
  symbols: Symbols,
  ports: Ports,
  zoom: (steps: number) => void,
): Extension[] {
  const index = buildIndex(meta);
  return [
    swyncLanguage(meta, index, swyncCompletions(meta, patternNames, symbols, ports)),
    // Imports are looked up as the document changes rather than when a menu
    // opens, because the lookup is a round trip: asking at the menu would
    // answer after it had been drawn, and the first completion after writing a
    // `use` — the one that matters — would be the one without it.
    EditorView.updateListener.of((update) => {
      if (!update.docChanged) return;
      const doc = update.state.doc.toString();
      symbols.refresh(doc);
      // Warmed as the document is typed rather than when the menu opens, so
      // that the first `midiout("` already has the ports rather than filling
      // in a keystroke later. It returns immediately unless the document
      // mentions `midiout` and the last answer has gone stale.
      ports.refresh(doc);
    }),
    ...(SIGNATURE_HELP ? [signatureHelp(index)] : []),
    builtinHelp(index, openDocs),
    // Holds nothing until a run fails; the marks arrive by transaction, which
    // is what keeps this array's identity out of it.
    errorMarks(),
    editorZoom(zoom),
  ];
}
