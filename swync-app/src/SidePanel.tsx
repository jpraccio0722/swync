import type { ReactNode } from "react";
import { PanelTab, PanelTabs } from "./PanelTabs";

/** Which of the panel's views is on top. */
export type SideTab = "problems" | "project" | "search" | "libraries";

interface SidePanelProps {
  open: boolean;
  /** Pixels wide, as the drag handle has left it. */
  width: number;
  /** Grab handle for that drag, drawn along the panel's trailing edge. */
  onResizeStart: (e: React.PointerEvent) => void;
  tab: SideTab;
  onTabChange: (tab: SideTab) => void;
  /** Drawn on the Problems tab, so a failure is visible from the others. */
  problemCount: number;
  problems: ReactNode;
  project: ReactNode;
  search: ReactNode;
  libraries: ReactNode;
}

/**
 * The left-hand panel: the project's files, what is in them, and the last
 * run's problems.
 *
 * Every view stays mounted and the hidden ones are only taken off screen, so
 * switching tabs keeps what each was showing — the folders opened in the tree
 * and a set of search results are worth as much as the errors beside them, and
 * rebuilding any of them on every click would be a tax on looking at the rest.
 */
export function SidePanel({
  open,
  width,
  onResizeStart,
  tab,
  onTabChange,
  problemCount,
  problems,
  project,
  search,
  libraries,
}: SidePanelProps) {
  return (
    <aside
      style={{ width }}
      className={
        "relative shrink-0 flex-col border-r border-neutral-800 bg-neutral-950/40 " +
        (open ? "flex" : "hidden")
      }
    >
      <PanelTabs>
        <PanelTab
          label="Project"
          selected={tab === "project"}
          onClick={() => onTabChange("project")}
        />
        <PanelTab
          label="Search"
          selected={tab === "search"}
          onClick={() => onTabChange("search")}
        />
        <PanelTab
          label="Libraries"
          selected={tab === "libraries"}
          onClick={() => onTabChange("libraries")}
        />
        <PanelTab
          label="Problems"
          selected={tab === "problems"}
          count={problemCount}
          onClick={() => onTabChange("problems")}
        />
      </PanelTabs>

      <div
        className={
          "min-h-0 flex-1 flex-col overflow-y-auto " +
          (tab === "problems" ? "flex" : "hidden")
        }
      >
        {problems}
      </div>
      <div
        className={
          "min-h-0 flex-1 flex-col overflow-y-auto " +
          (tab === "project" ? "flex" : "hidden")
        }
      >
        {project}
      </div>
      <div
        className={
          "min-h-0 flex-1 flex-col overflow-y-auto " +
          (tab === "search" ? "flex" : "hidden")
        }
      >
        {search}
      </div>
      <div
        className={
          "min-h-0 flex-1 flex-col overflow-y-auto " +
          (tab === "libraries" ? "flex" : "hidden")
        }
      >
        {libraries}
      </div>

      {/* Mirrors the transport's handle, on the edge that faces the editor. */}
      <div
        onPointerDown={onResizeStart}
        title="Drag to resize"
        role="separator"
        aria-orientation="vertical"
        className="absolute inset-y-0 -right-1 z-10 w-2 cursor-col-resize hover:bg-blue-400/40 active:bg-blue-600/60"
      />
    </aside>
  );
}
