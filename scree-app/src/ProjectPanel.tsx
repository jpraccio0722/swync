import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { toDiagnostic, type Diagnostic } from "./diagnostics";
import {
  basename,
  isWithin,
  join,
  parentOf,
  useProjectTree,
  type Entry,
  type ProjectTree,
} from "./projectTree";

import ChevronRight from './assets/chevron-right.svg?react'
import FileSVG from './assets/file.svg?react'

export type { Entry } from "./projectTree";

/** A `use` a move could not correct, as `move_path` reports it. */
interface Unfollowed {
  file: string;
  detail: string;
}

/** What `move_path` answers with. */
interface Moved {
  path: string;
  unfollowed: Unfollowed[];
}

/** How far in each level of the tree sits, in pixels. */
const INDENT = 12;

/** A row being typed into, before the thing it names exists. */
interface Draft {
  /** The folder it will be made in. */
  parent: string;
  kind: "file" | "folder";
}

/**
 * The row the panel is acting on.
 *
 * Carries whether it is a folder as well as where it is, because that is what
 * decides where a New File goes — into the selection, or beside it — and after
 * a rename or a create there is no `Entry` to ask.
 */
interface Selected {
  path: string;
  isDir: boolean;
}

/** An open context menu, and what it is about. */
interface Menu {
  x: number;
  y: number;
  /** The row it was opened on, or null for the space below the tree — which
   *  is the project folder itself, and can still be made things in. */
  entry: Entry | null;
}

/**
 * Everything a row needs that is not the row.
 *
 * Through a context rather than props because a row is drawn by a folder which
 * is drawn by a folder: threading a dozen callbacks down an arbitrarily deep
 * recursion obscures the one thing a `Row` is actually about.
 */
interface Tree {
  tree: ProjectTree;
  activePath: string | null;
  selected: Selected | null;
  setSelected: (selection: Selected | null) => void;
  openFile: (path: string) => void;
  renaming: string | null;
  startRename: (path: string) => void;
  commitRename: (entry: Entry, name: string) => void;
  cancelEdit: () => void;
  draft: Draft | null;
  commitDraft: (name: string) => void;
  remove: (entry: Entry) => void;
  openMenu: (menu: Menu) => void;
  /** The row being dragged, so a folder can refuse to be dropped into itself
   *  before the pointer is over it rather than after. */
  dragging: Selected | null;
  setDragging: (row: Selected | null) => void;
  dropTarget: string | null;
  setDropTarget: (path: string | null) => void;
  move: (from: Selected, intoDir: string) => void;
}

const TreeContext = createContext<Tree | null>(null);

function useTree(): Tree {
  const tree = useContext(TreeContext);
  if (tree === null) throw new Error("a tree row outside the tree");
  return tree;
}

function Chevron({ open }: { open: boolean }) {
  return (
    <ChevronRight
      className={
        "h-3 w-3 shrink-0 text-neutral-500 transition-transform " +
        (open ? "rotate-90" : "")
      }
    />
  );
}

function FileIcon() {
  return (
    <FileSVG className="h-4 w-4 shrink-0 text-neutral-600 fill-neutral-600"/>
  );
}

/**
 * A name being typed, for a rename or a new file.
 *
 * The two are one component because they are one interaction: a row that is an
 * input, committed with Enter and abandoned with Escape or by clicking away.
 * The only difference is what is in it to start with.
 *
 * A `.scree` name opens with the extension left out of the selection, the way
 * every file browser does it — the part you are renaming is the name, and
 * having to retype `.scree` to change `drums` to `percussion` is a small tax
 * charged every single time.
 */
function NameInput({
  initial,
  depth,
  icon,
  onCommit,
  onCancel,
}: {
  initial: string;
  depth: number;
  icon: React.ReactNode;
  onCommit: (name: string) => void;
  onCancel: () => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  // Committing on blur as well as on Enter means the commit can be asked for
  // twice — Enter moves the focus away — and the second one would be against a
  // file that has already been renamed.
  const done = useRef(false);

  useEffect(() => {
    const field = input.current;
    if (!field) return;
    field.focus();
    const dot = initial.lastIndexOf(".");
    field.setSelectionRange(0, dot > 0 ? dot : initial.length);
  }, [initial]);

  const commit = (name: string) => {
    if (done.current) return;
    done.current = true;
    const trimmed = name.trim();
    if (trimmed === "" || trimmed === initial) onCancel();
    else onCommit(trimmed);
  };

  return (
    <div
      style={{ paddingLeft: 8 + depth * INDENT }}
      className="flex w-full items-center gap-1.5 py-0.5 pr-2"
    >
      {icon}
      <input
        ref={input}
        defaultValue={initial}
        spellCheck={false}
        aria-label="Name"
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit(e.currentTarget.value);
          } else if (e.key === "Escape") {
            e.preventDefault();
            done.current = true;
            onCancel();
          }
          // The tree above listens for these, and a name with an `n` in it is
          // not a request for a new file.
          e.stopPropagation();
        }}
        onBlur={(e) => commit(e.currentTarget.value)}
        className="min-w-0 flex-1 rounded-sm bg-neutral-800 px-1 text-xs text-neutral-100 outline outline-1 outline-blue-500"
      />
    </div>
  );
}

/** The contents of one open folder. */
function Children({ path, depth }: { path: string; depth: number }) {
  const ctx = useTree();
  const listing = ctx.tree.listing(path);
  const pad = { paddingLeft: 8 + depth * INDENT };

  // A new file or folder is typed in where it is about to appear, so you can
  // see what you are naming and where it is going.
  const draft =
    ctx.draft?.parent === path ? (
      <NameInput
        key="draft"
        initial=""
        depth={depth}
        icon={ctx.draft.kind === "folder" ? <Chevron open={false} /> : <FileIcon />}
        onCommit={ctx.commitDraft}
        onCancel={ctx.cancelEdit}
      />
    ) : null;

  if (listing === undefined || listing.status === "loading") {
    return (
      <>
        {draft}
        <p style={pad} className="py-1 pr-2 text-xs text-neutral-600">
          loading…
        </p>
      </>
    );
  }
  if (listing.status === "error") {
    return (
      <>
        {draft}
        <p style={pad} className="py-1 pr-2 text-xs leading-snug text-red-400">
          {listing.message}
        </p>
      </>
    );
  }
  if (listing.entries.length === 0) {
    return (
      <>
        {draft}
        {draft === null && (
          <p style={pad} className="py-1 pr-2 text-xs italic text-neutral-600">
            empty
          </p>
        )}
      </>
    );
  }

  return (
    <>
      {draft}
      {listing.entries.map((entry) => (
        <Row key={entry.path} entry={entry} depth={depth} />
      ))}
    </>
  );
}

/** One line of the tree: a file to open, or a folder to expand. */
function Row({ entry, depth }: { entry: Entry; depth: number }) {
  const ctx = useTree();
  const open = entry.isDir && ctx.tree.isExpanded(entry.path);
  const isActive = !entry.isDir && entry.path === ctx.activePath;
  const isSelected = entry.path === ctx.selected?.path;
  const icon = entry.isDir ? <Chevron open={open} /> : <FileIcon />;

  if (ctx.renaming === entry.path) {
    return (
      <NameInput
        initial={entry.name}
        depth={depth}
        icon={icon}
        onCommit={(name) => ctx.commitRename(entry, name)}
        onCancel={ctx.cancelEdit}
      />
    );
  }

  // A drop on a folder goes into it; a drop on a file goes beside that file,
  // which is into the folder holding it.
  const dropInto = entry.isDir ? entry.path : parentOf(entry.path);
  // Only folders are ever lit up, even though a file is a perfectly good thing
  // to aim at. Lighting the file would light every one of its siblings too —
  // they all mean the same destination — and the folder row that means exactly
  // that destination is right there above them.
  const isDropTarget = entry.isDir && dropTargetIs(ctx, entry.path);

  return (
    <>
      <div
        role="treeitem"
        tabIndex={0}
        aria-expanded={entry.isDir ? open : undefined}
        aria-selected={isSelected}
        title={entry.path}
        draggable
        onDragStart={(e) => {
          ctx.setDragging(entry);
          e.dataTransfer.effectAllowed = "move";
          // Set for the platform's sake — a drag with nothing on it is one
          // some browsers refuse to start.
          e.dataTransfer.setData("text/plain", entry.path);
        }}
        onDragEnd={() => {
          ctx.setDragging(null);
          ctx.setDropTarget(null);
        }}
        onDragOver={(e) => {
          if (!canDrop(ctx, dropInto)) {
            // Over a row nothing can land on — the dragged row itself, or a
            // folder inside it. Whatever was lit up a moment ago is not where
            // this would go, so it stops being lit up. The event stops here
            // rather than reaching the panel behind, which would otherwise
            // offer the project folder for a drop the pointer is not over.
            ctx.setDropTarget(null);
            e.stopPropagation();
            return;
          }
          // Both, and the preventDefault on *over* rather than only on *enter*:
          // without it the drop never fires, which is the whole of why this
          // looks like more ceremony than it is.
          e.preventDefault();
          e.stopPropagation();
          e.dataTransfer.dropEffect = "move";
          ctx.setDropTarget(dropInto);
        }}
        onDrop={(e) => {
          if (!canDrop(ctx, dropInto) || ctx.dragging === null || dropInto === null) {
            // A drop the row cannot take is a drop that means nothing, not one
            // for the panel behind it to take as a move to the top level.
            e.stopPropagation();
            return;
          }
          e.preventDefault();
          e.stopPropagation();
          ctx.move(ctx.dragging, dropInto);
          ctx.setDragging(null);
          ctx.setDropTarget(null);
        }}
        onClick={(e) => {
          // The panel below clears the selection, which is what a click on
          // nothing means. A click on a row is not a click on nothing.
          e.stopPropagation();
          ctx.setSelected(entry);
          if (entry.isDir) ctx.tree.toggle(entry.path);
          else ctx.openFile(entry.path);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          ctx.setSelected(entry);
          ctx.openMenu({ x: e.clientX, y: e.clientY, entry });
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            ctx.setSelected(entry);
            if (entry.isDir) ctx.tree.toggle(entry.path);
            else ctx.openFile(entry.path);
          } else if (e.key === "F2") {
            e.preventDefault();
            ctx.startRename(entry.path);
          } else if ((e.key === "Delete" || e.key === "Backspace") && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            ctx.remove(entry);
          }
        }}
        style={{ paddingLeft: 8 + depth * INDENT }}
        className={
          "flex w-full cursor-pointer items-center gap-1.5 py-0.5 pr-2 text-left text-xs outline-none transition-colors " +
          (isDropTarget
            ? "bg-blue-600/30 text-neutral-100"
            : isActive
              ? "bg-neutral-800 text-neutral-100"
              : isSelected
                ? "bg-neutral-800/60 text-neutral-100"
                : "text-neutral-300 hover:bg-neutral-900") +
          (ctx.dragging?.path === entry.path ? " opacity-40" : "") +
          " focus-visible:outline focus-visible:outline-1 focus-visible:-outline-offset-1 focus-visible:outline-blue-500"
        }
      >
        {icon}
        <span className="truncate">{entry.name}</span>
      </div>
      {entry.isDir && open && <Children path={entry.path} depth={depth + 1} />}
    </>
  );
}

/** Whether the folder under the pointer is one the dragged row can land in. */
function canDrop(ctx: Tree, into: string | null): boolean {
  if (ctx.dragging === null || into === null) return false;
  // Into the folder it is already in, which is a move to where it already is.
  if (parentOf(ctx.dragging.path) === into) return false;
  // And into itself, or anywhere below itself, which is not a move at all.
  return !isWithin(into, ctx.dragging.path);
}

function dropTargetIs(ctx: Tree, into: string | null): boolean {
  return into !== null && ctx.dropTarget === into && canDrop(ctx, into);
}

/** A small button in the panel's heading. */
function HeaderButton({
  title,
  onClick,
  children,
}: {
  title: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      aria-label={title}
      className="shrink-0 rounded p-1 text-neutral-500 transition-colors hover:bg-neutral-800 hover:text-neutral-200"
    >
      {children}
    </button>
  );
}

interface ProjectPanelProps {
  /** The project's folder, or null before the backend has named one. */
  root: string | null;
  /**
   * What the project calls itself, from its settings file. Null until that has
   * been read, when the folder's own name stands in for it.
   */
  name: string | null;
  /** Rename the project, which is saved with the rest of its settings. */
  onRename: (name: string) => void;
  /** Bumped to read every open folder again — by the watch on the project's
   *  folder, and by whatever in the app has just changed it. */
  version: number;
  /** The file the editor is showing, so the tree can mark it. */
  activePath: string | null;
  /** Open a file in a tab. */
  onOpenFile: (path: string) => void;
  /** A file or folder has moved, so anything holding its old path — an open
   *  tab, the project's own files — can follow it. */
  onMoved: (from: string, to: string) => void;
  /** A file or folder has gone to the trash. */
  onDeleted: (path: string) => void;
  /** Somewhere for a failure, or a move's broken imports, to be shown. */
  onProblems: (diagnostics: Diagnostic[]) => void;
}

/**
 * Every file in the project, which is simply a folder on disk.
 *
 * It opens on whichever folder was last used and follows whatever File ▸ New
 * Project… or File ▸ Open Project… picks after that. What little a project
 * configures — its name, and the transport it is played at — lives in a
 * `scree-project.json` beside its files; the name is edited here, at the top of
 * the tree, because that is where you read it.
 *
 * The tree is editable, and every edit goes through the backend rather than
 * through anything held here: the panel asks for a move and then asks the
 * folders involved what they contain now. That is slower than patching the
 * listing it already has, and it is the only version that cannot drift from
 * what is actually on the disk — which, for a panel whose whole job is to say
 * what is on the disk, is the only property that matters.
 */
export function ProjectPanel({
  root,
  name,
  onRename,
  version,
  activePath,
  onOpenFile,
  onMoved,
  onDeleted,
  onProblems,
}: ProjectPanelProps) {
  const tree = useProjectTree(root, version);

  const [selected, setSelected] = useState<Selected | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [dragging, setDragging] = useState<Selected | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);

  const fail = useCallback(
    (e: unknown, doing: string) => onProblems([toDiagnostic(e, doing)]),
    [onProblems],
  );

  const cancelEdit = useCallback(() => {
    setRenaming(null);
    setDraft(null);
  }, []);

  /**
   * Where a New File or New Folder goes when it was asked for from the heading
   * rather than from a row: into whatever is selected, or the folder holding
   * it, and into the project itself when nothing is.
   */
  const target = useCallback(
    (entry: Entry | null): string | null => {
      const at: Selected | null = entry ?? selected;
      if (at === null) return root;
      return at.isDir ? at.path : (parentOf(at.path) ?? root);
    },
    [selected, root],
  );

  const startDraft = useCallback(
    (parent: string | null, kind: "file" | "folder") => {
      if (parent === null) return;
      setRenaming(null);
      // Opened before it is typed into, so the row appears where it is going —
      // and so that a new file in a shut folder is not made somewhere nobody
      // can see it happen.
      tree.expand(parent);
      setDraft({ parent, kind });
    },
    [tree],
  );

  const commitDraft = useCallback(
    (typed: string) => {
      if (draft === null) return;
      const { parent, kind } = draft;
      setDraft(null);
      const path = join(parent, typed);

      invoke<string>(kind === "folder" ? "create_dir" : "create_file", { path })
        .then((made) => {
          tree.refresh(parent);
          tree.reveal(made);
          setSelected({ path: made, isDir: kind === "folder" });
          // A new file is one you made in order to write in it.
          if (kind === "file") onOpenFile(made);
        })
        .catch((e) => fail(e, `could not create ${basename(path)}`));
    },
    [draft, tree, onOpenFile, fail],
  );

  /**
   * Move a file or folder, and put everything that pointed at it right.
   *
   * The backend does the part that is not bookkeeping — rewriting the `use`
   * paths in programs that imported it — and hands back the ones it could not.
   * Those go to the problems panel: the move has happened, and a program that
   * will now refuse to run should say so before it is played rather than after.
   */
  const move = useCallback(
    (from: Selected, to: string) => {
      if (from.path === to) return;
      invoke<Moved>("move_path", { from: from.path, to, root })
        .then((moved) => {
          for (const folder of [parentOf(from.path), parentOf(moved.path)]) {
            if (folder !== null) tree.refresh(folder);
          }
          tree.reveal(moved.path);
          setSelected({ path: moved.path, isDir: from.isDir });
          onMoved(from.path, moved.path);
          if (moved.unfollowed.length > 0) {
            onProblems(
              moved.unfollowed.map((u) => ({
                stage: "import" as const,
                message: u.detail,
                line: null,
                column: null,
                snippet: null,
                file: u.file,
              })),
            );
          }
        })
        .catch((e) => fail(e, `could not move ${basename(from.path)}`));
    },
    [root, tree, onMoved, onProblems, fail],
  );

  const commitRename = useCallback(
    (entry: Entry, typed: string) => {
      setRenaming(null);
      const parent = parentOf(entry.path);
      if (parent === null) return;
      // `typed` may carry folders of its own — `lib/drums.scree` renames and
      // moves in one go, which the backend makes the folders for. That is what
      // the name plainly says, and what every other editor does with it.
      move(entry, join(parent, typed));
    },
    [move],
  );

  const moveInto = useCallback(
    (from: Selected, intoDir: string) => move(from, join(intoDir, basename(from.path))),
    [move],
  );

  const remove = useCallback(
    (entry: Entry) => {
      void (async () => {
        const yes = await confirm(
          `Move ${entry.name} to the Trash?` +
            (entry.isDir ? "\n\nEverything inside it goes too." : ""),
          { title: "Delete", kind: "warning", okLabel: "Move to Trash" },
        );
        if (!yes) return;

        try {
          await invoke("delete_path", { path: entry.path });
          const parent = parentOf(entry.path);
          if (parent !== null) tree.refresh(parent);
          if (selected !== null && isWithin(selected.path, entry.path)) setSelected(null);
          onDeleted(entry.path);
        } catch (e) {
          fail(e, `could not delete ${entry.name}`);
        }
      })();
    },
    [tree, selected, onDeleted, fail],
  );

  // A menu is dismissed by whatever happens next, including the Escape that
  // means "not that one after all".
  useEffect(() => {
    if (menu === null) return;
    const close = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", close);
    };
  }, [menu]);

  if (root === null) {
    return (
      <div className="px-4 py-4 text-xs leading-relaxed text-neutral-500">
        <p>
          No project open. Pick a folder with{" "}
          <span className="text-neutral-400">File ▸ Open Project…</span> — or
          start one with <span className="text-neutral-400">New Project…</span>{" "}
          — and its files show up here.
        </p>
      </div>
    );
  }

  const context: Tree = {
    tree,
    activePath,
    selected,
    setSelected,
    openFile: onOpenFile,
    renaming,
    startRename: (path) => {
      setDraft(null);
      setRenaming(path);
    },
    commitRename,
    cancelEdit,
    draft,
    commitDraft,
    remove,
    openMenu: setMenu,
    dragging,
    setDragging,
    dropTarget,
    setDropTarget,
    move: moveInto,
  };

  const menuEntry = menu?.entry ?? null;

  return (
    <TreeContext.Provider value={context}>
      <div className="flex items-center justify-between gap-1 border-b border-neutral-800 px-3 py-1.5">
        {/* An input rather than a label, since the name is the project's own
            rather than the folder's: a folder is named for where it sits on a
            disk, a piece for what it is. Until the settings have been read the
            folder's name stands in, which is what they will say anyway. */}
        <input
          value={name ?? basename(root)}
          onChange={(e) => onRename(e.target.value)}
          // A project with no name is one the tree cannot be read by, and the
          // backend would fill it in on the next save regardless — so it is
          // filled in here, where it can still be seen happening.
          onBlur={() => {
            if ((name ?? "").trim() === "") onRename(basename(root));
          }}
          spellCheck={false}
          aria-label="Project name"
          title={root}
          className="min-w-0 flex-1 truncate rounded bg-transparent px-1 py-0.5 text-xs font-medium tracking-wide text-neutral-300 outline-none transition-colors hover:bg-neutral-800 focus:bg-neutral-800 focus:text-neutral-100"
        />
        <HeaderButton title="New file" onClick={() => startDraft(target(null), "file")}>
          <svg viewBox="0 0 24 24" fill="currentColor" className="h-3.5 w-3.5">
            <path d="M13 2H6v20h12V7l-5-5zm0 1.5L17.5 8H13V3.5zM11 10h2v3h3v2h-3v3h-2v-3H8v-2h3v-3z" />
          </svg>
        </HeaderButton>
        <HeaderButton title="New folder" onClick={() => startDraft(target(null), "folder")}>
          <svg viewBox="0 0 24 24" fill="currentColor" className="h-3.5 w-3.5">
            <path d="M10 4H2v16h20V6H12l-2-2zm1 6h2v3h3v2h-3v3h-2v-3H8v-2h3v-3z" />
          </svg>
        </HeaderButton>
        {/* There is no refresh button: the project's folder is watched, and a
            change made anywhere else reaches the tree on its own. See
            `watcher.rs`. */}
      </div>

      {/* The tree, and below it the rest of the panel — which is the project
          folder as a drop target, so dragging a file out of a folder and back
          to the top level does not need a row to aim at. */}
      <div
        role="tree"
        aria-label="Project files"
        className={
          "min-h-0 flex-1 py-1 " + (dropTargetIs(context, root) ? "bg-blue-600/10" : "")
        }
        onClick={() => setSelected(null)}
        onContextMenu={(e) => {
          e.preventDefault();
          setSelected(null);
          setMenu({ x: e.clientX, y: e.clientY, entry: null });
        }}
        onDragOver={(e) => {
          if (!canDrop(context, root)) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = "move";
          setDropTarget(root);
        }}
        onDrop={(e) => {
          if (!canDrop(context, root) || dragging === null) return;
          e.preventDefault();
          moveInto(dragging, root);
          setDragging(null);
          setDropTarget(null);
        }}
      >
        <Children path={root} depth={0} />
      </div>

      {menu && (
        <>
          {/* Catches the click that dismisses the menu, wherever it lands, so
              no part of the app has to know a menu might be open. */}
          <div
            className="fixed inset-0 z-40"
            onClick={() => setMenu(null)}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu(null);
            }}
          />
          <div
            role="menu"
            style={{ left: menu.x, top: menu.y }}
            className="fixed z-50 min-w-40 rounded-md border border-neutral-700 bg-neutral-900 py-1 text-xs shadow-lg shadow-black/40"
          >
            <MenuItem
              label="New File"
              onClick={() => {
                setMenu(null);
                startDraft(target(menuEntry), "file");
              }}
            />
            <MenuItem
              label="New Folder"
              onClick={() => {
                setMenu(null);
                startDraft(target(menuEntry), "folder");
              }}
            />
            {menuEntry && (
              <>
                <div className="my-1 border-t border-neutral-800" />
                <MenuItem
                  label="Rename"
                  hint="F2"
                  onClick={() => {
                    setMenu(null);
                    setDraft(null);
                    setRenaming(menuEntry.path);
                  }}
                />
                <MenuItem
                  label="Delete"
                  hint="⌘⌫"
                  onClick={() => {
                    setMenu(null);
                    remove(menuEntry);
                  }}
                />
              </>
            )}
          </div>
        </>
      )}
    </TreeContext.Provider>
  );
}

function MenuItem({
  label,
  hint,
  onClick,
}: {
  label: string;
  hint?: string;
  onClick: () => void;
}) {
  return (
    <button
      role="menuitem"
      onClick={onClick}
      className="flex w-full items-center justify-between gap-6 px-3 py-1 text-left text-neutral-300 transition-colors hover:bg-neutral-800 hover:text-neutral-100"
    >
      <span>{label}</span>
      {hint && <span className="font-mono text-[10px] text-neutral-500">{hint}</span>}
    </button>
  );
}
