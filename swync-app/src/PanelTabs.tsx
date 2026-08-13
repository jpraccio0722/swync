import { useEffect, useRef } from "react";

/**
 * The tab strip both side panels wear.
 *
 * Shared rather than written twice because the two panels sit either side of
 * the same editor: a difference of a pixel or a shade between them would read
 * as a difference in kind, and they are the same kind of thing.
 */

interface PanelTabProps {
  label: string;
  selected: boolean;
  /** A badge, when there is a number worth carrying on the tab itself. */
  count?: number;
  onClick: () => void;
}

export function PanelTab({ label, selected, count, onClick }: PanelTabProps) {
  const ref = useRef<HTMLButtonElement>(null);

  // A narrowed panel scrolls its strip rather than wrapping it, so the tab
  // that is showing can end up out of sight — and a selected tab nobody can
  // find reads as the panel having lost it. Watching the strip rather than
  // only the selection is what covers the drag: the width the tab went missing
  // at is the resize handle's doing, and nothing else would tell us.
  useEffect(() => {
    const tab = ref.current;
    const strip = tab?.parentElement;
    if (!selected || !tab || !strip) return;

    const reveal = () => tab.scrollIntoView({ block: "nearest", inline: "nearest" });
    reveal();

    const observer = new ResizeObserver(reveal);
    observer.observe(strip);
    return () => observer.disconnect();
  }, [selected]);

  return (
    <button
      ref={ref}
      onClick={onClick}
      role="tab"
      aria-selected={selected}
      className={
        "flex shrink-0 items-center gap-1.5 whitespace-nowrap border-b-2 px-3 py-2 text-xs font-semibold uppercase tracking-wide transition-colors " +
        (selected
          ? "border-blue-400 text-neutral-100"
          : "border-transparent text-neutral-500 hover:text-neutral-300")
      }
    >
      {label}
      {count !== undefined && count > 0 && (
        <span className="font-mono text-[10px] normal-case text-red-400">{count}</span>
      )}
    </button>
  );
}

export function PanelTabs({ children }: { children: React.ReactNode }) {
  return (
    // The tabs are wider than the panel's minimum width, and the panel does not
    // clip — its resize handle deliberately hangs a pixel outside it — so
    // without a scroller of its own the strip ran out over the editor. It
    // scrolls rather than wraps because the other panel's strip is one row
    // high, and two strips of different heights either side of the same editor
    // read as two different things. The scrollbar is hidden: it would be as
    // tall as the tabs it sits under, and dragging the panel wider is the
    // gesture anyone reaching for it actually wants.
    <div
      role="tablist"
      className="flex items-stretch overflow-x-auto border-b border-neutral-800 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
    >
      {children}
    </div>
  );
}
