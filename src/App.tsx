import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { ask, open } from "@tauri-apps/plugin-dialog";
import {
  ArrowLeft,
  Eye,
  EyeOff,
  FileType,
  Pencil,
  FolderPlus,
  FolderX,
  RefreshCw,
  Search,
  Tag as TagIcon,
  Trash2,
  Unplug,
  UserCog,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ai,
  api,
  cull,
  headOf,
  people,
  fmtBytes,
  fmtDateKey,
  type AiProgress,
  fmtNum,
  type Browse,
  type DeleteReport,
  type Facets,
  type Filters,
  filtersOn,
  NO_FILTERS,
  type FolderMoveEvent,
  type PhotoDetail,
  type FolderNode,
  type Overview,
  type Person,
  type PhotoList,
  type Progress,
  type Report,
  type RootInfo,
  type Scope,
  SORTS,
  type Sort,
} from "./api";
import { DeleteDialog } from "./components/DeleteDialog";
import { FilterBar } from "./components/FilterBar";
import { StraysDialog } from "./components/StraysDialog";
import { TagEditor } from "./components/Tags";
import { NameEditor, PeopleView } from "./components/PeopleView";
import { PhotoGrid } from "./components/PhotoGrid";
import { SettingsDialog, type SettingsTab } from "./components/SettingsDialog";
import { Sidebar, type FolderTarget } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { Viewer } from "./components/Viewer";

const EMPTY: PhotoList = { ids: [], taken: [], state: [], w: [], h: [], rev: [], preview: [], marked: [], ext: [], raw: [] };

/** A per-viewer on/off remembered between sessions (convenience only). */
function useRemembered(key: string, initial: boolean) {
  const [on, setOn] = useState(() => {
    try {
      const v = localStorage.getItem(key);
      return v == null ? initial : v === "1";
    } catch {
      return initial;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(key, on ? "1" : "0");
    } catch {
      /* per-viewer convenience only */
    }
  }, [key, on]);
  return [on, setOn] as const;
}

/** A place to come back to: the view, what it showed, and the photo looked at. */
interface Place {
  view: "photos" | "people";
  scope: Scope;
  focusId: number | null;
  viewerId: number | null;
}

function deleteSummary(r: DeleteReport) {
  const where = [
    r.recycled && `${fmtNum(r.recycled)} to the Recycle Bin`,
    r.permanent && `${fmtNum(r.permanent)} deleted for good`,
  ].filter(Boolean);
  const left = r.photosRemoved ? ` ${fmtNum(r.photosRemoved)} photos left the catalog.` : "";
  return `Deleted ${fmtNum(r.deleted)} files (${fmtBytes(r.bytes)})${where.length ? `: ${where.join(", ")}` : ""}.${left}`;
}

function loadRowHeight() {
  try {
    const v = Number(localStorage.getItem("shoebox.rowHeight"));
    return v >= 100 && v <= 400 ? v : 200;
  } catch {
    return 200;
  }
}

/** A status-bar message, optionally with one action (Undo). */
export interface Message {
  text: string;
  warn?: boolean;
  action?: { label: string; run: () => void };
}

const parentOf = (rel: string) => (rel.includes("\\") ? rel.slice(0, rel.lastIndexOf("\\")) : "");

/** Where the view goes when the folder it shows (or one it's inside) moved from `from` to `to`. */
function movedScope(s: Scope, from: { volumeId: number; rel: string }, to: { volumeId: number; rel: string }): Scope {
  if (s.kind === "folder" && s.volumeId === from.volumeId && (s.relPath === from.rel || s.relPath.startsWith(from.rel + "\\")))
    return { kind: "folder", volumeId: to.volumeId, relPath: to.rel + s.relPath.slice(from.rel.length) };
  return s;
}

/** The sidebar's dates and folders, newest first (they come oldest first). */
function newestFirst(b: Browse): Browse {
  const flip = (ns: FolderNode[]): FolderNode[] => [...ns].reverse().map((n) => ({ ...n, children: flip(n.children) }));
  return {
    ...b,
    years: [...b.years].reverse().map((y) => ({ ...y, months: [...y.months].reverse() })),
    folders: b.folders.map((f) => ({ ...f, children: flip(f.children) })),
  };
}

function loadSort(): Sort {
  try {
    const v = localStorage.getItem("shoebox.sort");
    return SORTS.some(([s]) => s === v) ? (v as Sort) : "oldest";
  } catch {
    return "oldest";
  }
}

function summarize(name: string, r: Report) {
  if (r.cancelled) return `${name}: stopped.`;
  // Every file read is counted once, so the parts add up to the total.
  const read = [
    r.photosAdded && `${fmtNum(r.photosAdded)} new photos`,
    r.duplicates && `${fmtNum(r.duplicates)} the same photos again under other names`,
    r.linked && `${fmtNum(r.linked)} already in the catalog (copies or moves)`,
    r.reread && `${fmtNum(r.reread)} read again`,
    r.errors && `${fmtNum(r.errors)} couldn't be read`,
  ].filter(Boolean);
  const total = r.photosAdded + r.duplicates + r.linked + r.reread + r.errors;
  const parts = [
    read.length && `${fmtNum(total)} ${total === 1 ? "file" : "files"}: ${read.join(", ")}`,
    r.restored && `${fmtNum(r.restored)} back where they were`,
    r.missing && `${fmtNum(r.missing)} not found`,
  ].filter(Boolean);
  return `${name}: ${parts.length ? parts.join(" · ") : "nothing changed"} · ${Math.max(1, Math.round(r.seconds))} s`;
}

export default function App() {
  const [overview, setOverview] = useState<Overview | null>(null);
  const [browse, setBrowse] = useState<Browse | null>(null);
  const [version, setVersion] = useState("");
  const [scope, setScope] = useState<Scope>({ kind: "all" });
  const [list, setList] = useState<PhotoList>(EMPTY);
  const [viewerId, setViewerId] = useState<number | null>(null);
  const [focusId, setFocusId] = useState<number | null>(null);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [queued, setQueued] = useState(0);
  const [message, setMessage] = useState<Message | null>(null);
  // A folder on its way to another disk, and where it came from.
  const [folderMove, setFolderMove] = useState<FolderMoveEvent | null>(null);
  const movingFrom = useRef<FolderTarget | null>(null);
  const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
  const settingsOpen = settingsTab !== null;
  // A message about a missing AI model offers to open Settings > AI models.
  // (Its shoebox-cli hint is for agents, not the window.)
  const modelHint = (text: string): Message =>
    text.includes("Settings > AI models")
      ? {
          text: text.replace(/ \(or: shoebox-cli [^)]*\)/, ""),
          warn: true,
          action: { label: "AI models…", run: () => setSettingsTab("models") },
        }
      : { text, warn: true };
  const [rowHeight, setRowHeight] = useState(loadRowHeight);
  const [sort, setSort] = useState<Sort>(loadSort);
  // File type (CR3, JPG...) on each thumbnail.
  const [showType, setShowType] = useRemembered("shoebox.showType", false);
  const sortRef = useRef(sort);
  sortRef.current = sort;
  // Word search: what is typed, and what is being shown.
  const [typed, setTyped] = useState("");
  const [query, setQuery] = useState("");
  const [aiProgress, setAiProgress] = useState<AiProgress | null>(null);
  const [clearCount, setClearCount] = useState(0);
  const [showPossible, setShowPossible] = useState(false);
  // "people" shows the People view instead of the photo grid.
  const [view, setView] = useState<"photos" | "people">("photos");
  const [peopleList, setPeopleList] = useState<Person[]>([]);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [straysFor, setStraysFor] = useState<Person | null>(null);
  // Naming the person whose page this is, from the title.
  const [namingTitle, setNamingTitle] = useState(false);
  const personHere = scope.kind === "person" ? peopleList.find((p) => p.id === scope.id) : undefined;
  useEffect(() => setNamingTitle(false), [scope]);
  // Photos being tagged from the grid (T, or the Tag button).
  const [tagging, setTagging] = useState<number[] | null>(null);
  // Escape closes it - unless there's text in the tag box, which Escape clears first.
  useEffect(() => {
    if (!tagging) return;
    const esc = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || (e.target as HTMLInputElement | null)?.value) return;
      e.stopPropagation(); // not also Back
      setTagging(null);
      setGridFocus((n) => n + 1);
    };
    window.addEventListener("keydown", esc, true);
    return () => window.removeEventListener("keydown", esc, true);
  }, [tagging]);
  // Where drill-downs came from, for Back and Escape. The sidebar starts afresh.
  const [trail, setTrail] = useState<Place[]>([]);
  // Grid selection (photo ids); `focusId` is the current photo within it.
  const [selected, setSelected] = useState<Set<number>>(() => new Set());
  // Bumped to hand the keyboard back to the grid (viewer closed).
  const [gridFocus, setGridFocus] = useState(0);

  // Filters narrow whatever is shown and stay on from folder to folder.
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const [facets, setFacets] = useState<Facets | null>(null);
  const filtersRef = useRef(filters);
  filtersRef.current = filters;

  const scopeRef = useRef(scope);
  scopeRef.current = scope;
  const queryRef = useRef(query);
  queryRef.current = query;
  const lastRootName = useRef("Scan");

  const refresh = useCallback(async () => {
    try {
      const q = queryRef.current;
      const f = filtersRef.current;
      const [ov, l, b, pp, fc] = await Promise.all([
        api.overview(),
        q
          ? ai.search(q, scopeRef.current, f).then((r) => {
              setClearCount(r.clear);
              return r.list;
            })
          : api.listPhotos(scopeRef.current, sortRef.current, f),
        api.browse(),
        people.list().catch(() => [] as Person[]),
        api.facets(scopeRef.current, f).catch(() => null),
      ]);
      setBrowse(b);
      setPeopleList(pp);
      setOverview(ov);
      setList(l);
      setFacets(fc);
    } catch (e) {
      // The folder or disk shown may have been absorbed into another one.
      if (queryRef.current) setMessage(modelHint(`Search: ${e}`));
      else if (scopeRef.current.kind !== "all") setScope({ kind: "all" });
      else setMessage({ text: String(e), warn: true });
    }
  }, []);

  const timer = useRef<number | undefined>(undefined);
  // When People was last reloaded while faces are being found.
  const facesShown = useRef(0);
  const refreshSoon = useCallback(() => {
    if (timer.current) return;
    timer.current = window.setTimeout(() => {
      timer.current = undefined;
      refresh();
    }, 400);
  }, [refresh]);

  useEffect(() => {
    refresh();
  }, [scope, query, sort, filters, refresh]);

  useEffect(() => {
    try {
      localStorage.setItem("shoebox.sort", sort);
    } catch {
      /* per-viewer convenience only */
    }
  }, [sort]);

  // Search as you type, once typing pauses.
  useEffect(() => {
    const t = window.setTimeout(() => {
      setQuery(typed.trim());
      setShowPossible(false);
    }, 450);
    return () => window.clearTimeout(t);
  }, [typed]);

  useEffect(() => {
    api.scanStatus().then((s) => {
      setProgress(s.current);
      setQueued(s.queued.length);
    });
    const subs = [
      listen("catalog-changed", refreshSoon),
      listen("volumes-changed", refreshSoon),
      listen<AiProgress>("ai-progress", (e) => {
        if (e.payload.error) setMessage(modelHint(`Word search: ${e.payload.error}`));
        setAiProgress(e.payload.finished ? null : e.payload);
        // People fills in while faces are found, and is complete when they're done.
        if (e.payload.finished || (e.payload.task === "faces" && Date.now() - facesShown.current > 3000)) {
          facesShown.current = Date.now();
          refreshSoon();
        }
      }),
      listen<Progress>("scan-progress", (e) => {
        lastRootName.current = e.payload.rootName;
        setProgress(e.payload);
        api.scanStatus().then((s) => setQueued(s.queued.length));
      }),
      listen<FolderMoveEvent>("folder-move", (e) => {
        const ev = e.payload;
        if (!ev.finished) {
          setFolderMove(ev);
          return;
        }
        setFolderMove(null);
        const src = movingFrom.current;
        movingFrom.current = null;
        if (ev.error) {
          setMessage({ text: `Moving “${ev.name}”: ${ev.error}`, warn: true });
        } else if (ev.result) {
          const r = ev.result;
          if (src) setScope((s) => movedScope(s, { volumeId: src.v.id, rel: src.relPath }, { volumeId: r.volumeId, rel: r.rel }));
          const left = r.leftBehind.length;
          setMessage({
            text:
              `Moved “${ev.name}” to the other disk: ${fmtNum(r.files)} files (${fmtBytes(r.bytes)}) copied, checked byte for byte, original removed.` +
              (left ? ` ${fmtNum(left)} originals couldn't be removed (locked or read-only) and are still on the old disk.` : ""),
            warn: left > 0,
          });
        }
      }),
      listen<Report>("scan-done", (e) => {
        setProgress(null);
        setMessage({ text: summarize(lastRootName.current, e.payload) });
      }),
      listen<{ rootId: number; message: string }>("scan-error", (e) => {
        setProgress(null);
        setMessage({ text: e.payload.message, warn: true });
      }),
    ];
    return () => subs.forEach((p) => p.then((un) => un()));
  }, [refreshSoon]);

  useEffect(() => {
    try {
      localStorage.setItem("shoebox.rowHeight", String(rowHeight));
    } catch {
      /* per-viewer convenience only */
    }
  }, [rowHeight]);

  const addFolder = async () => {
    const dir = await open({ directory: true, multiple: false, title: "Add a folder of photos" });
    if (typeof dir !== "string") return;
    try {
      await api.addFolder(dir);
      setMessage(null);
      refresh();
    } catch (e) {
      setMessage({ text: String(e), warn: true });
    }
  };

  const run = (p: Promise<unknown>) => p.catch((e) => setMessage({ text: String(e), warn: true }));

  // Culling: the mark shows at once; the list isn't reloaded, so a photo
  // unmarked in "Marked for deletion" stays put until the next refresh.
  const setMarked = useCallback(
    (ids: number[], marked: boolean) => {
      const set = new Set(ids);
      setList((l) => ({ ...l, marked: l.marked.map((m, i) => (set.has(l.ids[i]) ? +marked : m)) }));
      cull
        .mark(ids, marked)
        .then(() => api.overview().then(setOverview))
        .catch((e) => {
          setMessage({ text: String(e), warn: true });
          refresh();
        });
    },
    [refresh],
  );

  // Drill-downs (People -> a person, a face in the viewer -> their photos)
  // remember where they came from; Back or Escape returns there.
  const drill = (to: Scope) => {
    setTrail((t) => [...t, { view, scope, focusId, viewerId }]);
    setViewerId(null);
    setView("photos");
    setScope(to);
    setFocusId(null);
  };

  const goBack = useCallback(() => {
    const prev = trail[trail.length - 1];
    if (!prev) return;
    setTrail(trail.slice(0, -1));
    setView(prev.view);
    setScope(prev.scope);
    setFocusId(prev.viewerId ?? prev.focusId);
    setViewerId(prev.viewerId);
  }, [trail]);

  /** From a person's page. Ignoring a stranger goes back to People, with Undo. */
  const ignorePerson = (p: Person) => {
    const ignored = !p.ignored;
    run(
      people.ignore([p.id], ignored).then(() => {
        const label = p.name ?? "This person";
        setMessage({
          text: ignored ? `${label} is ignored: hidden from People.` : `${label} is back in People.`,
          action: {
            label: "Undo",
            run: () => run(people.ignore([p.id], !ignored).then(refresh)),
          },
        });
        if (ignored && trail[trail.length - 1]?.view === "people") goBack();
        else refresh();
      }),
    );
  };

  useEffect(() => {
    if (!trail.length || viewerId != null || deleteOpen || settingsOpen || straysFor || tagging) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      goBack();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [trail, viewerId, deleteOpen, settingsOpen, straysFor, tagging, goBack]);

  // Drag and drop of a folder. Same disk: done at once, with Undo. Another
  // disk: asked first, then copied, checked and the original removed in the
  // background (progress in the status bar).
  const moveFolder = async (src: FolderTarget, into: FolderTarget) => {
    if (src.v.id !== into.v.id) {
      const yes = await ask(
        `Move “${src.name}” (${fmtNum(src.count)} photos) into ${into.path}?\n\n` +
          `That's another disk (${into.v.label}). Shoebox copies everything in the folder, checks every byte, ` +
          `then removes the original from ${src.v.label}. If anything goes wrong, or you press Stop, the original stays as it is.`,
        { title: "Move to another disk", okLabel: "Move", cancelLabel: "Cancel" },
      );
      if (!yes) return;
    }
    try {
      movingFrom.current = src;
      const r = await api.moveFolder(src.v.id, src.relPath, into.v.id, into.relPath);
      if (!r) return; // another disk: progress follows
      movingFrom.current = null;
      const from = { volumeId: src.v.id, rel: src.relPath };
      const to = { volumeId: r.volumeId, rel: r.rel };
      setScope((s) => movedScope(s, from, to));
      setMessage({
        text: `Moved “${src.name}” into “${into.name}”.`,
        action: {
          label: "Undo",
          run: async () => {
            try {
              const back = await api.moveFolder(to.volumeId, to.rel, from.volumeId, parentOf(from.rel));
              if (back) setScope((s) => movedScope(s, to, { volumeId: back.volumeId, rel: back.rel }));
              setMessage({ text: `“${src.name}” is back where it was.` });
              refresh();
            } catch (e) {
              setMessage({ text: String(e), warn: true });
            }
          },
        },
      });
      refresh();
    } catch (e) {
      movingFrom.current = null;
      setMessage({ text: String(e), warn: true });
    }
  };

  const unmarkAll = async () => {
    const n = overview?.marked ?? 0;
    const yes = await ask(`Unmark all ${fmtNum(n)} photos? Nothing is deleted.`, {
      title: "Unmark all",
      okLabel: "Unmark all",
      cancelLabel: "Keep marked",
    });
    if (yes) run(cull.unmarkAll().then(refresh));
  };

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  // Never the browser's own menu (Back, Refresh, Print…); text boxes keep theirs.
  useEffect(() => {
    const block = (e: MouseEvent) => {
      if (!(e.target as HTMLElement | null)?.closest("input, textarea")) e.preventDefault();
    };
    window.addEventListener("contextmenu", block);
    return () => window.removeEventListener("contextmenu", block);
  }, []);

  const regenerate = (rootId: number | null) =>
    run(
      api.regenerate(rootId).then((n) =>
        setMessage({ text: `Rebuilding previews for ${n} folder${n === 1 ? "" : "s"}. Unplugged disks keep theirs.` }),
      ),
    );

  const removeFolder = async (r: RootInfo) => {
    const yes = await ask(
      `Take "${r.name}" (${fmtNum(r.photos)} photos) out of the catalog?\n\nYour files are not touched. Photos that only live in this folder leave the catalog with their previews.`,
      { title: "Remove from catalog", kind: "warning", okLabel: "Remove", cancelLabel: "Keep" },
    );
    if (!yes) return;
    try {
      const rep = await api.removeFolder(r.id);
      if (scopeRef.current.kind !== "all") setScope({ kind: "all" });
      setMessage({ text: `${r.name} removed from the catalog (${fmtNum(rep.photos)} photos). Files untouched.` });
      refresh();
    } catch (e) {
      setMessage({ text: String(e), warn: true });
    }
  };

  // A search shows its clear matches unless "show more" was pressed.
  const shown = useMemo(
    () => (query && !showPossible ? headOf(list, clearCount) : list),
    [list, query, showPossible, clearCount],
  );
  const viewerIndex = useMemo(() => (viewerId == null ? -1 : shown.ids.indexOf(viewerId)), [viewerId, shown]);
  const focusIndex = useMemo(() => (focusId == null ? null : shown.ids.indexOf(focusId)), [focusId, shown]);

  // A new place starts with nothing selected.
  useEffect(() => setSelected(new Set()), [scope, query, filters]);

  // The selected photo's basics, for the status bar.
  const [focusDetail, setFocusDetail] = useState<PhotoDetail | null>(null);
  const anySelected = selected.size > 0;
  useEffect(() => {
    if (focusId == null || !anySelected || viewerId != null) return;
    let live = true;
    const t = window.setTimeout(() => {
      api
        .photo(focusId)
        .then((d) => live && setFocusDetail(d))
        .catch(() => {});
    }, 80);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [focusId, anySelected, viewerId]);

  const scopeInfo = useMemo(() => {
    if (scope.kind === "all") return { title: "All photos" };
    if (scope.kind === "missing") return { title: "Missing" };
    if (scope.kind === "marked") return { title: "Marked for deletion" };
    if (scope.kind === "date") return { title: fmtDateKey(scope.key) };
    if (scope.kind === "camera") return { title: scope.name };
    if (scope.kind === "person") {
      const p = peopleList.find((p) => p.id === scope.id);
      return { title: p?.name ?? "Unnamed person" };
    }
    if (scope.kind === "folder") {
      const v = overview?.volumes.find((v) => v.id === scope.volumeId);
      const mount = v?.mount ?? v?.lastMount ?? "";
      return {
        title: scope.relPath.split(/[\\/]/).pop() || v?.label || "",
        sub: `${mount.replace(/[\\/]$/, "")}\\${scope.relPath}`,
        offline: v ? !v.online : false,
      };
    }
    for (const v of overview?.volumes ?? []) {
      if (scope.kind === "volume" && v.id === scope.id) return { title: v.label, offline: !v.online };
      const r = v.roots.find((r) => scope.kind === "root" && r.id === scope.id);
      if (r) return { title: r.relPath ? r.name : v.label, sub: r.path, offline: !v.online, lost: v.online && !r.found };
    }
    return { title: "" };
  }, [scope, overview, peopleList]);

  const empty = overview && overview.volumes.length === 0;

  return (
    <div className="app">
      <Sidebar
        overview={overview}
        browse={browse && sort === "newest" ? newestFirst(browse) : browse}
        version={version}
        scope={scope}
        scanningRoot={progress?.rootId ?? null}
        onRegenerate={(id) => regenerate(id)}
        onRemove={removeFolder}
        people={peopleList.filter((p) => !p.ignored && (p.name || p.photos >= 2)).length}
        peopleOpen={view === "people"}
        onPeople={() => {
          setTrail([]);
          setView("people");
          // Faces found since the last visit show as new; Back from a person isn't a new visit.
          run(people.visit().then(refresh));
        }}
        onScope={(s) => {
          setTrail([]);
          setView("photos");
          setScope(s);
          setFocusId(null);
        }}
        onAddFolder={addFolder}
        onRescan={(id) => run(api.rescan(id))}
        onRename={(id, label) => run(api.renameVolume(id, label).then(refresh))}
        onRenameFolder={async (vid, rel, name) => {
          try {
            const moved = await api.renameFolder(vid, rel, name);
            // Still showing that folder (or one inside it)? Follow it.
            setScope((s) =>
              s.kind === "folder" && s.volumeId === vid && (s.relPath === rel || s.relPath.startsWith(rel + "\\"))
                ? { ...s, relPath: moved + s.relPath.slice(rel.length) }
                : s,
            );
            refresh();
          } catch (e) {
            setMessage({ text: String(e), warn: true });
          }
        }}
        onNewFolder={async (vid, parentRel) => {
          try {
            const rel = await api.newFolder(vid, parentRel);
            refresh();
            return rel;
          } catch (e) {
            setMessage({ text: String(e), warn: true });
            return null;
          }
        }}
        onShowFolder={(vid, rel) => run(api.showFolder(vid, rel))}
        onMoveFolder={moveFolder}
        onSettings={() => setSettingsTab("general")}
      />

      <main className="main">
        <header className="toolbar">
          <div className="title">
            {trail.length > 0 && (
              <button
                className="icon-btn back-btn"
                onClick={goBack}
                aria-label="Back"
                title={trail[trail.length - 1].view === "people" ? "Back to People (Esc)" : "Back (Esc)"}
              >
                <ArrowLeft size={18} />
              </button>
            )}
            {view === "photos" && personHere ? (
              namingTitle ? (
                <NameEditor
                  wide
                  person={personHere}
                  everyone={peopleList}
                  onRename={(name) => {
                    setNamingTitle(false);
                    if (name.trim() !== (personHere.name ?? "")) run(people.rename(personHere.id, name).then(refresh));
                  }}
                  onMerge={(into) => {
                    setNamingTitle(false);
                    run(people.merge([personHere.id, into.id], into.id).then(() => setScope({ kind: "person", id: into.id })));
                  }}
                  onCancel={() => setNamingTitle(false)}
                />
              ) : (
                <>
                  <h1>
                    <button
                      className={`title-name ${personHere.name ? "" : "unnamed"}`}
                      onClick={() => setNamingTitle(true)}
                      title="Name this person"
                    >
                      {personHere.name ?? "Unnamed person"} <Pencil size={14} />
                    </button>
                  </h1>
                  <button
                    className="btn small ghost"
                    onClick={() => ignorePerson(personHere)}
                    title={personHere.ignored ? "Show them in People again" : "A stranger: hide them from People"}
                  >
                    {personHere.ignored ? <Eye size={14} /> : <EyeOff size={14} />}{" "}
                    {personHere.ignored ? "Show again" : "Ignore"}
                  </button>
                </>
              )
            ) : (
              <h1>{view === "people" ? "People" : scopeInfo.title}</h1>
            )}
            <span className="muted">
              {fmtNum(shown.ids.length)} {query ? "matches" : "photos"}
              {view === "photos" && selected.size > 1 && ` · ${fmtNum(selected.size)} selected`}
            </span>
            {"sub" in scopeInfo && scopeInfo.sub && <span className="muted path">{scopeInfo.sub}</span>}
          </div>
          <div className="search">
            <Search size={15} />
            <input
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") setQuery(typed.trim());
                if (e.key === "Escape") setTyped("");
              }}
              placeholder="Search in words: succulent, airport lounge, kids on a beach…"
              spellCheck={false}
            />
            {typed && (
              <button className="icon-btn tiny-x" onClick={() => setTyped("")} aria-label="Clear search">
                <X size={14} />
              </button>
            )}
          </div>
          <div className="tools">
            {view === "photos" && scope.kind === "person" && (
              <button
                className="btn ghost"
                onClick={() => setStraysFor(peopleList.find((p) => p.id === scope.id) ?? null)}
                title="Find faces in this person that don't look like them, and move them out"
              >
                <UserCog size={15} /> Clean up faces…
              </button>
            )}
            {view === "photos" && (
              <select
                className="sort-select"
                value={sort}
                disabled={!!query}
                onChange={(e) => setSort(e.target.value as Sort)}
                title={query ? "Search results are in order of best match" : "Order of the photos"}
              >
                {SORTS.map(([s, label]) => (
                  <option key={s} value={s}>
                    {label}
                  </option>
                ))}
              </select>
            )}
            {view === "photos" && selected.size > 0 && (
              <button
                className="btn ghost"
                onClick={() => setTagging(shown.ids.filter((id) => selected.has(id)))}
                title="Tag the selected photos (T)"
              >
                <TagIcon size={15} /> Tag{selected.size > 1 ? ` ${fmtNum(selected.size)}` : ""}…
              </button>
            )}
            {view === "photos" && (
              <button
                className={`icon-btn type-toggle ${showType ? "on" : ""}`}
                onClick={() => setShowType(!showType)}
                title={showType ? "Hide the file type on thumbnails" : "Show the file type (CR3, JPG…) on thumbnails"}
                aria-pressed={showType}
              >
                <FileType size={16} />
              </button>
            )}
            <input
              type="range"
              min={110}
              max={380}
              value={rowHeight}
              onChange={(e) => setRowHeight(+e.target.value)}
              title="Thumbnail size"
            />
            <button className="btn ghost" onClick={() => run(api.rescanAll())} title="Scan every folder whose disk is plugged in">
              <RefreshCw size={15} /> Scan plugged-in disks
            </button>
          </div>
        </header>

        {view === "photos" && !empty && (
          <FilterBar facets={facets} filters={filters} onChange={setFilters} people={peopleList} />
        )}
        {view === "photos" && filtersOn(filters) && list.ids.length === 0 && (
          <div className="banner">
            No photos here match these filters.
            <button className="btn small ghost" onClick={() => setFilters(NO_FILTERS)}>
              Clear filters
            </button>
          </div>
        )}

        {"offline" in scopeInfo && scopeInfo.offline && (
          <div className="banner">
            <Unplug size={15} /> This disk is unplugged. You're browsing Shoebox's copies; nothing is lost.
          </div>
        )}
        {"lost" in scopeInfo && scopeInfo.lost && (
          <div className="banner warn">
            <FolderX size={15} /> This folder isn't at {scopeInfo.sub} any more. Its photos stay in the catalog, shown
            from Shoebox's copies. Put it back and press ↻ next to it to scan it again.
          </div>
        )}
        {query && list.ids.length > clearCount && (
          <div className="banner">
            {showPossible
              ? `Showing ${fmtNum(clearCount)} clear and ${fmtNum(list.ids.length - clearCount)} possible matches, best first.`
              : `Showing the ${fmtNum(clearCount)} clear matches.`}
            <button className="btn small ghost" onClick={() => setShowPossible(!showPossible)}>
              {showPossible ? "Clear matches only" : `Show ${fmtNum(list.ids.length - clearCount)} more possible matches`}
            </button>
          </div>
        )}
        {scope.kind === "missing" && (
          <div className="banner warn">
            These photos weren't where Shoebox last saw them, on a disk that was plugged in. They stay in the catalog: if
            they turn up on any disk, the next scan finds them again.
          </div>
        )}
        {scope.kind === "marked" && view === "photos" && (
          <div className="banner">
            <Trash2 size={15} /> Nothing is deleted until you confirm. Press Delete on a photo to unmark it.
            <span className="grow" />
            <button className="btn small ghost" disabled={!overview?.marked} onClick={unmarkAll}>
              Unmark all
            </button>
            <button className="btn small danger" disabled={!overview?.marked} onClick={() => setDeleteOpen(true)}>
              <Trash2 size={14} /> Delete marked files…
            </button>
          </div>
        )}

        {view === "people" ? (
          <PeopleView
            people={peopleList}
            onOpen={(p) => drill({ kind: "person", id: p.id })}
            onChanged={refresh}
            onError={(e) => setMessage({ text: e, warn: true })}
            onOpenModels={() => setSettingsTab("models")}
          />
        ) : empty ? (
          <div className="empty">
            <div className="empty-card">
              <h2>Add your first folder</h2>
              <p>
                Shoebox lists every photo in it and keeps a copy of each one, so you can browse them even when the disk is
                unplugged. It never changes, moves or deletes your files.
              </p>
              <button className="btn primary" onClick={addFolder}>
                <FolderPlus size={16} /> Add folder
              </button>
            </div>
          </div>
        ) : (
          <PhotoGrid
            key={JSON.stringify(scope) + query + sort + JSON.stringify(filters)}
            list={shown}
            rowHeight={rowHeight}
            onOpen={(i) => setViewerId(shown.ids[i])}
            onSelect={(cursor, sel) => {
              setFocusId(cursor >= 0 ? shown.ids[cursor] : null);
              setSelected(sel);
            }}
            onMark={setMarked}
            onTag={setTagging}
            selected={selected}
            keyboard={viewerIndex < 0 && !deleteOpen && !settingsOpen && !straysFor && !tagging}
            focusRequest={gridFocus}
            focusIndex={focusIndex}
            showType={showType}
            flatLabel={
              query
                ? `Best matches for "${query}"`
                : sort === "name"
                  ? "By file name"
                  : sort === "added"
                    ? "Most recently added first"
                    : undefined
            }
          />
        )}

        <StatusBar
          overview={overview}
          progress={progress}
          aiProgress={aiProgress}
          queued={queued}
          message={message}
          onCancel={() => run(api.cancelScan())}
          move={folderMove}
          onStopMove={() => run(api.cancelFolderMove())}
          photo={view === "photos" && selected.size > 0 && focusDetail?.id === focusId ? focusDetail : null}
          selected={selected.size}
        />
      </main>

      {viewerIndex >= 0 && (
        <Viewer
          list={shown}
          index={viewerIndex}
          onIndex={(i) => setViewerId(shown.ids[i])}
          onToggleMark={() => setMarked([shown.ids[viewerIndex]], shown.marked[viewerIndex] !== 1)}
          onTagsChanged={refreshSoon}
          onPerson={(id) => drill({ kind: "person", id })}
          onClose={() => {
            setFocusId(viewerId);
            if (viewerId != null) setSelected(new Set([viewerId]));
            setViewerId(null);
            setGridFocus((n) => n + 1);
          }}
        />
      )}
      {tagging && (
        <div className="modal-back" onClick={() => setTagging(null)}>
          <div className="modal tag-modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <h2>
                Tags <span className="muted small">{tagging.length === 1 ? "1 photo" : `${fmtNum(tagging.length)} photos`}</span>
              </h2>
              <button className="icon-btn" onClick={() => setTagging(null)} aria-label="Close">
                <X size={18} />
              </button>
            </div>
            <TagEditor photoIds={tagging} onChanged={refreshSoon} autoFocus />
            <p className="muted small">Enter adds · commas add several · Esc closes. Tags stay with the photo when it moves or is copied.</p>
          </div>
        </div>
      )}
      {straysFor && (
        <StraysDialog
          person={straysFor}
          onClose={() => setStraysFor(null)}
          onDone={(moved) => {
            setMessage({ text: `${fmtNum(moved)} faces moved out of ${straysFor.name ?? "this person"}.` });
            setStraysFor(null);
            refresh();
          }}
        />
      )}
      {deleteOpen && (
        <DeleteDialog
          onClose={() => setDeleteOpen(false)}
          onDone={(r) => {
            setDeleteOpen(false);
            setMessage({
              text: deleteSummary(r) + (r.failed.length ? ` Not deleted: ${r.failed.join("; ")}` : ""),
              warn: r.failed.length > 0,
            });
            refresh();
          }}
        />
      )}
      {settingsTab && (
        <SettingsDialog
          initialTab={settingsTab}
          dataDir={overview?.dataDir ?? ""}
          version={version}
          onRegenerateAll={() => regenerate(null)}
          onClose={() => setSettingsTab(null)}
        />
      )}
    </div>
  );
}
