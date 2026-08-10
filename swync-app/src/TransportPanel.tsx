import { PatternPanel } from "./PatternPanel";
import type { GraphicalPattern } from "./patterns";

interface TransportPanelProps {
  patterns: GraphicalPattern[];
  onPatternsChange: (patterns: GraphicalPattern[]) => void;
  /** Open one in the composer, which lives in a tab rather than in here. */
  onOpenPattern: (id: string) => void;
  /** Open a file by path, for the panel's heading — it names the patterns file
   *  and opens it. */
  onOpenFile: (path: string) => void;
  /** The project's patterns file, passed through to the panel that writes it. */
  patternsPath: string | null;
  /** False when no folder is open, so there is nowhere to save patterns to. */
  hasProject: boolean;
  /** Beats in a bar, from the project's time signature — passed through to the
   *  grids, which rule a bar into that many. */
  beatsPerBar: number;
}

/**
 * The drawn patterns, in the right panel.
 *
 * The engine's own controls — tempo and volume — used to sit above them here.
 * They are in the title bar now, beside play and stop, since they are what you
 * reach for while the music is running and a panel you have to open first is
 * one you reach for too late. See `component/Transport`.
 *
 * What is left is a different kind of thing anyway: the drawn patterns are
 * part of the program, not the engine.
 *
 * This is one of the right panel's two tabs; the frame around it, including
 * the width and the drag handle, belongs to `RightPanel`.
 */
export function TransportPanel({
  patterns,
  onOpenPattern,
  onOpenFile,
  onPatternsChange,
  patternsPath,
  hasProject,
  beatsPerBar,
}: TransportPanelProps) {
  return (
    <PatternPanel
      patterns={patterns}
      onChange={onPatternsChange}
      onOpen={onOpenPattern}
      onOpenFile={onOpenFile}
      path={patternsPath}
      hasProject={hasProject}
      beatsPerBar={beatsPerBar}
    />
  );
}
