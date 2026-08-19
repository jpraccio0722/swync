import type { ReactNode } from "react";
import { ActivityBar, ActivityIcon, PanelTitle, icons } from "./ActivityBar";

/** Which of the panel's three views is on top. */
export type RightTab = "transport" | "docs" | "settings";

/** What each view is called, on its tooltip and in the panel's header. */
const LABELS: Record<RightTab, string> = {
  transport: "Patterns",
  docs: "Reference",
  settings: "Settings",
};

/** Which icon each wears, keyed the way the views are rather than the way the
 *  icons are, since the two names differ for the pattern panel. */
const GLYPHS: Record<RightTab, ReactNode> = {
  transport: icons.patterns,
  docs: icons.reference,
  settings: icons.settings,
};

interface RightPanelProps {
  open: boolean;
  /** Pixels wide, as the drag handle has left it. */
  width: number;
  /** Grab handle for that drag, drawn along the panel's leading edge. */
  onResizeStart: (e: React.PointerEvent) => void;
  tab: RightTab;
  /** A tray icon was hit; the caller owns `open` and so decides what that means. */
  onSelect: (tab: RightTab) => void;
  transport: ReactNode;
  docs: ReactNode;
  settings: ReactNode;
}

/**
 * The right-hand panel: the drawn patterns, the language reference, and what
 * the app itself is set to.
 *
 * They share a panel because none of them is the program — they are the things
 * you reach for while writing one, and all of them want to be reachable
 * without giving up the file you are looking at.
 *
 * Like the left panel, its tray of icons is pinned to the window's edge and
 * outlives the panel, and every view stays mounted with the hidden ones only
 * taken off screen: the reference keeps its search and its scroll position
 * while the patterns are showing, and the settings keep the folder they have
 * already read.
 */
export function RightPanel({
  open,
  width,
  onResizeStart,
  tab,
  onSelect,
  transport,
  docs,
  settings,
}: RightPanelProps) {
  return (
    <>
      <aside
        style={{ width }}
        className={
          "relative shrink-0 flex-col border-l border-neutral-800 bg-neutral-950/40 " +
          (open ? "flex" : "hidden")
        }
      >
        {/* The drag handle sits on the border itself and is a little wider than
            it looks, so the panel can be resized without hunting for one pixel. */}
        <div
          onPointerDown={onResizeStart}
          title="Drag to resize"
          role="separator"
          aria-orientation="vertical"
          className="absolute inset-y-0 -left-1 z-10 w-2 cursor-col-resize hover:bg-blue-400/40 active:bg-blue-600/60"
        />

        {/* No close control of its own: the tray's icons work both ways, which
            is the one place a reader already looks. */}
        <PanelTitle>{LABELS[tab]}</PanelTitle>

        {/* The transport scrolls as a whole: the patterns list has no bound, and
            clipping the sliders off the bottom of a short window would be worse
            than scrolling past them. */}
        <div
          className={
            "min-h-0 flex-1 flex-col overflow-y-auto " +
            (tab === "transport" ? "flex" : "hidden")
          }
        >
          {transport}
        </div>
        {/* The reference does its own scrolling, so its search box can stay put
            while the list moves under it — a search that scrolls away is one
            that has to be scrolled back to. */}
        <div
          className={
            "min-h-0 flex-1 flex-col overflow-hidden " + (tab === "docs" ? "flex" : "hidden")
          }
        >
          {docs}
        </div>
        {/* Scrolls as a whole, like the patterns: it is a short column of
            controls, and a window too small for them should move them rather
            than cut the bottom one off. */}
        <div
          className={
            "min-h-0 flex-1 flex-col overflow-y-auto " +
            (tab === "settings" ? "flex" : "hidden")
          }
        >
          {settings}
        </div>
      </aside>

      <ActivityBar side="right">
        {(["transport", "docs", "settings"] as const).map((name) => (
          <ActivityIcon
            key={name}
            side="right"
            label={LABELS[name]}
            icon={GLYPHS[name]}
            selected={open && tab === name}
            onClick={() => onSelect(name)}
          />
        ))}
      </ActivityBar>
    </>
  );
}
