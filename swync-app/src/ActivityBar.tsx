import type { ReactNode } from "react";

/**
 * The strip of icons down a window edge, and the panel title under it.
 *
 * Both side panels wear one, mirrored, because they sit either side of the
 * same editor: a difference of a pixel or a shade between them would read as a
 * difference in kind, and they are the same kind of thing.
 *
 * This replaced a row of text tabs across the top of each panel — four on the
 * left, three on the right, wide enough that a narrowed panel scrolled the
 * selected one out of sight. Icons stacked down the edge cost the panel no
 * width at all, so the number of views stops being a thing the panel has to
 * pay for.
 *
 * The tray is drawn whether its panel is open or shut, which is what lets it
 * be the only control either panel needs: it is the way in, and something that
 * vanished along with the thing it opens could not be. An icon that is not
 * showing brings its view up; the one that is showing puts the panel away.
 */

/** One 24-grid glyph, sized for the tray. */
function Glyph({ d }: { d: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" className="h-5 w-5">
      <path d={d} />
    </svg>
  );
}

/**
 * The eight views' icons, in one place so the two trays cannot drift apart.
 *
 * Each is meant to be recognisable at 20px with nothing but a tooltip to lean
 * on: a folder, a magnifier, books on a shelf, a warning sign, notes on a
 * roll, an open book, a gear, and two faders on a desk.
 */
export const icons = {
  project: <Glyph d="M4 4h5l2 2h9a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z" />,
  search: (
    <Glyph d="M15.5 14h-.79l-.28-.27A6.47 6.47 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z" />
  ),
  libraries: <Glyph d="M3 3h3v15H3V3zm5 0h3v15H8V3zm5 0h3v15h-3V3zM2 19h20v2H2v-2z" />,
  problems: <Glyph d="M12 2 1 21h22L12 2zm1 16h-2v-2h2v2zm0-4h-2v-4h2v4z" />,
  patterns: <Glyph d="M3 5h9v3H3V5zm5 5.5h13v3H8v-3zM3 16h7v3H3v-3z" />,
  reference: (
    <Glyph d="M12 6.5C10.5 5.2 8.4 4.5 6 4.5c-1 0-2 .1-3 .4v13.6c1-.3 2-.4 3-.4 2.4 0 4.5.7 6 2 1.5-1.3 3.6-2 6-2 1 0 2 .1 3 .4V4.9c-1-.3-2-.4-3-.4-2.4 0-4.5.7-6 2zm-1 11.1c-1.4-.8-3.1-1.2-5-1.2-.7 0-1.4.1-2 .2V6.3c.6-.1 1.3-.2 2-.2 1.9 0 3.6.4 5 1.2v10.3zm2 0V7.3c1.4-.8 3.1-1.2 5-1.2.7 0 1.4.1 2 .2v10.3c-.6-.1-1.3-.2-2-.2-1.9 0-3.6.4-5 1.2z" />
  ),
  // Two faders, one up and one down: the shape a mixer strip has at a glance,
  // and the one thing on this tray that could not be mistaken for the gear.
  controls: (
    <Glyph d="M7 3h2v7h2v3H9v8H7v-8H5v-3h2V3zm8 0h2v11h2v3h-2v4h-2v-4h-2v-3h2V3z" />
  ),
  settings: (
    <Glyph d="M19.14 12.94c.04-.3.06-.61.06-.94s-.02-.64-.07-.94l2.03-1.58a.49.49 0 0 0 .12-.61l-1.92-3.32a.49.49 0 0 0-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54a.48.48 0 0 0-.48-.41h-3.84a.48.48 0 0 0-.48.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96a.48.48 0 0 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.01 1.58a.49.49 0 0 0-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.04.24.24.41.48.41h3.84c.24 0 .44-.17.48-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6A3.6 3.6 0 1 1 12 8.4a3.6 3.6 0 0 1 0 7.2z" />
  ),
};

interface ActivityIconProps {
  /** Named in the tooltip, and again in the panel's header while it is showing. */
  label: string;
  icon: ReactNode;
  /** Whether this is the view the panel is showing — false while the panel is shut. */
  selected: boolean;
  /** The edge the tray is pinned to, which decides the side the marker is drawn on. */
  side: "left" | "right";
  /** A badge, when there is a number worth carrying on the icon itself. */
  count?: number;
  /** Whether that number stopped a run, which is what decides the badge's colour. */
  countIsError?: boolean;
  onClick: () => void;
}

export function ActivityIcon({
  label,
  icon,
  selected,
  side,
  count,
  countIsError,
  onClick,
}: ActivityIconProps) {
  return (
    <button
      onClick={onClick}
      role="tab"
      aria-selected={selected}
      aria-label={label}
      title={label}
      className={
        // The marker sits on the outer edge, away from the panel, so the two
        // trays mirror each other rather than both pointing the same way.
        "relative flex h-11 w-full items-center justify-center transition-colors " +
        (side === "left" ? "border-l-2 " : "border-r-2 ") +
        (selected
          ? "border-blue-400 text-neutral-100"
          : "border-transparent text-neutral-500 hover:text-neutral-200")
      }
    >
      {icon}
      {count !== undefined && count > 0 && (
        // Rides the icon rather than the panel, so a failed run is legible
        // with every panel shut — which is the state most of them are in while
        // something is being played.
        <span
          className={
            "absolute bottom-1.5 right-1.5 min-w-4 rounded-full px-1 text-center text-[10px] font-semibold leading-4 text-white " +
            (countIsError ? "bg-red-600" : "bg-amber-600")
          }
        >
          {count}
        </span>
      )}
    </button>
  );
}

export function ActivityBar({
  side,
  children,
}: {
  side: "left" | "right";
  children: ReactNode;
}) {
  return (
    <div
      role="tablist"
      aria-orientation="vertical"
      className={
        "flex w-11 shrink-0 flex-col items-stretch bg-neutral-950/60 py-1 " +
        (side === "left"
          ? "border-r border-neutral-800"
          : "border-l border-neutral-800")
      }
    >
      {children}
    </div>
  );
}

/**
 * The name of the view a panel is showing.
 *
 * An icon and a tooltip are enough to pick a view but not enough to stay sure
 * of which one is up, and the panel has the room to say so once.
 */
export function PanelTitle({ children }: { children: ReactNode }) {
  return (
    <div className="flex shrink-0 items-center border-b border-neutral-800 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-neutral-400">
      {children}
    </div>
  );
}
