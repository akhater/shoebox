import {
  Aperture,
  Camera,
  ChevronRight,
  CircleArrowUp,
  Ellipsis,
  Folder,
  FolderPlus,
  FolderX,
  HardDrive,
  Images,
  PanelLeftClose,
  PanelLeftOpen,
  RefreshCw,
  Settings,
  Trash2,
  TriangleAlert,
  Users,
} from "lucide-react";
import {
  useEffect,
  useRef,
  useState,
  type FocusEvent as ReactFocusEvent,
  type HTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import {
  fmtAgo,
  fmtNum,
  sameScope,
  type Browse,
  type FolderNode,
  type GearItem,
  type GearKind,
  type Overview,
  type RootInfo,
  type Scope,
  type VolumeInfo,
} from "../api";
import boxLogo from "../assets/shoebox-box.png";

interface Props {
  overview: Overview | null;
  browse: Browse | null;
  scope: Scope;
  scanningRoot: number | null;
  version: string;
  /** A newer published release ("26.43.1"), or null. */
  update: string | null;
  /** Opens its release page. */
  onUpdate: () => void;
  /** People shown in the People view. */
  people: number;
  /** Of those, the ones new or found in new photos since you last looked. */
  peopleNew: number;
  peopleOpen: boolean;
  /** `newOnly`: open People with "Only new" on (the yellow number). */
  onPeople: (newOnly?: boolean) => void;
  onScope: (s: Scope) => void;
  onAddFolder: () => void;
  onRescan: (rootId: number) => void;
  onRegenerate: (rootId: number) => void;
  onRemove: (root: RootInfo) => void;
  onRename: (volumeId: number, label: string) => void;
  /** Renames a folder on its disk and in the catalog. */
  onRenameFolder: (volumeId: number, relPath: string, name: string) => Promise<void>;
  /** Makes "New folder" inside a folder; its path, or null if it failed. */
  onNewFolder: (volumeId: number, parentRel: string) => Promise<string | null>;
  onShowFolder: (volumeId: number, relPath: string) => void;
  /** Drag and drop: `folder` goes inside `into`. */
  onMoveFolder: (folder: FolderTarget, into: FolderTarget) => void;
  onSettings: () => void;
  /** Folded to a rail of icons (Ctrl+B). */
  folded: boolean;
  onFold: (folded: boolean) => void;
  /** Gear: renames a camera or lens (onto another's name = merge). */
  onGearRename: (kind: GearKind, name: string, to: string) => void;
  /** Gear: back to the names in its files, kept apart. */
  onGearSplit: (kind: GearKind, name: string) => void;
}

/** A camera or lens row's menu: where it was asked for. */
interface GearMenuAt {
  kind: GearKind;
  item: GearItem;
  x: number;
  y: number;
}

/** A folder in Sources: a source (catalog root) or a folder inside one. */
export interface FolderTarget {
  v: VolumeInfo;
  relPath: string;
  name: string;
  rootId: number;
  /** Set when the folder is a catalog root. */
  root?: RootInfo;
  /** Its key in the open set, to open it when a folder is made inside. */
  key: string;
  /** Full path, as shown. */
  path: string;
  /** Photos in it and below. */
  count: number;
}

/** What folder rows share: the right-click menu, renaming in place, drag and drop. */
interface FolderUi {
  menuFor: (t: FolderTarget, at: { x: number; y: number }) => void;
  renaming: string | null;
  startRename: (t: FolderTarget) => void;
  endRename: (t: FolderTarget, name: string | null) => void;
  /** The row's drag-and-drop handlers. */
  dnd: (t: FolderTarget) => DragProps;
  /** The folder being dragged over, as `folderId`. */
  dropOn: string | null;
}

type DragProps = Pick<
  HTMLAttributes<HTMLElement>,
  "draggable" | "onDragStart" | "onDragEnd" | "onDragOver" | "onDragLeave" | "onDrop"
>;

const folderId = (vid: number, rel: string) => `${vid}:${rel}`;
const parentOf = (rel: string) => (rel.includes("\\") ? rel.slice(0, rel.lastIndexOf("\\")) : "");

/** Can `folder` be dropped into `into`? Not into itself, inside itself, or where it already is.
 *  Sources themselves don't move (they'd merge into another source). */
function canDrop(folder: FolderTarget, into: FolderTarget) {
  if (folder.root || !folder.v.online || !into.v.online || (into.root && !into.root.found)) return false;
  if (folder.v.id !== into.v.id) return true;
  const inside = into.relPath === folder.relPath || into.relPath.startsWith(folder.relPath + "\\");
  return !inside && parentOf(folder.relPath) !== into.relPath;
}

/** Opens the folder menu from the keyboard (Menu key, Shift+F10) or starts a rename (F2). */
function folderKeys(e: ReactKeyboardEvent<HTMLElement>, t: FolderTarget, f: FolderUi) {
  if ((e.target as HTMLElement).tagName === "INPUT") return;
  if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
    const r = e.currentTarget.getBoundingClientRect();
    f.menuFor(t, { x: r.left + 24, y: r.bottom });
  } else if (e.key === "F2") {
    f.startRename(t);
  } else return;
  e.preventDefault();
  e.stopPropagation();
}

/** A whole disk added as a source goes by the disk's name; renaming it renames the disk (in Shoebox only). */
const shownName = (t: FolderTarget) => (t.relPath ? t.name : t.v.label);

/** The folder's name, or a box to type its new name. */
function FolderName({ t, f }: { t: FolderTarget; f: FolderUi }) {
  if (f.renaming !== folderId(t.v.id, t.relPath)) return <span className="grow ellipsis">{shownName(t)}</span>;
  return (
    <input
      className="rename grow"
      autoFocus
      defaultValue={shownName(t)}
      onFocus={(e) => e.target.select()}
      onClick={(e) => e.stopPropagation()}
      onBlur={(e) => f.endRename(t, e.target.value)}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") e.currentTarget.blur();
        if (e.key === "Escape") f.endRename(t, null);
      }}
    />
  );
}

/** Which sections and tree nodes are open, remembered between sessions. */
function useOpenSet(key: string, initial: string[]) {
  const [open, setOpen] = useState<Set<string>>(() => {
    try {
      const saved = localStorage.getItem(key);
      return new Set(saved ? (JSON.parse(saved) as string[]) : initial);
    } catch {
      return new Set(initial);
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(key, JSON.stringify([...open]));
    } catch {
      /* per-viewer convenience only */
    }
  }, [key, open]);
  const toggle = (id: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  return [open, toggle] as const;
}

/** A row the keyboard can reach: its depth in the tree and, if it opens, its key. */
const nav = (depth: number, key?: string) => ({ "data-nav": "", "data-depth": depth, "data-key": key, tabIndex: -1 });

export function Sidebar(p: Props) {
  const { overview, scope, onScope } = p;
  const [open, toggle] = useOpenSet("shoebox.sidebar", ["sec:disks"]);
  const [menu, setMenu] = useState<{ t: FolderTarget; x: number; y: number } | null>(null);
  const [gearMenu, setGearMenu] = useState<GearMenuAt | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  // Read synchronously: a blur right after Escape must not rename.
  const renamingNow = useRef(renaming);
  renamingNow.current = renaming;
  // Where focus goes back when the menu closes.
  const opener = useRef<HTMLElement | null>(null);
  // Drag and drop: the folder being dragged, the one under it, and a timer
  // that opens a closed folder after a moment over it (as Explorer does).
  const dragged = useRef<FolderTarget | null>(null);
  const [dropOn, setDropOn] = useState<string | null>(null);
  const opening = useRef<{ key: string; timer: number } | null>(null);
  const stopOpening = () => {
    if (opening.current) window.clearTimeout(opening.current.timer);
    opening.current = null;
  };

  const dnd = (t: FolderTarget): DragProps => ({
    draggable: !t.root && t.v.online && renaming !== folderId(t.v.id, t.relPath),
    onDragStart: (e) => {
      dragged.current = t;
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", t.path);
      setMenu(null);
    },
    onDragEnd: () => {
      dragged.current = null;
      setDropOn(null);
      stopOpening();
    },
    onDragOver: (e) => {
      const src = dragged.current;
      if (!src || !canDrop(src, t)) return; // not a place it can go: no drop
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      setDropOn(folderId(t.v.id, t.relPath));
      if (!open.has(t.key) && opening.current?.key !== t.key) {
        stopOpening();
        opening.current = { key: t.key, timer: window.setTimeout(() => open.has(t.key) || toggle(t.key), 700) };
      }
    },
    onDragLeave: (e) => {
      if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
      setDropOn((d) => (d === folderId(t.v.id, t.relPath) ? null : d));
      if (opening.current?.key === t.key) stopOpening();
    },
    onDrop: (e) => {
      e.preventDefault();
      const src = dragged.current;
      dragged.current = null;
      setDropOn(null);
      stopOpening();
      if (src && canDrop(src, t)) p.onMoveFolder(src, t);
    },
  });

  const f: FolderUi = {
    menuFor: (t, at) => {
      opener.current = document.activeElement as HTMLElement | null;
      setMenu({ t, ...at });
    },
    renaming,
    startRename: (t) => setRenaming(folderId(t.v.id, t.relPath)),
    endRename: (t, name) => {
      if (renamingNow.current !== folderId(t.v.id, t.relPath)) return;
      renamingNow.current = null;
      setRenaming(null);
      const n = name?.trim();
      if (!n || n === shownName(t)) return;
      if (t.relPath) p.onRenameFolder(t.v.id, t.relPath, n);
      else p.onRename(t.v.id, n);
    },
    dnd,
    dropOn,
  };

  const newFolder = async (t: FolderTarget) => {
    const rel = await p.onNewFolder(t.v.id, t.relPath);
    if (rel == null) return;
    if (!open.has(t.key)) toggle(t.key);
    // Name it straight away, as Explorer does; the row appears with the next refresh.
    setRenaming(folderId(t.v.id, rel));
  };

  // One Tab stop for the whole sidebar: the row last focused, else the one shown, else the first.
  useEffect(() => {
    const el = scroller.current;
    if (!el || el.querySelector('[data-nav][tabindex="0"]')) return;
    (el.querySelector("[data-nav].active") ?? el.querySelector("[data-nav]"))?.setAttribute("tabindex", "0");
  });

  const onFocus = (e: ReactFocusEvent) => {
    const row = (e.target as HTMLElement).closest("[data-nav]");
    if (!row) return;
    scroller.current?.querySelectorAll('[data-nav][tabindex="0"]').forEach((r) => r.setAttribute("tabindex", "-1"));
    row.setAttribute("tabindex", "0");
  };

  // Tree keys: Up/Down move, Right opens (or steps in), Left closes (or goes
  // to the parent), Enter/Space shows, Home/End.
  const onKey = (e: ReactKeyboardEvent) => {
    const target = e.target as HTMLElement;
    const cur = target.closest<HTMLElement>("[data-nav]");
    if (!cur || target.tagName === "INPUT" || !scroller.current) return;
    const rows = [...scroller.current.querySelectorAll<HTMLElement>("[data-nav]")];
    const i = rows.indexOf(cur);
    const depth = Number(cur.dataset.depth);
    const key = cur.dataset.key;
    const go = (row?: HTMLElement) => {
      row?.focus();
      row?.scrollIntoView({ block: "nearest" });
    };
    switch (e.key) {
      case "ArrowDown":
        go(rows[i + 1]);
        break;
      case "ArrowUp":
        go(rows[i - 1]);
        break;
      case "Home":
        go(rows[0]);
        break;
      case "End":
        go(rows[rows.length - 1]);
        break;
      case "ArrowRight":
        if (key && !open.has(key)) toggle(key);
        else if (key && Number(rows[i + 1]?.dataset.depth) > depth) go(rows[i + 1]);
        break;
      case "ArrowLeft":
        if (key && open.has(key)) toggle(key);
        else go(rows.slice(0, i).reverse().find((r) => Number(r.dataset.depth) < depth));
        break;
      case "Enter":
      case " ":
        if (target.tagName === "BUTTON") return; // buttons click themselves
        cur.click();
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  // The fold button swaps for its twin in the other state: keep the keyboard on it.
  const refold = useRef(false);
  const fold = (on: boolean) => {
    refold.current = true;
    p.onFold(on);
  };
  useEffect(() => {
    if (!refold.current) return;
    refold.current = false;
    document.querySelector<HTMLElement>(".sidebar .fold-btn")?.focus();
  }, [p.folded]);

  const anyMenu = menu != null || gearMenu != null;
  useEffect(() => {
    if (!anyMenu) return;
    const close = () => {
      setMenu(null);
      setGearMenu(null);
    };
    window.addEventListener("click", close);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  }, [anyMenu]);

  const cls = (s: Scope, extra = "") => `nav-item ${!p.peopleOpen && sameScope(s, scope) ? "active" : ""} ${extra}`;

  // Every folder added, by path, whatever disk it is on.
  const sources = (overview?.volumes ?? [])
    .flatMap((v) => v.roots.map((r) => ({ v, r })))
    .sort((a, b) => a.r.path.localeCompare(b.r.path, undefined, { numeric: true, sensitivity: "base" }));

  if (p.folded) {
    // Sources and Gear unfold the sidebar on their section; lit when what is shown comes from them.
    const unfold = (section: string) => {
      if (!open.has(section)) toggle(section);
      p.onFold(false);
    };
    const from = p.peopleOpen ? null : scope.kind;
    const inSources = from === "root" || from === "folder" || from === "volume";
    const inGear = from === "camera" || from === "lens";
    return (
      <nav className="sidebar folded">
        <div className="brand" title="Shoebox - every photo, every disk">
          <img className="brand-mark" src={boxLogo} alt="Shoebox" />
        </div>
        <div className="sidebar-scroll" ref={scroller} onKeyDown={onKey} onFocus={onFocus}>
          <div className="nav-group">
            <RailItem
              label={`All photos · ${fmtNum(overview?.total ?? 0)}`}
              className={cls({ kind: "all" })}
              onClick={() => onScope({ kind: "all" })}
            >
              <Images size={16} />
            </RailItem>
            <RailItem
              label={`People · ${fmtNum(p.people)}${
                p.peopleNew > 0 ? ` · ${fmtNum(p.peopleNew)} new or found in new photos` : ""
              }`}
              className={`nav-item ${p.peopleOpen ? "active" : ""}`}
              onClick={p.onPeople}
            >
              <Users size={16} />
              {p.peopleNew > 0 && <span className="nav-badge">{p.peopleNew > 99 ? "99+" : p.peopleNew}</span>}
            </RailItem>
            {(overview?.missing ?? 0) > 0 && (
              <RailItem
                label={`Missing · ${fmtNum(overview!.missing)}`}
                className={cls({ kind: "missing" })}
                onClick={() => onScope({ kind: "missing" })}
              >
                <TriangleAlert size={16} className="missing-icon" />
              </RailItem>
            )}
            {((overview?.marked ?? 0) > 0 || scope.kind === "marked") && (
              <RailItem
                label={`Marked for deletion · ${fmtNum(overview?.marked ?? 0)}`}
                className={cls({ kind: "marked" })}
                onClick={() => onScope({ kind: "marked" })}
              >
                <Trash2 size={16} className="marked-icon" />
              </RailItem>
            )}
          </div>
          <div className="rail-sep" />
          <div className="nav-group">
            <RailItem
              label="Sources: unfold to browse them"
              className={`nav-item ${inSources ? "active" : ""}`}
              onClick={() => unfold("sec:disks")}
            >
              <HardDrive size={16} />
            </RailItem>
            <RailItem
              label="Gear: unfold to browse cameras and lenses"
              className={`nav-item ${inGear ? "active" : ""}`}
              onClick={() => unfold("sec:gear")}
            >
              <Camera size={16} />
            </RailItem>
          </div>
        </div>
        <div className="sidebar-foot">
          {p.update && (
            <button
              className="nav-item rail-item update"
              onClick={p.onUpdate}
              title={`Shoebox ${p.update} is out: open its release page`}
            >
              <CircleArrowUp size={16} />
            </button>
          )}
          <button className="nav-item rail-item" onClick={p.onAddFolder} title="Add folder">
            <FolderPlus size={16} />
          </button>
          <button
            className="nav-item rail-item"
            onClick={p.onSettings}
            title={`Settings${p.version ? ` · v${p.version}` : ""}`}
          >
            <Settings size={16} />
          </button>
          <FoldButton folded onFold={fold} />
        </div>
      </nav>
    );
  }

  return (
    <nav className="sidebar">
      <div className="brand">
        <div className="brand-name">
          <span className="logo">Shoebox</span>
          <span className="tagline">Every photo, every disk</span>
        </div>
        <img className="brand-mark" src={boxLogo} alt="" />
      </div>

      {menu && (
        <FolderMenu
          {...menu}
          p={p}
          onClose={() => {
            setMenu(null);
            opener.current?.focus();
          }}
          onNewFolder={newFolder}
          onRename={(t) => f.startRename(t)}
        />
      )}

      {gearMenu && (
        <GearMenu
          {...gearMenu}
          others={(gearMenu.kind === "camera" ? p.browse?.gear.cameras : p.browse?.gear.lenses) ?? []}
          onClose={() => {
            setGearMenu(null);
            opener.current?.focus();
          }}
          onRename={() => setRenaming(gearId(gearMenu.kind, gearMenu.item.name))}
          onMerge={(into) => p.onGearRename(gearMenu.kind, gearMenu.item.name, into)}
          onSplit={() => p.onGearSplit(gearMenu.kind, gearMenu.item.name)}
        />
      )}

      <div
        className="sidebar-scroll"
        ref={scroller}
        onKeyDown={onKey}
        onFocus={onFocus}
        onScroll={() => {
          setMenu(null);
          setGearMenu(null);
        }}
      >
        <div className="nav-group">
          <button {...nav(0)} className={cls({ kind: "all" })} onClick={() => onScope({ kind: "all" })}>
            <Images size={16} />
            <span className="grow">All photos</span>
            <span className="count">{fmtNum(overview?.total ?? 0)}</span>
          </button>
          <button {...nav(0)} className={`nav-item ${p.peopleOpen ? "active" : ""}`} onClick={() => p.onPeople()}>
            <Users size={16} />
            <span className="grow">People</span>
            {p.peopleNew > 0 && (
              <span
                className="nav-badge"
                title={`${fmtNum(p.peopleNew)} ${p.peopleNew === 1 ? "person" : "people"} new or found in new photos: click to see only them`}
                onClick={(e) => {
                  e.stopPropagation();
                  p.onPeople(true);
                }}
              >
                {fmtNum(p.peopleNew)}
              </span>
            )}
            <span className="count">{fmtNum(p.people)}</span>
          </button>
          {(overview?.missing ?? 0) > 0 && (
            <button {...nav(0)} className={cls({ kind: "missing" })} onClick={() => onScope({ kind: "missing" })}>
              <TriangleAlert size={16} className="missing-icon" />
              <span className="grow">Missing</span>
              <span className="count">{fmtNum(overview!.missing)}</span>
            </button>
          )}
          {((overview?.marked ?? 0) > 0 || scope.kind === "marked") && (
            <button {...nav(0)} className={cls({ kind: "marked" })} onClick={() => onScope({ kind: "marked" })}>
              <Trash2 size={16} className="marked-icon" />
              <span className="grow">Marked for deletion</span>
              <span className="count">{fmtNum(overview?.marked ?? 0)}</span>
            </button>
          )}
        </div>

        {/* Dates and cameras moved into the filter bar above the grid. */}
        <Section
          id="sec:disks"
          title="Sources"
          open={open}
          toggle={toggle}
          action={
            <button tabIndex={-1} className="icon-btn" onClick={p.onAddFolder} title="Add a folder (or a whole disk)">
              <FolderPlus size={15} />
            </button>
          }
        >
          {sources.map(({ v, r }) => (
            <Source
              key={r.id}
              v={v}
              r={r}
              sameName={sources.filter((x) => x.r.name.toLowerCase() === r.name.toLowerCase()).length > 1}
              p={p}
              open={open}
              toggle={toggle}
              f={f}
              cls={cls}
            />
          ))}
          {overview && sources.length === 0 && <p className="muted small pad">No sources yet.</p>}
        </Section>

        <Section id="sec:gear" title="Gear" open={open} toggle={toggle}>
          {(["camera", "lens"] as const).map((kind) => (
            <GearGroup
              key={kind}
              kind={kind}
              items={(kind === "camera" ? p.browse?.gear.cameras : p.browse?.gear.lenses) ?? []}
              open={open}
              toggle={toggle}
              cls={cls}
              onScope={onScope}
              renaming={renaming}
              onMenu={(item, at) => {
                opener.current = document.activeElement as HTMLElement | null;
                setMenu(null);
                setGearMenu({ kind, item, ...at });
              }}
              onRename={(item) => setRenaming(gearId(kind, item.name))}
              onRenamed={(item, to) => {
                if (renamingNow.current !== gearId(kind, item.name)) return;
                renamingNow.current = null;
                setRenaming(null);
                const n = to?.trim();
                if (n && n !== item.name) p.onGearRename(kind, item.name, n);
              }}
            />
          ))}
        </Section>
      </div>

      <div className="sidebar-foot">
        {p.update && (
          <button className="nav-item update" onClick={p.onUpdate} title="Open its release page to download it">
            <CircleArrowUp size={16} />
            <span className="grow">Shoebox {p.update} is out</span>
          </button>
        )}
        <button className="nav-item" onClick={p.onAddFolder}>
          <FolderPlus size={16} />
          <span className="grow">Add folder</span>
        </button>
        <div className="foot-row">
          <button className="nav-item" onClick={p.onSettings}>
            <Settings size={16} />
            <span className="grow">Settings</span>
            <span className="version">{p.version && `v${p.version}`}</span>
          </button>
          <FoldButton folded={false} onFold={fold} />
        </div>
      </div>
    </nav>
  );
}

/** A row of the folded sidebar: an icon, its name and count as the tooltip. */
function RailItem({
  label,
  className,
  onClick,
  children,
}: {
  label: string;
  className: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button {...nav(0)} className={`${className} rail-item`} title={label} aria-label={label} onClick={onClick}>
      {children}
    </button>
  );
}

/** Folds the sidebar to a rail of icons, or unfolds it. */
function FoldButton({ folded, onFold }: { folded: boolean; onFold: (folded: boolean) => void }) {
  const Icon = folded ? PanelLeftOpen : PanelLeftClose;
  const label = `${folded ? "Unfold" : "Fold"} the sidebar (Ctrl+B)`;
  return (
    <button
      className={folded ? "nav-item rail-item fold-btn" : "icon-btn fold-btn"}
      onClick={() => onFold(!folded)}
      title={label}
      aria-label={label}
    >
      <Icon size={16} />
    </button>
  );
}

function Section({
  id,
  title,
  open,
  toggle,
  action,
  children,
}: {
  id: string;
  title: string;
  open: Set<string>;
  toggle: (id: string) => void;
  action?: ReactNode;
  children: ReactNode;
}) {
  const isOpen = open.has(id);
  return (
    <div className="section">
      <div {...nav(0, id)} className="nav-title" onClick={() => toggle(id)}>
        <ChevronRight size={12} className={`chev ${isOpen ? "open" : ""}`} />
        <span className="grow">{title}</span>
        <span onClick={(e) => e.stopPropagation()}>{action}</span>
      </div>
      {isOpen && <div className="nav-group">{children}</div>}
    </div>
  );
}

function Twisty({ open, onClick, hidden }: { open: boolean; onClick: () => void; hidden?: boolean }) {
  if (hidden) return <span className="twisty-space" />;
  return (
    <span
      className="twisty"
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
    >
      <ChevronRight size={12} className={`chev ${open ? "open" : ""}`} />
    </span>
  );
}

/** The folder menu: right-click on a folder, its "…" button, or the Menu key. */
function FolderMenu({
  t,
  x,
  y,
  p,
  onClose,
  onNewFolder,
  onRename,
}: {
  t: FolderTarget;
  x: number;
  y: number;
  p: Props;
  onClose: () => void;
  onNewFolder: (t: FolderTarget) => void;
  onRename: (t: FolderTarget) => void;
}) {
  const here = t.v.online && (!t.root || t.root.found);
  const busy = p.scanningRoot === t.rootId;
  const act = (fn: () => void) => () => {
    onClose();
    fn();
  };
  return (
    <div
      className="menu folder-menu"
      style={{ left: Math.min(x, window.innerWidth - 220), top: Math.min(y, window.innerHeight - 260) }}
      ref={(el) => {
        if (el && !el.contains(document.activeElement)) el.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
      }}
      onClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.preventDefault()}
      onKeyDown={(e) => {
        e.stopPropagation();
        const items = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
        const i = items.indexOf(e.target as HTMLButtonElement);
        if (e.key === "Escape" || e.key === "Tab") {
          e.preventDefault();
          onClose();
        } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
        }
      }}
    >
      <button disabled={!here} onClick={act(() => p.onShowFolder(t.v.id, t.relPath))}>
        Show in Explorer
      </button>
      <hr />
      <button disabled={!here} onClick={act(() => onNewFolder(t))}>
        New folder
      </button>
      <button
        disabled={t.relPath ? !here : false}
        onClick={act(() => onRename(t))}
        title={t.relPath ? "Renames the folder on the disk" : "The disk's name in Shoebox; the disk itself is not changed"}
      >
        {t.relPath ? "Rename" : "Rename disk"} <span className="shortcut">F2</span>
      </button>
      <hr />
      <button disabled={!t.v.online || busy} onClick={act(() => p.onRescan(t.rootId))}>
        Scan again
      </button>
      {t.root && (
        <>
          <button disabled={!t.v.online} onClick={act(() => p.onRegenerate(t.rootId))}>
            Rebuild previews
          </button>
          <hr />
          <button className="danger" onClick={act(() => p.onRemove(t.root!))}>
            Remove from catalog…
          </button>
        </>
      )}
    </div>
  );
}

/** A source: a folder (or a whole disk) added to the catalog, top of its own tree. */
function Source({
  v,
  r,
  sameName,
  p,
  open,
  toggle,
  f,
  cls,
}: {
  v: VolumeInfo;
  r: RootInfo;
  /** Another source has the same name (Photos on two disks): show the disk too. */
  sameName: boolean;
  p: Props;
  open: Set<string>;
  toggle: (id: string) => void;
  f: FolderUi;
  cls: (s: Scope, extra?: string) => string;
}) {
  const s: Scope = { kind: "root", id: r.id };
  const key = `r:${r.id}`;
  const busy = p.scanningRoot === r.id;
  const lost = v.online && !r.found;
  const tree = p.browse?.folders.find((x) => x.rootId === r.id)?.children ?? [];
  const t: FolderTarget = { v, relPath: r.relPath, name: r.name, rootId: r.id, root: r, key, path: r.path, count: r.photos };
  const where = v.online ? `on ${v.label}, plugged in` : `on ${v.label}, unplugged - browsing Shoebox's copies`;
  return (
    <div className={v.online ? "" : "is-offline"}>
      <div
        {...nav(1, tree.length ? key : undefined)}
        {...f.dnd(t)}
        className={cls(s, `${lost ? "not-found" : ""} ${f.dropOn === folderId(v.id, r.relPath) ? "drop-target" : ""}`)}
        onClick={(e) => {
          e.currentTarget.focus(); // the keyboard follows the click, even on a draggable row
          p.onScope(s);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          f.menuFor(t, { x: e.clientX, y: e.clientY });
        }}
        onKeyDown={(e) => folderKeys(e, t, f)}
        title={
          lost
            ? `Folder not found at ${r.path}\nIts photos stay in the catalog, shown from Shoebox's copies.`
            : `${r.path}\n${where}\n${fmtAgo(r.lastScan)}`
        }
      >
        <Twisty open={open.has(key)} onClick={() => toggle(key)} hidden={!tree.length} />
        {lost ? (
          <FolderX size={15} className="lost-icon" />
        ) : r.relPath ? (
          <Folder size={15} />
        ) : (
          <HardDrive size={15} />
        )}
        <FolderName t={t} f={f} />
        {sameName && r.relPath && <span className="muted small ellipsis source-disk">{v.label}</span>}
        <span className={`dot ${v.online ? "on" : "off"}`} />
        <button
          tabIndex={-1}
          className={`icon-btn tiny ${busy ? "spin" : ""}`}
          disabled={!v.online || busy}
          onClick={(e) => {
            e.stopPropagation();
            p.onRescan(r.id);
          }}
          title={v.online ? "Scan again" : "Plug the disk in to scan"}
        >
          <RefreshCw size={12} />
        </button>
        <button
          tabIndex={-1}
          className="icon-btn tiny"
          onClick={(e) => {
            e.stopPropagation();
            const b = e.currentTarget.getBoundingClientRect();
            f.menuFor(t, { x: b.left, y: b.bottom + 4 });
          }}
          title="More"
        >
          <Ellipsis size={12} />
        </button>
        <span className="count">{fmtNum(r.photos)}</span>
      </div>
      {open.has(key) &&
        tree.map((n) => (
          <Tree key={n.relPath} n={n} depth={1} root={r.id} v={v} p={p} open={open} toggle={toggle} f={f} cls={cls} />
        ))}
    </div>
  );
}

function Tree({
  n,
  depth,
  root,
  v,
  p,
  open,
  toggle,
  f,
  cls,
}: {
  n: FolderNode;
  depth: number;
  root: number;
  v: VolumeInfo;
  p: Props;
  open: Set<string>;
  toggle: (id: string) => void;
  f: FolderUi;
  cls: (s: Scope, extra?: string) => string;
}) {
  const s: Scope = { kind: "folder", volumeId: n.volumeId, relPath: n.relPath };
  const key = `f:${n.volumeId}:${n.relPath}`;
  const mount = (v.mount ?? v.lastMount ?? "?:\\").replace(/\\$/, "");
  const t: FolderTarget = {
    v,
    relPath: n.relPath,
    name: n.name,
    rootId: root,
    key,
    path: `${mount}\\${n.relPath}`,
    count: n.count,
  };
  return (
    <>
      <div
        {...nav(depth + 1, n.children.length ? key : undefined)}
        {...f.dnd(t)}
        className={cls(s, f.dropOn === folderId(v.id, n.relPath) ? "drop-target" : "")}
        style={{ paddingLeft: 10 + depth * 14 }}
        onClick={(e) => {
          e.currentTarget.focus(); // the keyboard follows the click, even on a draggable row
          p.onScope(s);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          f.menuFor(t, { x: e.clientX, y: e.clientY });
        }}
        onKeyDown={(e) => folderKeys(e, t, f)}
        title={n.relPath}
      >
        <Twisty open={open.has(key)} onClick={() => toggle(key)} hidden={!n.children.length} />
        <Folder size={13} />
        <FolderName t={t} f={f} />
        <span className="count">{fmtNum(n.count)}</span>
      </div>
      {open.has(key) &&
        n.children.map((c) => (
          <Tree key={c.relPath} n={c} depth={depth + 1} root={root} v={v} p={p} open={open} toggle={toggle} f={f} cls={cls} />
        ))}
    </>
  );
}

const gearId = (kind: GearKind, name: string) => `gear:${kind}:${name}`;

/** Cameras or lenses, most used first: click one to see its photos. One name
 *  per camera or lens, however its files spell it; rename and merge from its menu. */
function GearGroup({
  kind,
  items,
  open,
  toggle,
  cls,
  onScope,
  renaming,
  onMenu,
  onRename,
  onRenamed,
}: {
  kind: GearKind;
  items: GearItem[];
  open: Set<string>;
  toggle: (id: string) => void;
  cls: (s: Scope, extra?: string) => string;
  onScope: (s: Scope) => void;
  renaming: string | null;
  onMenu: (item: GearItem, at: { x: number; y: number }) => void;
  onRename: (item: GearItem) => void;
  onRenamed: (item: GearItem, to: string | null) => void;
}) {
  const key = `g:${kind}`;
  const Icon = kind === "camera" ? Camera : Aperture;
  if (!items.length) return null;
  return (
    <>
      <div {...nav(1, key)} className="nav-item" onClick={() => toggle(key)}>
        <Twisty open={open.has(key)} onClick={() => toggle(key)} />
        <Icon size={15} />
        <span className="grow">{kind === "camera" ? "Cameras" : "Lenses"}</span>
        <span className="count">{fmtNum(items.filter((i) => !i.unknown).length)}</span>
      </div>
      {open.has(key) &&
        items.map((item) => {
          const s: Scope = { kind, name: item.name };
          const editing = renaming === gearId(kind, item.name);
          const menuAt = (x: number, y: number) => !item.unknown && onMenu(item, { x, y });
          return (
            <div
              key={item.name}
              {...nav(2)}
              className={cls(s, `gear-item ${item.unknown ? "unknown" : ""}`)}
              style={{ paddingLeft: 38 }}
              onClick={(e) => {
                e.currentTarget.focus();
                onScope(s);
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                menuAt(e.clientX, e.clientY);
              }}
              onKeyDown={(e) => {
                if (editing || item.unknown) return;
                if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
                  const r = e.currentTarget.getBoundingClientRect();
                  menuAt(r.left + 24, r.bottom);
                } else if (e.key === "F2") {
                  onRename(item);
                } else return;
                e.preventDefault();
                e.stopPropagation();
              }}
              title={
                item.unknown
                  ? `Photos whose files don't name their ${kind}`
                  : item.inFiles.length
                    ? `${item.name}\nIn the files: ${item.inFiles.join(" · ")}`
                    : item.name
              }
            >
              {editing ? (
                <input
                  className="rename grow"
                  autoFocus
                  defaultValue={item.name}
                  title={`Type another ${kind}'s name to merge the two`}
                  onFocus={(e) => e.target.select()}
                  onClick={(e) => e.stopPropagation()}
                  onBlur={(e) => onRenamed(item, e.target.value)}
                  onKeyDown={(e) => {
                    e.stopPropagation();
                    if (e.key === "Enter") e.currentTarget.blur();
                    if (e.key === "Escape") onRenamed(item, null);
                  }}
                />
              ) : (
                <span className="grow ellipsis">{item.name}</span>
              )}
              {!item.unknown && !editing && (
                <button
                  tabIndex={-1}
                  className="icon-btn tiny"
                  onClick={(e) => {
                    e.stopPropagation();
                    const b = e.currentTarget.getBoundingClientRect();
                    menuAt(b.left, b.bottom + 4);
                  }}
                  title="Rename, merge"
                >
                  <Ellipsis size={12} />
                </button>
              )}
              <span className="count">{fmtNum(item.count)}</span>
            </div>
          );
        })}
    </>
  );
}

/** A camera's or lens's menu: rename, merge into another, split back into the names in its files. */
function GearMenu({
  kind,
  item,
  x,
  y,
  others,
  onClose,
  onRename,
  onMerge,
  onSplit,
}: GearMenuAt & {
  others: GearItem[];
  onClose: () => void;
  onRename: () => void;
  onMerge: (into: string) => void;
  onSplit: () => void;
}) {
  const act = (fn: () => void) => () => {
    onClose();
    fn();
  };
  const targets = others.filter((o) => !o.unknown && o.name !== item.name);
  return (
    <div
      className="menu folder-menu gear-menu"
      style={{ left: Math.min(x, window.innerWidth - 260), top: Math.min(y, window.innerHeight - 300) }}
      ref={(el) => {
        if (el && !el.contains(document.activeElement)) el.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
      }}
      onClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.preventDefault()}
      onKeyDown={(e) => {
        e.stopPropagation();
        const items = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
        const i = items.indexOf(e.target as HTMLButtonElement);
        if (e.key === "Escape" || e.key === "Tab") {
          e.preventDefault();
          onClose();
        } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
        }
      }}
    >
      <button onClick={act(onRename)} title="Shoebox's name for it; the photo files are not changed">
        Rename <span className="shortcut">F2</span>
      </button>
      {item.inFiles.length > 0 && (
        <button
          onClick={act(onSplit)}
          title={`In the files: ${item.inFiles.join(" · ")}`}
        >
          {item.inFiles.length > 1 ? "Split into the names in the files" : "Back to the name in the files"}
        </button>
      )}
      {targets.length > 0 && (
        <>
          <hr />
          <div className="menu-label">Same {kind} as…</div>
          <div className="menu-scroll">
            {targets.map((o) => (
              <button key={o.name} onClick={act(() => onMerge(o.name))} title={`Merge into ${o.name}`}>
                <span className="ellipsis">{o.name}</span>
                <span className="shortcut">{fmtNum(o.count)}</span>
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
