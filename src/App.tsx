import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { ask, open } from "@tauri-apps/plugin-dialog";
import {
  ArchiveX,
  ArrowLeft,
  CalendarClock,
  CalendarDays,
  Eye,
  ListChecks,
  EyeOff,
  FileType,
  Focus,
  Pencil,
  FolderPlus,
  FolderX,
  LockKeyhole,
  LockKeyholeOpen,
  RefreshCw,
  Search,
  Sparkles,
  Tag as TagIcon,
  Trash2,
  Unplug,
  UserCog,
  UserSearch,
  UserX,
  X,
} from "lucide-react";
import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import {
  ai,
  BIN,
  api,
  cull,
  datesApi,
  type DateChange,
  type DateFinding,
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
  type FileLoc,
  type FolderMoveEvent,
  gearApi,
  type GearKind,
  type PhotoDetail,
  type FolderNode,
  type Overview,
  type Merged,
  type MissingPhoto,
  type Moved,
  type Person,
  type PhotoList,
  PRIVATE_SHORTCUT,
  privateApi,
  type PrivateStatus,
  type Progress,
  type Report,
  type RootInfo,
  type Shifted,
  sameScope,
  type Scope,
  SORTS,
  type Sort,
  turnApi,
  type Turned,
  type TurnReport,
  type UpdateStatus,
  updatesApi,
  withoutPrivate,
} from "./api";
import { CalendarView } from "./components/CalendarView";
import { CullView } from "./components/CullView";
import { DatesCheckDialog, movedText } from "./components/DatesCheckDialog";
import { FixDatesDialog } from "./components/FixDatesDialog";
import { DeleteDialog } from "./components/DeleteDialog";
import { FilterBar } from "./components/FilterBar";
import { StraysDialog } from "./components/StraysDialog";
import { FindMoreDialog } from "./components/FindMoreDialog";
import { TagEditor } from "./components/Tags";
import { NameEditor, PeopleView, PersonSearch } from "./components/PeopleView";
import { PhotoGrid } from "./components/PhotoGrid";
import { PrivateDialog } from "./components/Private";
import { SettingsDialog, type SettingsTab } from "./components/SettingsDialog";
import { Sidebar, type FolderTarget } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { TurnControl, TurnPhotoIcon } from "./components/TurnControl";
import { turnWords, Viewer } from "./components/Viewer";
import { useRemembered } from "./remembered";
import { WhoIsIt } from "./components/WhoIsIt";

const EMPTY: PhotoList = {
  ids: [],
  taken: [],
  state: [],
  w: [],
  h: [],
  rev: [],
  preview: [],
  marked: [],
  ext: [],
  raw: [],
  fresh: [],
  flags: [],
  sideways: [],
  private: [],
};

/** What a turn did, for the status bar. `how`: the words for the turn, when all turned alike. */
function turnedText(r: TurnReport, how: string | null) {
  const what = r.turned === 1 ? "the photo" : `${fmtNum(r.turned)} photos`;
  const turned = r.turned
    ? `Turned ${what}${how ? ` ${how}` : ""}. Only Shoebox's ${r.turned === 1 ? "copy" : "copies"} changed: the files stay as they are.`
    : "Nothing to turn.";
  const again = r.toReadAgain.length
    ? ` ${fmtNum(r.toReadAgain.length)} couldn't be turned from Shoebox's copy: ${r.toReadAgain.length === 1 ? "it is" : "they are"} read again, turned, when ${r.toReadAgain.length === 1 ? "its disk is" : "their disks are"} plugged in.`
    : "";
  return turned + again;
}

/** A handler that stays the same from one render to the next but always runs
 *  the latest `f`: the grid and People aren't drawn again because of it. */
function useLatest<A extends unknown[], R>(f: (...args: A) => R) {
  const latest = useRef(f);
  latest.current = f;
  return useCallback((...args: A) => latest.current(...args), []);
}

/** Cull works on a shoot: up to this many photos at once (as the backend). */
const CULL_MAX = 5000;

/** What the main area shows: the photo grid, People, or Cull on the photos of the grid. */
type View = "photos" | "people" | "cull";

/** A place to come back to: the view, what it showed, and the photo looked at. */
interface Place {
  view: View;
  scope: Scope;
  focusId: number | null;
  viewerId: number | null;
  /** The word search it showed, if any. */
  query: string;
}

function deleteSummary(r: DeleteReport) {
  const where = [
    r.recycled && `${fmtNum(r.recycled)} to the ${BIN}`,
    r.permanent && `${fmtNum(r.permanent)} deleted for good`,
  ].filter(Boolean);
  const left = r.photosRemoved ? ` ${fmtNum(r.photosRemoved)} photos left the catalog.` : "";
  const gone = r.forgotten
    ? ` ${fmtNum(r.forgotten)} of them ${r.forgotten === 1 ? "was" : "were"} already gone from ${r.forgotten === 1 ? "its disk" : "their disks"}.`
    : "";
  const deleted = r.deleted || !r.forgotten ? `Deleted ${fmtNum(r.deleted)} files (${fmtBytes(r.bytes)})${where.length ? `: ${where.join(", ")}` : ""}.` : "";
  return `${deleted}${left}${gone}`.trim();
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

/** A folder as the sidebar shows it: a source when it is one, else a folder inside one. */
function folderScope(ov: Overview | null, volumeId: number, relPath: string): Scope {
  const root = ov?.volumes.find((v) => v.id === volumeId)?.roots.find((r) => r.relPath === relPath);
  return root ? { kind: "root", id: root.id } : { kind: "folder", volumeId, relPath };
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
  if (r.retry) {
    const fixed = r.photosAdded + r.duplicates + r.linked + r.reread;
    return (
      `${name}: ${fmtNum(fixed)} ${fixed === 1 ? "photo that couldn't be read before is" : "photos that couldn't be read before are"} fine now` +
      (r.errors ? `, ${fmtNum(r.errors)} still can't be read` : "")
    );
  }
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

/** People at the start of a status message: up to three names, then how many more. */
function whoOf(names: string[], unnamed: number) {
  const shown = names.slice(0, 3);
  const rest = names.length - shown.length + unnamed;
  const parts = [...shown];
  if (names.length > shown.length) parts.push(rest === 1 ? "1 other" : `${fmtNum(rest)} others`);
  else if (unnamed) parts.push(unnamed === 1 ? "an unnamed person" : `${fmtNum(unnamed)} unnamed people`);
  const text = parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}` : parts[0];
  return shown.length ? text : text[0].toUpperCase() + text.slice(1);
}

const namesOf = (joined: Merged["joined"]) => joined.flatMap((j) => (j.name ? [j.name] : []));

/** A sentence ending in a name: one full stop, even after "Jr." */
const stop = (s: string) => (s.endsWith(".") ? s : `${s}.`);

/** Who a merge made one: "Bee is now part of Bea." */
function mergedText(m: Merged) {
  const names = namesOf(m.joined);
  const unnamed = m.joined.length - names.length;
  if (m.name) return stop(`${whoOf(names, unnamed)} ${m.joined.length === 1 ? "is" : "are"} now part of ${m.name}`);
  // The person they joined had no name: it took the first of theirs, if any.
  return stop(`${whoOf(names, unnamed + 1)} are now one person${names.length ? `: ${names[0]}` : ""}`);
}

/** After its Undo: "Bea and Bee are apart again." */
function unmergedText(m: Merged) {
  const names = [...(m.name ? [m.name] : []), ...namesOf(m.joined)];
  return `${whoOf(names, m.joined.length + 1 - names.length)} are apart again.`;
}

export default function App() {
  const [overview, setOverview] = useState<Overview | null>(null);
  const [browse, setBrowse] = useState<Browse | null>(null);
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<UpdateStatus | null>(null);
  // Each new version is announced once in the status bar; the sidebar keeps it.
  const announced = useRef<string | null>(null);
  const [scope, setScope] = useState<Scope>({ kind: "all" });
  // The photos shown, and the view they were loaded for (see `viewKey`): its
  // search and order, which the grid follows until the next view's photos are here.
  const [loaded, setLoaded] = useState<{ key: string; query: string; sort: Sort; list: PhotoList; clear: number }>({
    key: "",
    query: "",
    sort: "oldest",
    list: EMPTY,
    clear: 0,
  });
  const { list, clear: clearCount } = loaded;
  const [viewerId, setViewerId] = useState<number | null>(null);
  const [focusId, setFocusId] = useState<number | null>(null);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [queued, setQueued] = useState<number[]>([]);
  const [message, setMessage] = useState<Message | null>(null);
  // A folder on its way to another disk, and where it came from.
  const [folderMove, setFolderMove] = useState<FolderMoveEvent | null>(null);
  const movingFrom = useRef<FolderTarget | null>(null);
  const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
  const settingsOpen = settingsTab !== null;
  // Private photos: locked at every start, and while locked nothing shows they exist.
  // Ctrl+Shift+L (or Settings) asks for a password: each opens its own space. The
  // first photos made private in a space ask for its password again (`confirm`).
  const [privateStatus, setPrivateStatus] = useState<PrivateStatus | null>(null);
  const [privateAsk, setPrivateAsk] = useState<
    { mode: "unlock" } | { mode: "confirm"; hide: (password: string) => Promise<number> } | null
  >(null);
  const unlocked = privateStatus?.unlocked ?? false;
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
  // The slider follows the mouse at once; the grid, laid out again for each
  // size (tens of ms on a big catalog), follows when it can.
  const gridRowHeight = useDeferredValue(rowHeight);
  const [sort, setSort] = useState<Sort>(loadSort);
  // File type (CR3, JPG...) on each thumbnail.
  const [showType, setShowType] = useRemembered("shoebox.showType", false);
  // What Cull found wrong (blurry, dark...) on each thumbnail.
  const [showProblems, setShowProblems] = useRemembered("shoebox.showProblems", false);
  // The sidebar folded to a rail of icons.
  const [folded, setFolded] = useRemembered("shoebox.sidebarFolded", false);
  const sortRef = useRef(sort);
  sortRef.current = sort;
  // Word search: what is typed, and what is being shown.
  const [typed, setTyped] = useState("");
  const [query, setQuery] = useState("");
  const [aiProgress, setAiProgress] = useState<AiProgress | null>(null);
  const [showPossible, setShowPossible] = useState(false);
  // "people" shows the People view instead of the photo grid; "cull", Cull on the grid's photos.
  const [view, setView] = useState<View>("photos");
  const [peopleList, setPeopleList] = useState<Person[]>([]);
  // People's search box: what's typed, and whoever it finds (null: nothing typed).
  const [personTyped, setPersonTyped] = useState("");
  // People's "Only new": just the people new or found in new photos.
  const [peopleNewOnly, setPeopleNewOnly] = useState(false);
  const [personFound, setPersonFound] = useState<Person[] | null>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [straysFor, setStraysFor] = useState<Person | null>(null);
  const [moreFor, setMoreFor] = useState<Person | null>(null);
  // "Who is this?" for a person's face in the selected photos.
  const [reassigning, setReassigning] = useState<{ faceIds: number[]; from: Person; photos: number } | null>(null);
  // Faces changed in the viewer: the grid is reloaded when it closes.
  const facesChanged = useRef(false);
  // Naming the person whose page this is, from the title.
  const [namingTitle, setNamingTitle] = useState(false);
  const personHere = scope.kind === "person" ? peopleList.find((p) => p.id === scope.id) : undefined;
  useEffect(() => setNamingTitle(false), [scope]);
  // "Check dates" (in a folder and those in it, or everywhere), and "Fix dates" for some photos.
  const [datesCheck, setDatesCheck] = useState<{ where: { volumeId: number; relPath: string; name: string } | null } | null>(
    null,
  );
  const [fixingDates, setFixingDates] = useState<number[] | null>(null);
  const datesOpen = datesCheck != null || fixingDates != null;
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
  // The calendar covers the grid; a day picked there jumps the grid to it.
  const [calendar, setCalendar] = useState(false);
  const [jump, setJump] = useState<{ index: number; seq: number } | null>(null);
  const gridTop = useRef(0);

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

  // What the grid shows: the view asked for, and whether its photos are here yet.
  const viewKey = JSON.stringify([scope, query, sort, filters]);
  const viewKeyRef = useRef(viewKey);
  viewKeyRef.current = viewKey;
  const current = loaded.key === viewKey;

  // In the Missing view, once its photos are here: those whose files are in the
  // system's bin (deleted, not moved). Kept with the list they are about.
  const [binned, setBinned] = useState<{ list: PhotoList; photos: Map<number, MissingPhoto> } | null>(null);
  const missingView = scope.kind === "missing" && current;
  useEffect(() => {
    if (!missingView || list.ids.length === 0) return;
    let left = false;
    api
      .missingPhotos(list.ids)
      .then((r) => {
        if (!left) setBinned({ list, photos: new Map(r.photos.filter((p) => p.inBin).map((p) => [p.id, p])) });
      })
      .catch(() => {});
    return () => {
      left = true;
    };
  }, [missingView, list]);
  const inBin = missingView && binned?.list === list ? binned.photos : null;

  // The photos of the view shown, then its filters' counts. Only the latest
  // request counts: an answer for a view already left is dropped.
  const viewSeq = useRef(0);
  const loadView = useCallback(async () => {
    const seq = ++viewSeq.current;
    const [key, s, q, o, f] = [viewKeyRef.current, scopeRef.current, queryRef.current, sortRef.current, filtersRef.current];
    const counts = api.facets(s, f).catch(() => null);
    try {
      const found = q ? await ai.search(q, s, f) : { list: await api.listPhotos(s, o, f), clear: 0 };
      if (seq !== viewSeq.current) return;
      setLoaded({ key, query: q, sort: o, ...found });
    } catch (e) {
      if (seq !== viewSeq.current) return;
      // The folder or disk shown may have been absorbed into another one.
      if (!q && s.kind !== "all") return setScope({ kind: "all" });
      setLoaded({ key, query: q, sort: o, list: EMPTY, clear: 0 });
      setMessage(q ? modelHint(`Search: ${e}`) : { text: String(e), warn: true });
    }
    const fc = await counts;
    if (seq === viewSeq.current) setFacets(fc);
  }, []);

  // The sidebar, the status bar's totals and People: the same whatever the view.
  const catalogSeq = useRef(0);
  const loadCatalog = useCallback(() => {
    const seq = ++catalogSeq.current;
    const latest =
      <T,>(set: (v: T) => void) =>
      (v: T) => {
        if (seq === catalogSeq.current) set(v);
      };
    const warn = (e: unknown) => setMessage({ text: String(e), warn: true });
    return Promise.all([
      api.overview().then(latest(setOverview)).catch(warn),
      api.browse().then(latest(setBrowse)).catch(warn),
      people
        .list()
        .then(latest(setPeopleList))
        .catch(() => {}),
    ]);
  }, []);

  const refresh = useCallback(() => Promise.all([loadView(), loadCatalog()]), [loadView, loadCatalog]);

  const timer = useRef<number | undefined>(undefined);
  // When People was last reloaded while faces are being found.
  const facesShown = useRef(0);
  // A refresh asked for by background work (a scan, faces found) never starts
  // while the last one is still answering: on a big catalog they would pile
  // up. It runs once that one is done.
  const refreshing = useRef(false);
  const again = useRef(false);
  const refreshSoon = useCallback(() => {
    if (timer.current) return;
    const go = () => {
      timer.current = undefined;
      if (refreshing.current) {
        again.current = true;
        return;
      }
      refreshing.current = true;
      refresh().finally(() => {
        refreshing.current = false;
        if (again.current) {
          again.current = false;
          timer.current = window.setTimeout(go, 400);
        }
      });
    };
    timer.current = window.setTimeout(go, 400);
  }, [refresh]);

  // Another view loads only its own photos; the rest is reloaded when the catalog changes.
  useEffect(() => {
    loadView();
  }, [viewKey, loadView]);
  useEffect(() => {
    loadCatalog();
  }, [loadCatalog]);

  // A view still loading after a moment says so (and dims the last one); a
  // quicker one just takes its place.
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    if (current) return setSlow(false);
    const t = window.setTimeout(() => setSlow(true), 150);
    return () => window.clearTimeout(t);
  }, [current]);

  useEffect(() => {
    try {
      localStorage.setItem("shoebox.sort", sort);
    } catch {
      /* per-viewer convenience only */
    }
  }, [sort]);

  // People are found as you type, and again when People changes (a rename, a
  // merge). Only the latest answer counts.
  const findSeq = useRef(0);
  useEffect(() => {
    const seq = ++findSeq.current;
    if (!personTyped.trim()) {
      setPersonFound(null);
      return;
    }
    people
      .find(personTyped)
      .then((found) => seq === findSeq.current && setPersonFound(found))
      .catch((e) => setMessage({ text: String(e), warn: true }));
  }, [personTyped, peopleList]);

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
      setQueued(s.queued);
    });
    const subs = [
      listen("catalog-changed", refreshSoon),
      listen("volumes-changed", refreshSoon),
      listen<AiProgress>("ai-progress", (e) => {
        if (e.payload.error) setMessage(modelHint(`Word search: ${e.payload.error}`));
        setAiProgress(e.payload.finished ? null : e.payload);
        // People (and the photos shown, with their filters) fill in while faces
        // are found; everything is reloaded once they're done.
        if (e.payload.finished) refreshSoon();
        else if (e.payload.task === "faces" && Date.now() - facesShown.current > 3000) {
          facesShown.current = Date.now();
          loadView();
          people
            .list()
            .then(setPeopleList)
            .catch(() => {});
        }
      }),
      listen<Progress>("scan-progress", (e) => {
        lastRootName.current = e.payload.rootName;
        setProgress(e.payload);
        api.scanStatus().then((s) => setQueued(s.queued));
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
        api.scanStatus().then((s) => setQueued(s.queued)); // a Stop empties the queue
        // A disk just plugged in had files that couldn't be read: a word only if some are fine now.
        const r = e.payload;
        if (r.retry && r.errors === r.found) return;
        setMessage({ text: summarize(lastRootName.current, r) });
      }),
      listen<{ rootId: number; message: string }>("scan-error", (e) => {
        setProgress(null);
        api.scanStatus().then((s) => setQueued(s.queued));
        setMessage({ text: e.payload.message, warn: true });
      }),
    ];
    return () => subs.forEach((p) => p.then((un) => un()));
  }, [refreshSoon, loadView]);

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

  // Private photos. Locking hides them at once: from the grid before the next
  // answer comes, from the viewer, the selection and the Private view.
  useEffect(() => {
    privateApi
      .status()
      .then(setPrivateStatus)
      .catch(() => {});
  }, []);
  const privateChanged = useLatest((s: PrivateStatus) => {
    setPrivateStatus(s);
    setPrivateAsk(null);
    if (!s.unlocked) {
      setLoaded((v) => ({ ...v, list: withoutPrivate(v.list) }));
      setViewerId(null);
      setSelected(new Set());
      setTagging(null);
      // The Private filter only exists while open.
      setFilters((f) => (f.private.length ? { ...f, private: [] } : f));
    }
    // Another space may be open now: the Private view shows its photos, or All photos once locked.
    if (!s.unlocked && scopeRef.current.kind === "private") setScope({ kind: "all" });
    refresh();
  });
  const lockPrivate = useLatest(() => run(privateApi.lock().then(privateChanged)));
  // Ctrl+Shift+L, from anywhere: a password, or lock.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey || e.key.toLowerCase() !== "l") return;
      e.preventDefault();
      if (unlocked) lockPrivate();
      else setPrivateAsk({ mode: "unlock" });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [unlocked, lockPrivate]);
  // Open, they lock again after a while without using Shoebox (Settings).
  useEffect(() => {
    if (!unlocked) return;
    let minutes = 5;
    let last = Date.now();
    api
      .settings()
      .then((s) => (minutes = s.privateLockMinutes || 5))
      .catch(() => {});
    const used = () => (last = Date.now());
    const events = ["pointerdown", "pointermove", "keydown", "wheel"] as const;
    events.forEach((e) => window.addEventListener(e, used, { passive: true }));
    const t = window.setInterval(() => {
      if (Date.now() - last > minutes * 60_000) lockPrivate();
    }, 10_000);
    return () => {
      events.forEach((e) => window.removeEventListener(e, used));
      window.clearInterval(t);
    };
  }, [unlocked, lockPrivate]);
  /** Makes photos private in the space open now; its first ones ask for its password again. */
  const hidePrivate = useLatest((hide: (confirm?: string) => Promise<number>) => {
    const done = (n: number) => {
      setMessage({
        text: `${n === 1 ? "1 photo" : `${fmtNum(n)} photos`} made private: hidden everywhere once you lock (${PRIVATE_SHORTCUT}).`,
      });
      refresh();
      return n;
    };
    if ((overview?.private ?? 0) === 0) setPrivateAsk({ mode: "confirm", hide: (password) => hide(password).then(done) });
    else hide().then(done, (e) => setMessage({ text: String(e), warn: true }));
  });
  const makePrivate = useLatest(async (ids: number[], isPrivate: boolean) => {
    if (isPrivate) return hidePrivate((confirm) => privateApi.hide(ids, confirm));
    try {
      const n = await privateApi.show(ids);
      setMessage({ text: `${n === 1 ? "1 photo" : `${fmtNum(n)} photos`} no longer private.` });
      refresh();
    } catch (e) {
      setMessage({ text: String(e), warn: true });
    }
  });
  const makeFolderPrivate = useLatest((s: Scope) => hidePrivate((confirm) => privateApi.hideIn(s, confirm)));

  // Culling: the mark shows at once; the list isn't reloaded, so a photo
  // unmarked in "Marked for deletion" stays put until the next refresh.
  const setMarked = useCallback(
    (ids: number[], marked: boolean) => {
      const set = new Set(ids);
      setLoaded((v) => ({
        ...v,
        list: { ...v.list, marked: v.list.marked.map((m, i) => (set.has(v.list.ids[i]) ? +marked : m)) },
      }));
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

  // Turning: only Shoebox's copies change, never a file. A quarter or half turn has Undo
  // (the opposite turn); the grid and viewer show the new pictures once reloaded.
  // Pointing at Turn in the toolbar: the selected thumbnails drawn turned that way (quarter turns).
  const [turnPreview, setTurnPreview] = useState(0);
  // The turned photos where they are: their new pictures and shapes at once, so the
  // grid and the viewer show how they ended up (a reload follows).
  const showTurned = (photos: Turned[]) => {
    const at = new Map(photos.map((t) => [t.id, t]));
    setLoaded((v) => {
      const l = v.list;
      const pick = <K extends "rev" | "w" | "h">(k: K) => l[k].map((x, i) => at.get(l.ids[i])?.[k] ?? x);
      return {
        ...v,
        list: { ...l, rev: pick("rev"), w: pick("w"), h: pick("h"), sideways: l.sideways.map((x, i) => (at.has(l.ids[i]) ? 0 : x)) },
      };
    });
  };
  // Turning: only Shoebox's copies change, never a file. Every turn has Undo: each
  // photo turned back by exactly as much. Says whether anything turned.
  const turnPhotos = useLatest((ids: number[], by: number | "back" | "suggested"): Promise<boolean> => {
    if (!ids.length) return Promise.resolve(false);
    // A few hundred take a while (each preview is turned): say so meanwhile.
    if (ids.length > 20) setMessage({ text: `Turning ${fmtNum(ids.length)} photos…` });
    const call = by === "back" ? turnApi.back(ids) : by === "suggested" ? turnApi.accept(ids) : turnApi.turn(ids, by);
    return call
      .then((r) => {
        const amounts = new Set(r.photos.map((t) => t.by));
        const how =
          by === "back" ? "back to how the file shows it" : amounts.size === 1 ? turnWords([...amounts][0]) : null;
        showTurned(r.photos);
        setTurnPreview(0);
        setMessage({
          text: turnedText(r, how),
          warn: r.toReadAgain.length > 0,
          action: r.turned ? { label: "Undo", run: () => undoTurn(r) } : undefined,
        });
        refresh();
        return r.turned > 0;
      })
      .catch((e) => {
        setMessage({ text: String(e), warn: true });
        return false;
      });
  });
  const undoTurn = (r: TurnReport) => {
    const byAmount = new Map<number, number[]>();
    for (const t of r.photos) byAmount.set(t.by, [...(byAmount.get(t.by) ?? []), t.id]);
    const back = [...byAmount].reduce<Promise<unknown>>((p, [by, ids]) => p.then(() => turnApi.turn(ids, -by)), Promise.resolve());
    run(
      back.then(() => {
        setMessage({ text: r.photos.length === 1 ? "The photo stands as before." : "The photos stand as before." });
        refresh();
      }),
    );
  };

  // Sideways: the photos you say are fine as they are leave it, never suggested again.
  const leaveUpright = (ids: number[]) =>
    run(
      turnApi.leave(ids).then((n) => {
        setSelected(new Set());
        setMessage({ text: `${n === 1 ? "That photo stays" : `${fmtNum(n)} photos stay`} as ${n === 1 ? "it is" : "they are"}: not suggested again.` });
        refresh();
      }),
    );

  // Cull's decisions change what the sidebar counts (marked for deletion).
  const cullChanged = useLatest(() => {
    api
      .overview()
      .then(setOverview)
      .catch(() => {});
  });
  const cullError = useLatest((text: string) => setMessage({ text, warn: true }));
  const closeCull = useLatest(() => {
    setView("photos");
    setGridFocus((n) => n + 1);
    refresh();
  });

  // Drill-downs (People -> a person, a face in the viewer -> their photos)
  // remember where they came from; Back or Escape returns there.
  const drill = (to: Scope) => {
    setTrail((t) => [...t, { view, scope, focusId, viewerId, query }]);
    setViewerId(null);
    setCalendar(false);
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
    if (prev.query !== queryRef.current) {
      setTyped(prev.query);
      setQuery(prev.query);
    }
  }, [trail]);

  // A person's new photos are read once you leave their page (like an email):
  // they keep their "New" tag while you're there.
  const personOpen = useRef<number | null>(null);
  useEffect(() => {
    const here = view === "photos" && scope.kind === "person" ? scope.id : null;
    const left = personOpen.current;
    personOpen.current = here;
    const p = left != null && left !== here ? peopleList.find((x) => x.id === left) : undefined;
    if (p && (p.isNew || p.newPhotos > 0)) run(people.read([p.id]).then(refresh));
  }, [view, scope, peopleList, refresh]);

  /** From a person's page: the selected photos aren't them. Their face in
   *  each goes to whoever the user says. */
  const notThem = (p: Person) => {
    const ids = shown.ids.filter((id) => selected.has(id));
    run(
      people.inPhotos(ids).then((faces) => {
        const mine = faces.filter((f) => f.personId === p.id);
        if (mine.length) setReassigning({ faceIds: mine.map((f) => f.id), from: p, photos: ids.length });
        else setMessage({ text: `No face of ${p.name ?? "this person"} in ${ids.length === 1 ? "this photo" : "these photos"}.` });
      }),
    );
  };

  /** Faces moved: says where, with Undo (every face changed goes back). */
  const facesMoved = (moved: Moved, to: string, from: Person | null) => {
    const what = moved.faces === 1 ? "1 face" : `${fmtNum(moved.faces)} faces`;
    // A face already theirs in the photo made room: say where it went.
    const where = [...new Set(moved.bumped)].map((id) => {
      const p = peopleList.find((q) => q.id === id);
      return p ? (p.name ?? "an unnamed person") : "someone new";
    });
    const room = moved.bumped.length
      ? ` ${to} was already in ${moved.bumped.length === 1 ? "that photo" : `${fmtNum(moved.bumped.length)} of them`}: that face went to ${[...new Set(where)].join(", ")}.`
      : "";
    setMessage({
      text: moved.faces ? `${what} moved${from ? ` from ${from.name ?? "this person"}` : ""} to ${to}.${room}` : `Already ${to}.`,
      action: moved.undo.length
        ? {
            label: "Undo",
            run: () =>
              run(
                people.restore(moved.undo).then(() => {
                  setMessage({ text: moved.faces === 1 ? "The face is back where it was." : "The faces are back where they were." });
                  refresh();
                }),
              ),
          }
        : undefined,
    });
    refresh();
  };

  /** People made one: says who, with Undo (they are apart again). `from`: merged
   *  from that person's page, which now shows the person they joined. */
  const peopleMerged = (m: Merged, from?: number) => {
    if (!m.joined.length) return;
    setMessage({ text: mergedText(m), action: { label: "Undo", run: () => unmerge(m, from) } });
  };

  /** Undo of a merge. Merged from a person's page and still on the page of the
   *  person they joined: back to their page - or to People, should their id
   *  have gone to someone new since (they come back under another one). */
  const unmerge = useLatest((m: Merged, from?: number) => {
    const free = from != null && !peopleList.some((p) => p.id === from);
    const toPeople = trail[trail.length - 1]?.view === "people";
    const onJoined = () => scopeRef.current.kind === "person" && scopeRef.current.id === m.into;
    run(
      people.unmerge(m).then(async () => {
        setMessage({ text: unmergedText(m) });
        if (from == null || !onJoined() || (!free && !toPeople)) return refresh();
        // Their page once People knows them again: never an "Unnamed person" in between.
        await loadCatalog();
        if (!onJoined()) return;
        if (free) setScope({ kind: "person", id: from });
        else goBack();
      }),
    );
  });

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

  // Ctrl+B folds or unfolds the sidebar, anywhere but in a text box.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey || e.key.toLowerCase() !== "b") return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      e.preventDefault();
      setFolded((f) => !f);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setFolded]);

  // Escape closes the calendar first.
  useEffect(() => {
    if (!calendar || viewerId != null || deleteOpen || settingsOpen || straysFor || tagging || reassigning || datesOpen) return;
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (e.key !== "Escape" || (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA"))) return;
      setCalendar(false);
      setGridFocus((n) => n + 1);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [calendar, viewerId, deleteOpen, settingsOpen, straysFor, tagging, reassigning, datesOpen]);

  // (Cull has its own Escape: Done.)
  useEffect(() => {
    if (
      !trail.length ||
      view === "cull" ||
      calendar ||
      viewerId != null ||
      deleteOpen ||
      settingsOpen ||
      straysFor ||
      tagging ||
      reassigning ||
      datesOpen
    )
      return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      goBack();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [trail, view, calendar, viewerId, deleteOpen, settingsOpen, straysFor, tagging, reassigning, goBack]);

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

  // Gear: one name per camera or lens. A view or filter on the old name follows it.
  const followGear = (kind: GearKind, from: string, to: string[]) => {
    setScope((s) => (s.kind === kind && s.name === from ? { kind, name: to[0] } : s));
    const key = kind === "camera" ? "cameras" : "lenses";
    setFilters((f) =>
      f[key].includes(from) ? { ...f, [key]: [...new Set(f[key].flatMap((v) => (v === from ? to : [v])))] } : f,
    );
  };
  const renameGear = async (kind: GearKind, name: string, to: string) => {
    const items = (kind === "camera" ? browse?.gear.cameras : browse?.gear.lenses) ?? [];
    // Onto another one's name (any case) = merge, as the backend does.
    const typed = to.trim().split(/\s+/).join(" ").toLowerCase();
    const into = items.find((i) => i.name !== name && !i.unknown && i.name.toLowerCase() === typed);
    const merged = into ? into.count + (items.find((i) => i.name === name)?.count ?? 0) : 0;
    try {
      const shown = await gearApi.rename(kind, name, to);
      followGear(kind, name, [shown]);
      setMessage({
        text: into
          ? `“${name}” and “${shown}” are one ${kind} now (${fmtNum(merged)} photos). Its menu splits them again.`
          : `“${name}” is now “${shown}”, in Shoebox only: the photo files are not changed.`,
      });
      refresh();
    } catch (e) {
      setMessage({ text: String(e), warn: true });
    }
  };
  const splitGear = async (kind: GearKind, name: string) => {
    try {
      const names = await gearApi.split(kind, name);
      followGear(kind, name, names);
      setMessage({
        text:
          names.length > 1
            ? `“${name}” is split into the names in its files: ${names.join(", ")}.`
            : `“${name}” goes by the name in its files again: ${names[0]}.`,
      });
      refresh();
    } catch (e) {
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

  useEffect(() => {
    const show = (s: UpdateStatus | null) => {
      setUpdate(s);
      if (s?.newer && s.latest && announced.current !== s.latest) {
        announced.current = s.latest;
        setMessage({
          text: `Shoebox ${s.latest} is out (this is ${s.current}).`,
          action: { label: "Get it", run: () => run(updatesApi.openPage()) },
        });
      }
    };
    updatesApi.status().then(show).catch(() => {});
    const un = listen<UpdateStatus>("update-status", (e) => show(e.payload));
    return () => {
      un.then((f) => f());
    };
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

  // Missing photos deleted outside Shoebox: out of the catalog, files already gone.
  const forgetMissing = async (ids: number[]) => {
    const n = ids.length;
    if (n === 0) return;
    const one = n === 1;
    const yes = await ask(
      `Take ${one ? "this missing photo" : `these ${fmtNum(n)} missing photos`} out of the catalog?\n\n` +
        `${one ? "Its file is" : "Their files are"} already gone from ${one ? "its disk" : "their disks"}: nothing on any disk is touched. ` +
        `Shoebox forgets ${one ? "it" : "them"} with ${one ? "its" : "their"} preview${one ? "" : "s"}, faces and tags. ` +
        `If a file turns up again, the next scan adds it back as a new photo.`,
      { title: "Remove from catalog", kind: "warning", okLabel: "Remove", cancelLabel: "Keep" },
    );
    if (!yes) return;
    try {
      const r = await api.forgetMissing(ids);
      setSelected(new Set());
      setMessage({ text: `${fmtNum(r.photos)} missing photo${r.photos === 1 ? "" : "s"} removed from the catalog.` });
      refresh();
    } catch (e) {
      setMessage({ text: String(e), warn: true });
    }
  };

  // A search shows its clear matches unless "show more" was pressed.
  const shown = useMemo(
    () => (loaded.query && !showPossible ? headOf(list, clearCount) : list),
    [list, loaded.query, showPossible, clearCount],
  );
  // Sideways: each photo's suggested turn. Its photos are drawn turned that way (laid
  // out at that shape) to look them over, or as they are: one switch for the grid,
  // the viewer and its filmstrip (V).
  const sidewaysView = scope.kind === "sideways" && view === "photos";
  const [asTurned, setAsTurned] = useState(true);
  const suggestions = useMemo(
    () => (sidewaysView ? new Map(shown.ids.map((id, i) => [id, shown.sideways[i]] as const).filter(([, t]) => t > 0)) : null),
    [sidewaysView, shown],
  );
  const turnedShown = asTurned ? suggestions : null;
  // V in Sideways: as they are, or as they would be turned.
  useEffect(() => {
    if (!sidewaysView || viewerId != null) return;
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if ((e.key !== "v" && e.key !== "V") || e.ctrlKey || e.metaKey || e.altKey) return;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      setAsTurned((on) => !on);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sidewaysView, viewerId]);

  const gridList = useMemo(
    () =>
      turnedShown
        ? {
            ...shown,
            w: shown.w.map((w, i) => (shown.sideways[i] % 2 ? shown.h[i] : w)),
            h: shown.h.map((h, i) => (shown.sideways[i] % 2 ? shown.w[i] : h)),
          }
        : shown,
    [shown, turnedShown],
  );
  // On a person's page: photos they were found in since you last looked at them.
  const freshHere = useMemo(() => (current ? shown.fresh.filter((f) => f === 1).length : 0), [shown, current]);
  // ...and "Only new" there: just those photos.
  const newOnlyHere = scope.kind === "person" && !!scope.newOnly;
  const viewerIndex = useMemo(() => (viewerId == null ? -1 : shown.ids.indexOf(viewerId)), [viewerId, shown]);
  // Sideways is a review: a photo turned or left there leaves the list, and the
  // viewer goes on to the one that took its place (closing after the last).
  const viewerAt = useRef(-1);
  useEffect(() => {
    if (viewerIndex >= 0) viewerAt.current = viewerIndex;
    else if (viewerId != null && sidewaysView && current)
      setViewerId(shown.ids.length ? shown.ids[Math.min(viewerAt.current, shown.ids.length - 1)] : null);
  }, [viewerIndex, viewerId, sidewaysView, current, shown]);
  // Selected photos that are missing everywhere: "Remove from catalog" takes them.
  const missingSelected = useMemo(
    () => (current && selected.size > 0 ? shown.ids.filter((id, i) => shown.state[i] === 0 && selected.has(id)) : []),
    [current, selected, shown],
  );
  // Selected photos that are private (listed only while their space is open).
  const privateSelected = useMemo(
    () => (unlocked && selected.size > 0 ? shown.ids.filter((id, i) => shown.private[i] === 1 && selected.has(id)) : []),
    [unlocked, selected, shown],
  );
  const focusIndex = useMemo(() => (focusId == null ? null : shown.ids.indexOf(focusId)), [focusId, shown]);
  // Stable, so the grid isn't drawn again for a scan's progress or a letter typed in the search box.
  const openPhoto = useLatest((i: number) => setViewerId(shown.ids[i]));
  const selectPhotos = useLatest((cursor: number, sel: Set<number>) => {
    setFocusId(cursor >= 0 ? shown.ids[cursor] : null);
    setSelected(sel);
  });
  const topPhoto = useLatest((i: number) => {
    gridTop.current = i;
  });

  // A new place starts with nothing selected, or with the photos it was reached from.
  const selectOnArrival = useRef<number[] | null>(null);
  useEffect(() => {
    const ids = selectOnArrival.current;
    selectOnArrival.current = null;
    setSelected(new Set(ids ?? []));
  }, [scope, query, filters]);

  /** Closes the viewer; the photos selected in its filmstrip stay selected in the grid. */
  const closeViewer = (picked?: number[]) => {
    setFocusId(viewerId);
    if (viewerId != null) setSelected(new Set(picked?.length ? picked : [viewerId]));
    setViewerId(null);
    setGridFocus((n) => n + 1);
    if (facesChanged.current) {
      facesChanged.current = false;
      refresh();
    }
  };

  /** From the viewer: the folder a copy of the photo is in, with the photo
   *  selected there. Back (Esc) returns to the photo. */
  const goToFolder = (copy: FileLoc) => {
    const to = folderScope(overview, copy.volumeId, copy.folder);
    if (sameScope(to, scope) && !query) return closeViewer();
    const id = viewerId;
    drill(to);
    setTyped("");
    setQuery("");
    setFocusId(id);
    selectOnArrival.current = id == null ? null : [id];
    setGridFocus((n) => n + 1);
    facesChanged.current = false; // the new place is loaded afresh
  };

  /** From "Check dates": the photos' folder, those photos selected (Fix dates is then one click). */
  const showDates = (f: DateFinding) => {
    const to = folderScope(overview, f.volumeId, f.relPath);
    setFocusId(f.photos[0]);
    setGridFocus((n) => n + 1);
    if (sameScope(to, scope) && !query && filters === NO_FILTERS) {
      setSelected(new Set(f.photos));
      return;
    }
    drill(to);
    setTyped("");
    setQuery("");
    setFilters(NO_FILTERS);
    selectOnArrival.current = f.photos;
  };
  /** "Fix dates" done: said in the status bar, with Undo. */
  const datesMoved = (done: Shifted, how: DateChange) => {
    setFixingDates(null);
    setGridFocus((n) => n + 1);
    const n = done.photos.length;
    const photosText = n === 1 ? "1 photo" : `${fmtNum(n)} photos`;
    setMessage({
      text: how.kind === "back" && n ? `${photosText} back on their files' dates.` : movedText(done),
      action: n
        ? {
            label: "Undo",
            run: () =>
              run(
                datesApi.undo(done).then(() => {
                  setMessage({ text: n === 1 ? "The photo's date is as before." : "The photos' dates are as before." });
                  refresh();
                }),
              ),
          }
        : undefined,
    });
    refresh();
  };

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
    if (scope.kind === "sideways") return { title: "Sideways" };
    if (scope.kind === "private") return { title: "Private", sub: `Hidden everywhere once you lock (${PRIVATE_SHORTCUT})` };
    if (scope.kind === "date") return { title: fmtDateKey(scope.key) };
    if (scope.kind === "camera" || scope.kind === "lens") {
      const items = (scope.kind === "camera" ? browse?.gear.cameras : browse?.gear.lenses) ?? [];
      const item = items.find((i) => i.name === scope.name);
      return { title: scope.name, sub: item?.inFiles.length ? `In the files: ${item.inFiles.join(" · ")}` : undefined };
    }
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
  }, [scope, overview, peopleList, browse]);

  const empty = overview && overview.volumes.length === 0;
  // What Cull would work on: the photos shown, as filtered.
  const cullTitle = useMemo(() => {
    if (query) return "Cull works on a folder or a day, not on search results: clear the search first";
    const n = shown.ids.length;
    const what =
      scope.kind === "folder" || scope.kind === "root"
        ? "of this folder and its subfolders"
        : scope.kind === "volume"
          ? "of this disk"
          : scope.kind === "date"
            ? `of ${fmtDateKey(scope.key)}`
            : scope.kind === "camera" || scope.kind === "lens"
              ? `taken with ${scope.name}`
              : scope.kind === "person"
                ? `of ${scopeInfo.title}`
                : scope.kind === "marked"
                  ? "marked for deletion"
                  : scope.kind === "missing"
                    ? "missing"
                    : scope.kind === "sideways"
                      ? "that look sideways"
                      : scope.kind === "private"
                        ? "that are private"
                        : "in the catalog";
    const filtered = filtersOn(filters) ? ", as the filters narrow them" : "";
    if (current && n > CULL_MAX)
      return `Cull works on up to ${fmtNum(CULL_MAX)} photos at once; this view has ${fmtNum(n)}: pick a folder or a day, or narrow it with the filters`;
    return `Cull the ${current ? `${fmtNum(n)} ` : ""}photos ${what}${filtered}: bursts gathered, what's blurry or badly exposed, what to keep. Delete marks a photo for deletion in the grid too.`;
  }, [query, shown, scope, scopeInfo.title, filters, current]);
  // The people People shows without asking: not ignored, not seen only once.
  const shownPeople = useMemo(() => peopleList.filter((p) => !p.ignored && (p.name || p.photos >= 2)), [peopleList]);
  const peopleNew = useMemo(() => shownPeople.filter((p) => p.isNew || p.newPhotos > 0).length, [shownPeople]);
  const sidebarBrowse = useMemo(() => (browse && sort === "newest" ? newestFirst(browse) : browse), [browse, sort]);
  // People's handlers. From "Only new", straight to what's new in them.
  const openPerson = useLatest((p: Person) =>
    drill({ kind: "person", id: p.id, ...(peopleNewOnly && (p.isNew || p.newPhotos > 0) ? { newOnly: true } : {}) }),
  );
  const peopleError = useLatest((e: string) => setMessage({ text: e, warn: true }));
  const mergedInPeople = useLatest((m: Merged) => peopleMerged(m));
  const openModels = useLatest(() => setSettingsTab("models"));
  const flatLabel = loaded.query
    ? `Best matches for "${loaded.query}"`
    : loaded.sort === "name"
      ? "By file name"
      : loaded.sort === "added"
        ? "Most recently added first"
        : undefined;

  return (
    <div className={`app ${folded ? "folded" : ""}`}>
      <Sidebar
        folded={folded}
        onFold={setFolded}
        overview={overview}
        browse={sidebarBrowse}
        version={version}
        update={update?.newer ? update.latest : null}
        onUpdate={() => run(updatesApi.openPage())}
        scope={scope}
        scanningRoot={progress?.rootId ?? null}
        queuedRoots={queued}
        onRegenerate={(id) => regenerate(id)}
        onRemove={removeFolder}
        people={shownPeople.length}
        peopleNew={peopleNew}
        peopleOpen={view === "people"}
        onPeople={(newOnly = false) => {
          setTrail([]);
          setPersonTyped("");
          setPeopleNewOnly(newOnly);
          setCalendar(false);
          setView("people");
        }}
        onScope={(s) => {
          setTrail([]);
          setCalendar(false);
          setView("photos");
          setScope(s);
          setFocusId(null);
        }}
        privateUnlocked={unlocked}
        onLockPrivate={lockPrivate}
        onMakePrivate={makeFolderPrivate}
        onAddFolder={addFolder}
        onRescan={(id) => run(api.rescan(id))}
        onCheckDates={(t) => setDatesCheck({ where: { volumeId: t.v.id, relPath: t.relPath, name: t.name } })}
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
        onGearRename={renameGear}
        onGearSplit={splitGear}
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
                    const from = personHere.id;
                    run(
                      people.merge([from, into.id], into.id).then((m) => {
                        setScope({ kind: "person", id: into.id });
                        loadCatalog(); // People without them
                        peopleMerged(m, from);
                      }),
                    );
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
                      <span className="ellipsis">{personHere.name ?? "Unnamed person"}</span> <Pencil size={14} />
                    </button>
                  </h1>
                  <button
                    className="btn small ghost tool-ignore"
                    onClick={() => ignorePerson(personHere)}
                    title={personHere.ignored ? "Show them in People again" : "A stranger: hide them from People"}
                  >
                    {personHere.ignored ? <Eye size={14} /> : <EyeOff size={14} />}
                    <span className="btn-label">{personHere.ignored ? "Show again" : "Ignore"}</span>
                  </button>
                </>
              )
            ) : (
              <h1>{view === "people" ? "People" : view === "cull" ? `Cull · ${scopeInfo.title}` : scopeInfo.title}</h1>
            )}
            <span className="muted">
              {view === "cull"
                ? ""
                : view === "people"
                ? personFound
                  ? `${fmtNum(personFound.length)} found`
                  : `${fmtNum(shownPeople.length)} people`
                : slow
                  ? "Loading…"
                  : `${fmtNum(shown.ids.length)} ${query ? "matches" : newOnlyHere ? "new photos" : "photos"}`}
              {view === "photos" && personHere && freshHere > 0 && !newOnlyHere && (
                <b className="new-count"> · {fmtNum(freshHere)} new</b>
              )}
            </span>
            {view !== "people" && "sub" in scopeInfo && scopeInfo.sub && <span className="muted path">{scopeInfo.sub}</span>}
          </div>
          {/* On People the box finds a person by name; elsewhere it searches photos in words. */}
          {view === "people" ? (
            <PersonSearch
              value={personTyped}
              found={personFound}
              onChange={setPersonTyped}
              onOpen={(p) => drill({ kind: "person", id: p.id })}
            />
          ) : view === "cull" ? (
            <span className="grow" />
          ) : (
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
          )}
          <div className="tools">
            {view === "photos" && scope.kind === "person" && (
              <button
                className="btn ghost tool-person"
                onClick={() => setStraysFor(peopleList.find((p) => p.id === scope.id) ?? null)}
                title="Find faces in this person that don't look like them, and move them out"
              >
                <UserCog size={15} />
                <span className="btn-label">Clean up faces…</span>
              </button>
            )}
            {view === "photos" && scope.kind === "person" && (
              <button
                className="btn ghost tool-person"
                onClick={() => setMoreFor(peopleList.find((p) => p.id === scope.id) ?? null)}
                title="Find faces in other people that look more like them, and move them here"
              >
                <UserSearch size={15} />
                <span className="btn-label">Find more…</span>
              </button>
            )}
            {view === "photos" && !empty && (
              <button
                className="btn ghost"
                onClick={() => {
                  setViewerId(null);
                  setCalendar(false);
                  setView("cull");
                }}
                disabled={!!query || (current && shown.ids.length > CULL_MAX)}
                title={cullTitle}
              >
                <ListChecks size={15} /> Cull
              </button>
            )}
            <button
              className="btn ghost tool-scan"
              onClick={() => run(api.rescanAll())}
              title="Scan every folder whose disk is plugged in"
            >
              <RefreshCw size={15} />
              <span className="btn-label">Scan plugged-in disks</span>
            </button>
          </div>
        </header>

        {view === "photos" && !empty && (
          <div className="viewbar">
            <FilterBar
              facets={facets}
              filters={filters}
              onChange={setFilters}
              people={peopleList}
              lead={
                scope.kind === "person" &&
                (freshHere > 0 || newOnlyHere) && (
                  <button
                    className={`filter-chip ${newOnlyHere ? "on" : ""}`}
                    aria-pressed={newOnlyHere}
                    onClick={() => setScope(newOnlyHere ? { kind: "person", id: scope.id } : { ...scope, newOnly: true })}
                    title={
                      newOnlyHere
                        ? `Show all their photos (${fmtNum(personHere?.photos ?? 0)})`
                        : "Only the photos they were found in since you last looked at them, to check them quickly"
                    }
                  >
                    <Sparkles size={14} /> Only new
                  </button>
                )
              }
            />
            <div className="view-tools">
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
              <span className="vsep" />
              <button
                className={`icon-btn type-toggle ${calendar ? "on" : ""}`}
                onClick={() => {
                  setCalendar(!calendar);
                  if (calendar) setGridFocus((n) => n + 1);
                }}
                title={calendar ? "Back to the photos (Esc)" : "Calendar: every day you took photos; click a day to go there"}
                aria-pressed={calendar}
              >
                <CalendarDays size={16} />
              </button>
              <button
                className={`icon-btn type-toggle ${showType ? "on" : ""}`}
                onClick={() => setShowType(!showType)}
                title={showType ? "Hide the file type on thumbnails" : "Show the file type (CR3, JPG…) on thumbnails"}
                aria-pressed={showType}
              >
                <FileType size={16} />
              </button>
              <button
                className={`icon-btn type-toggle ${showProblems ? "on" : ""}`}
                onClick={() => setShowProblems(!showProblems)}
                title={
                  showProblems
                    ? "Hide what Cull found on thumbnails"
                    : "Show what Cull found (blurry, face out of focus, dark...) on thumbnails"
                }
                aria-pressed={showProblems}
              >
                <Focus size={16} />
              </button>
              <span className="vsep" />
              <input
                type="range"
                min={110}
                max={380}
                value={rowHeight}
                onChange={(e) => setRowHeight(+e.target.value)}
                title="Thumbnail size"
              />
            </div>
          </div>
        )}
        {view === "photos" && current && filtersOn(filters) && list.ids.length === 0 && (
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
        {query && current && list.ids.length > clearCount && (
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
            <span>
              These photos weren't where Shoebox last saw them, on a disk that was plugged in. They stay in the catalog: if
              they turn up on any disk, the next scan finds them again.
              {inBin && inBin.size > 0 && (
                <>
                  {" "}
                  <b>
                    {inBin.size === list.ids.length
                      ? list.ids.length === 1
                        ? "It is"
                        : "All of them are"
                      : `${fmtNum(inBin.size)} of them ${inBin.size === 1 ? "is" : "are"}`}{" "}
                    in the {BIN}
                  </b>
                  : deleted, not moved. Restore {inBin.size === 1 ? "it" : "them"} from the {BIN} to keep{" "}
                  {inBin.size === 1 ? "it" : "them"} with faces and tags, or remove {inBin.size === 1 ? "it" : "them"}{" "}
                  from the catalog.
                </>
              )}
            </span>
            <span className="grow" />
            {inBin && inBin.size > 0 && inBin.size < list.ids.length && (
              <button className="btn small ghost" onClick={() => forgetMissing([...inBin.keys()])}>
                Remove the {fmtNum(inBin.size)} in the {BIN}…
              </button>
            )}
            {list.ids.length > 0 && (
              <button
                className="btn small danger"
                disabled={!current}
                onClick={() => forgetMissing(list.ids)}
                title="Take these photos out of the catalog: for files deleted outside Shoebox. Nothing on any disk is touched."
              >
                <ArchiveX size={14} /> Remove {list.ids.length === 1 ? "it" : `all ${fmtNum(list.ids.length)}`} from catalog…
              </button>
            )}
          </div>
        )}
        {sidewaysView && (
          <div className="banner">
            <TurnPhotoIcon size={15} />
            <span>
              These photos look sideways or upside down. Only Shoebox's copies turn, never your files.
            </span>
            <div className="view-switch in-banner" role="group" aria-label="Show">
              <button className={asTurned ? "" : "on"} onClick={() => setAsTurned(false)} title="Show them as they are now (V)">
                As they are
              </button>
              <button
                className={asTurned ? "on" : ""}
                onClick={() => setAsTurned(true)}
                title="Show them as Shoebox would turn them; nothing is turned until you say so (V)"
              >
                As Shoebox would turn them
              </button>
            </div>
            <span className="grow" />
            {shown.ids.length > 0 && (
              <button className="btn small primary" disabled={!current} onClick={() => turnPhotos(shown.ids, "suggested")}>
                Turn {shown.ids.length === 1 ? "it" : `all ${fmtNum(shown.ids.length)}`} as shown
              </button>
            )}
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

        {view === "cull" ? (
          <CullView
            scope={scope}
            filters={filters}
            title={scopeInfo.title}
            onClose={closeCull}
            onChanged={cullChanged}
            onError={cullError}
          />
        ) : view === "people" ? (
          <PeopleView
            people={peopleList}
            found={personFound}
            query={personTyped}
            newOnly={peopleNewOnly}
            onNewOnly={setPeopleNewOnly}
            onOpen={openPerson}
            onChanged={refresh}
            onMerged={mergedInPeople}
            onError={peopleError}
            onOpenModels={openModels}
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
          // Until the view's photos are here the last ones stay (not clickable),
          // dimmed if it takes a moment; the grid is rebuilt when they arrive.
          <div className={`grid-stack ${current ? "" : "stale"} ${slow ? "slow" : ""}`}>
            <PhotoGrid
              key={loaded.key}
              list={gridList}
              rowHeight={gridRowHeight}
              onOpen={openPhoto}
              onSelect={selectPhotos}
              onMark={setMarked}
              onTag={setTagging}
              onTurn={sidewaysView ? undefined : turnPhotos}
              turned={turnedShown}
              previewTurn={turnPreview}
              selected={selected}
              binned={inBin}
              keyboard={
                current &&
                viewerIndex < 0 &&
                !calendar &&
                !deleteOpen &&
                !settingsOpen &&
                !privateAsk &&
                !straysFor &&
                !tagging &&
                !reassigning &&
                !datesOpen
              }
              focusRequest={gridFocus}
              focusIndex={focusIndex}
              showType={showType}
              showProblems={showProblems}
              flatLabel={flatLabel}
              jump={jump}
              onTopPhoto={topPhoto}
            />
            {/* What can be done with the selected photos: over the grid, so selecting moves nothing. */}
            {selected.size > 0 && !calendar && (
              <div className="selection-bar" role="group" aria-label="Selected photos">
                <span className="selection-count">{fmtNum(selected.size)} selected</span>
                <button
                  className="icon-btn"
                  onClick={() => {
                    setSelected(new Set());
                    setGridFocus((n) => n + 1);
                  }}
                  aria-label="Clear selection"
                  title="Select none (Esc)"
                >
                  <X size={15} />
                </button>
                <span className="vsep" />
                <button
                  className="btn ghost"
                  onClick={() => setTagging(shown.ids.filter((id) => selected.has(id)))}
                  title="Tag the selected photos (T)"
                >
                  <TagIcon size={15} /> Tag…
                </button>
                <button
                  className="btn ghost"
                  onClick={() => setFixingDates(shown.ids.filter((id) => selected.has(id)))}
                  title="Move their dates in Shoebox (a camera clock set wrong): the files keep their own"
                >
                  <CalendarClock size={15} /> Fix dates…
                </button>
                {sidewaysView ? (
                  <>
                    <button
                      className="btn ghost"
                      onClick={() => leaveUpright(shown.ids.filter((id) => selected.has(id)))}
                      title="These stand right as they are: never suggested again"
                    >
                      Leave {selected.size === 1 ? "it" : fmtNum(selected.size)} as {selected.size === 1 ? "it is" : "they are"}
                    </button>
                    <button
                      className="btn ghost"
                      onClick={() => {
                        turnPhotos(shown.ids.filter((id) => selected.has(id)), "suggested");
                        setSelected(new Set());
                      }}
                    >
                      <TurnPhotoIcon size={15} /> Turn {selected.size === 1 ? "it" : fmtNum(selected.size)} as shown
                    </button>
                  </>
                ) : (
                  <TurnControl
                    on="toolbar"
                    what={selected.size === 1 ? "the selected photo" : `the ${fmtNum(selected.size)} selected photos`}
                    onTurn={(by) => turnPhotos(shown.ids.filter((id) => selected.has(id)), by)}
                    onPreview={setTurnPreview}
                  />
                )}
                {personHere && (
                  <button
                    className="btn ghost"
                    onClick={() => notThem(personHere)}
                    title={`Not ${personHere.name ?? "this person"} in ${selected.size === 1 ? "this photo" : "these photos"}: say who it is`}
                  >
                    <UserX size={15} /> Not {personHere.name?.split(" ")[0] ?? "them"}…
                  </button>
                )}
                {unlocked && privateSelected.length < selected.size && (
                  <button
                    className="btn ghost"
                    onClick={() => makePrivate(shown.ids.filter((id, i) => selected.has(id) && shown.private[i] !== 1), true)}
                    title="Hidden from every view, search, People and agent once you lock"
                  >
                    <LockKeyhole size={15} /> Make private
                  </button>
                )}
                {unlocked && privateSelected.length > 0 && (
                  <button
                    className="btn ghost"
                    onClick={() => makePrivate(privateSelected, false)}
                    title="Ordinary photos again: shown everywhere, locked or not"
                  >
                    <LockKeyholeOpen size={15} /> Not private
                  </button>
                )}
                {missingSelected.length > 0 && (
                  <button
                    className="btn ghost"
                    onClick={() => forgetMissing(missingSelected)}
                    title="Take the selected missing photos out of the catalog: for files deleted outside Shoebox"
                  >
                    <ArchiveX size={15} /> Remove{missingSelected.length > 1 ? ` ${fmtNum(missingSelected.length)}` : ""} from
                    catalog…
                  </button>
                )}
              </div>
            )}
            {calendar && (
              <CalendarView
                list={shown}
                newestFirst={loaded.sort === "newest" && !loaded.query}
                startIndex={focusIndex != null && focusIndex >= 0 ? focusIndex : gridTop.current}
                onPick={(i) => {
                  setCalendar(false);
                  setFocusId(shown.ids[i]);
                  setSelected(new Set([shown.ids[i]]));
                  setJump((j) => ({ index: i, seq: (j?.seq ?? 0) + 1 }));
                  setGridFocus((n) => n + 1);
                }}
              />
            )}
          </div>
        )}

        <StatusBar
          overview={overview}
          progress={progress}
          aiProgress={aiProgress}
          queued={queued.length}
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
          onForget={() => forgetMissing([shown.ids[viewerIndex]])}
          onTagsChanged={refreshSoon}
          onPrivate={unlocked ? (isPrivate) => makePrivate([shown.ids[viewerIndex]], isPrivate) : undefined}
          onTurn={(ids, by) => turnPhotos(ids, by)}
          suggested={suggestions}
          showTurned={asTurned}
          onShowTurned={setAsTurned}
          onLeave={leaveUpright}
          onPerson={(id) => drill({ kind: "person", id })}
          onFolder={goToFolder}
          everyone={peopleList}
          onFacesChanged={() => {
            // The grid waits: on a person's page this photo may leave it.
            facesChanged.current = true;
            people
              .list()
              .then(setPeopleList)
              .catch(() => {});
          }}
          onClose={closeViewer}
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
          everyone={peopleList}
          onClose={() => setStraysFor(null)}
          onDone={(moved, to) => {
            facesMoved(moved, to, straysFor);
            setStraysFor(null);
          }}
        />
      )}
      {moreFor && (
        <FindMoreDialog
          person={moreFor}
          everyone={peopleList}
          onClose={() => setMoreFor(null)}
          onDone={(moved, to) => {
            facesMoved(moved, to, null);
            setMoreFor(null);
          }}
        />
      )}
      {reassigning && (
        <WhoIsIt
          faceIds={reassigning.faceIds}
          from={reassigning.from}
          everyone={peopleList}
          onClose={() => {
            setReassigning(null);
            setGridFocus((n) => n + 1);
          }}
          onDone={(moved, to) => {
            facesMoved(moved, to, reassigning.from);
            setReassigning(null);
            setSelected(new Set());
            setGridFocus((n) => n + 1);
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
          update={update}
          onRegenerateAll={() => regenerate(null)}
          onCheckDates={() => {
            setSettingsTab(null);
            setDatesCheck({ where: null });
          }}
          privateStatus={privateStatus}
          onPrivateStatus={privateChanged}
          onUnlockPrivate={() => setPrivateAsk({ mode: "unlock" })}
          onClose={() => setSettingsTab(null)}
        />
      )}
      {datesCheck && (
        <DatesCheckDialog
          where={datesCheck.where}
          onChanged={refreshSoon}
          onShow={showDates}
          onClose={() => {
            setDatesCheck(null);
            setGridFocus((n) => n + 1);
          }}
        />
      )}
      {fixingDates && (
        <FixDatesDialog
          ids={fixingDates}
          onDone={datesMoved}
          onClose={() => {
            setFixingDates(null);
            setGridFocus((n) => n + 1);
          }}
        />
      )}
      {privateAsk && (
        <PrivateDialog
          mode={privateAsk.mode}
          onUnlocked={privateChanged}
          onConfirmed={async (password) => {
            if (privateAsk.mode === "confirm") await privateAsk.hide(password);
            setPrivateAsk(null);
          }}
          onClose={() => setPrivateAsk(null)}
        />
      )}
    </div>
  );
}
