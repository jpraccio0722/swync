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
  return (
    <button
      onClick={onClick}
      role="tab"
      aria-selected={selected}
      className={
        "flex items-center gap-1.5 border-b-2 px-3 py-2 text-xs font-semibold uppercase tracking-wide transition-colors " +
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
    <div role="tablist" className="flex items-stretch border-b border-neutral-800">
      {children}
    </div>
  );
}
