import { invoke } from "@tauri-apps/api/core";

export type Scope =
  | { kind: "all" }
  | { kind: "missing" }
  | { kind: "volume"; id: number }
  | { kind: "root"; id: number }
  | { kind: "folder"; volumeId: number; relPath: string }
  /** "2026", "2026-09" or "unknown" */
  | { kind: "date"; key: string }
  /** A camera or a lens, by the name shown (Gear). */
  | { kind: "camera"; name: string }
  | { kind: "lens"; name: string }
  /** `newOnly`: only the photos they were found in since you last looked at them. */
  | { kind: "person"; id: number; newOnly?: boolean }
  /** Marked for deletion. */
  | { kind: "marked" };

export const sameScope = (a: Scope, b: Scope) => JSON.stringify(a) === JSON.stringify(b);

/** How the values ticked in one filter combine: a photo with any of them, or only with all of them. */
export type Match = "any" | "all";

/**
 * Narrows any view. Within a filter any ticked value matches, except where a photo can hold
 * several values (people, tags): those can ask for all of them. Across filters all must.
 */
export interface Filters {
  /** "2026", "2026-09", "2026-09-26" or "unknown" */
  dates: string[];
  cameras: string[];
  lenses: string[];
  /** Extensions, lower case: "cr3", "jpg" */
  types: string[];
  status: ("online" | "offline" | "missing")[];
  /** Person ids. */
  people: number[];
  /** Photos with any of `people` in, or all of them together. */
  peopleMatch: Match;
  /** Tag ids. */
  tags: number[];
  /** Photos with all of `tags` (tags narrow down), or any of them. */
  tagsMatch: Match;
}
export const NO_FILTERS: Filters = {
  dates: [],
  cameras: [],
  lenses: [],
  types: [],
  status: [],
  people: [],
  peopleMatch: "any",
  tags: [],
  tagsMatch: "all",
};
export const filtersOn = (f: Filters) =>
  f.dates.length + f.cameras.length + f.lenses.length + f.types.length + f.status.length + f.people.length + f.tags.length >
  0;

/** A tag; `count` = photos with it (in the catalog, or among the photos asked about). */
export interface Tag {
  id: number;
  name: string;
  count: number;
}

/** Tags live in the catalog only - never written into photo files. */
export const tagsApi = {
  all: () => invoke<Tag[]>("tags_all"),
  on: (photoIds: number[]) => invoke<Tag[]>("tags_on", { photoIds }),
  add: (photoIds: number[], names: string[]) => invoke<Tag[]>("tags_add", { photoIds, names }),
  remove: (photoIds: number[], tagIds: number[]) => invoke<number>("tags_remove", { photoIds, tagIds }),
  rename: (tagId: number, name: string) => invoke<void>("tag_rename", { tagId, name }),
  delete: (tagId: number) => invoke<void>("tag_delete", { tagId }),
};

export interface FacetValue {
  value: string;
  count: number;
}
/** What each filter offers in the current view, with counts (each counted with the other filters applied). */
export interface Facets {
  /** Days ("2026-09-26") and "unknown", oldest first. */
  dates: FacetValue[];
  cameras: FacetValue[];
  lenses: FacetValue[];
  types: (FacetValue & { raw: boolean })[];
  status: FacetValue[];
  /** Named people first, then unnamed ones seen in 2+ photos. */
  people: { id: number; name: string | null; cover: number; count: number }[];
  /** Tags on the photos shown, most used first. */
  tags: Tag[];
}

/** Grid order: capture time (oldest/newest first, with month headers), file name, recently added. */
export type Sort = "oldest" | "newest" | "name" | "added";
export const SORTS: [Sort, string][] = [
  ["oldest", "Oldest first"],
  ["newest", "Newest first"],
  ["name", "File name"],
  ["added", "Recently added"],
];

export interface FolderNode {
  name: string;
  relPath: string;
  volumeId: number;
  count: number;
  children: FolderNode[];
}

/** A camera or lens, one name whatever its files call it. */
export interface GearItem {
  /** As shown everywhere. */
  name: string;
  count: number;
  /** What the files call it, when that isn't just its name (several spellings grouped, or renamed). */
  inFiles: string[];
  /** Renamed or merged by hand: it can be split back into the files' names. */
  byHand: boolean;
  /** "Unknown camera" / "Unknown lens": photos that don't say. Never renamed. */
  unknown: boolean;
}
export type GearKind = "camera" | "lens";

/** Gear names live in the catalog only - never written into photo files. */
export const gearApi = {
  /** Renames a camera or lens; onto a name already shown = merge. Returns the name it now goes by. */
  rename: (kind: GearKind, name: string, to: string) => invoke<string>("gear_rename", { kind, name, to }),
  /** Back to the names in the files, each kept apart. */
  split: (kind: GearKind, name: string) => invoke<string[]>("gear_split", { kind, name }),
};

export interface Browse {
  years: { year: string; count: number; months: { key: string; month: number; count: number }[] }[];
  unknownDate: number;
  gear: { cameras: GearItem[]; lenses: GearItem[] };
  folders: { rootId: number; children: FolderNode[] }[];
}

export interface Stats {
  photos: number;
  raw: number;
  other: number;
  noPreview: number;
  files: number;
  missingFiles: number;
  folders: number;
  disks: number;
  dbBytes: number;
  previewBytes: number;
  thumbBytes: number;
  totalBytes: number;
  /** What reading the catalog may use now, in MB (automatic values resolved). */
  memory: { cacheMb: number; mapMb: number };
}

export interface RootInfo {
  id: number;
  /** False when the disk is plugged in but the folder isn't there. */
  found: boolean;
  name: string;
  path: string;
  relPath: string;
  lastScan: number | null;
  photos: number;
}

export interface VolumeInfo {
  id: number;
  label: string;
  online: boolean;
  mount: string | null;
  lastMount: string | null;
  photos: number;
  roots: RootInfo[];
}

export interface Overview {
  volumes: VolumeInfo[];
  total: number;
  online: number;
  offline: number;
  missing: number;
  /** Marked for deletion. */
  marked: number;
  dataDir: string;
}

/** Parallel arrays, one entry per photo, newest first. */
export interface PhotoList {
  ids: number[];
  /** Camera local time as seconds since 1970 read as UTC; 0 = unknown. */
  taken: number[];
  /** 2 = on a plugged-in disk, 1 = only on unplugged disks, 0 = missing. */
  state: number[];
  w: number[];
  h: number[];
  rev: number[];
  /** 0 = pending, 1 = ready, 2 = could not be read. */
  preview: number[];
  /** 1 = marked for deletion. */
  marked: number[];
  /** File extension, lower case ("cr3", "jpg"). */
  ext: string[];
  /** 1 = a RAW file. */
  raw: number[];
  /** On a person's page: 1 = they were found in it since you last looked at them. */
  fresh: number[];
}

export interface FileLoc {
  id: number;
  name: string;
  size: number;
  state: "online" | "offline" | "unreachable" | "missing";
  error: string | null;
  volumeId: number;
  volumeLabel: string;
  path: string;
  /** Its folder on the disk, as a folder scope takes it (with volumeId). */
  folder: string;
}

export interface PhotoDetail {
  id: number;
  kind: "raw" | "image";
  takenTs: number | null;
  takenFromCamera: boolean;
  make: string | null;
  model: string | null;
  /** Camera and lens as shown everywhere (their Gear names); null when the file doesn't say. */
  camera: string | null;
  lens: string | null;
  /** The lens as the file names it. */
  lensInFile: string | null;
  width: number | null;
  height: number | null;
  iso: number | null;
  fnumber: number | null;
  exposure: string | null;
  focal: number | null;
  previewState: number;
  previewRev: number;
  error: string | null;
  files: FileLoc[];
}

export interface Progress {
  rootId: number;
  rootName: string;
  phase: "listing" | "reading" | "done";
  found: number;
  toRead: number;
  read: number;
  current: string;
}

export interface Report {
  rootId: number;
  found: number;
  newFiles: number;
  changed: number;
  unchanged: number;
  restored: number;
  missing: number;
  photosAdded: number;
  /** Files whose photo was already in the catalog (moved or copied). */
  linked: number;
  /** More copies of a photo this scan read: the same file twice in the folder. */
  duplicates: number;
  /** Known photos read again (regenerated, edited, failed before). */
  reread: number;
  errors: number;
  /** Only the files that couldn't be read before, on a disk just plugged in. */
  retry: boolean;
  cancelled: boolean;
  seconds: number;
}

export interface Settings {
  previewSize: number;
  previewQuality: number;
  thumbHeight: number;
  workers: number;
  /** Ask GitHub at start and every few hours whether a newer release is out. */
  checkUpdates: boolean;
  /** Pages each catalog connection keeps, in MB; 0 = automatic. */
  catalogCacheMb: number;
  /** The catalog file mapped into memory, in MB; 0 = automatic (follows the catalog's size). */
  catalogMapMb: number;
}

export interface FolderCheck {
  /** Where the catalog would actually go. */
  target: string;
  hasCatalog: boolean;
  bytes: number;
  problem: string | null;
}

export interface MoveProgress {
  done: number;
  total: number;
  finished: boolean;
  error: string | null;
}

/** A folder moved by drag and drop. */
export interface MoveResult {
  volumeId: number;
  /** Its new path on its disk. */
  rel: string;
  /** Copied to another disk (true) or moved on the same disk. */
  copied: boolean;
  files: number;
  bytes: number;
  /** Originals that couldn't be removed after a checked copy. */
  leftBehind: string[];
}

/** Progress of a folder going to another disk. */
export interface FolderMoveEvent {
  name: string;
  progress: {
    phase: "copying" | "checking" | "removing";
    files: number;
    totalFiles: number;
    bytes: number;
    totalBytes: number;
  } | null;
  result: MoveResult | null;
  error: string | null;
  finished: boolean;
}

export const api = {
  overview: () => invoke<Overview>("get_overview"),
  listPhotos: (scope: Scope, sort: Sort, filters: Filters) => invoke<PhotoList>("list_photos", { scope, sort, filters }),
  facets: (scope: Scope, filters: Filters) => invoke<Facets>("facets", { scope, filters }),
  photo: (id: number) => invoke<PhotoDetail>("get_photo", { id }),
  browse: () => invoke<Browse>("browse"),
  stats: () => invoke<Stats>("catalog_stats"),
  regenerate: (rootId: number | null) => invoke<number>("regenerate", { rootId }),
  removeFolder: (rootId: number) => invoke<{ files: number; photos: number }>("remove_folder", { rootId }),
  addFolder: (path: string) => invoke<number>("add_folder", { path }),
  rescan: (rootId: number) => invoke<void>("rescan", { rootId }),
  rescanAll: () => invoke<number>("rescan_all"),
  cancelScan: () => invoke<void>("cancel_scan"),
  scanStatus: () => invoke<{ current: Progress | null; queued: number[] }>("scan_status"),
  revealFile: (fileId: number) => invoke<void>("reveal_file", { fileId }),
  openFile: (fileId: number) => invoke<void>("open_file", { fileId }),
  settings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  renameVolume: (volumeId: number, label: string) => invoke<void>("rename_volume", { volumeId, label }),
  /** Renames a folder on its disk and in the catalog; returns its new path on the disk. */
  renameFolder: (volumeId: number, relPath: string, name: string) =>
    invoke<string>("rename_folder", { volumeId, relPath, name }),
  /** Makes "New folder" inside a folder; returns its path on the disk. */
  newFolder: (volumeId: number, parentRel: string) => invoke<string>("new_folder", { volumeId, parentRel }),
  showFolder: (volumeId: number, relPath: string) => invoke<void>("show_folder", { volumeId, relPath }),
  /** Moves a folder into another one. Same disk: done, result back. Other disk: null, `folder-move` events follow. */
  moveFolder: (volumeId: number, relPath: string, toVolumeId: number, toParentRel: string) =>
    invoke<MoveResult | null>("move_folder", { volumeId, relPath, toVolumeId, toParentRel }),
  cancelFolderMove: () => invoke<void>("cancel_folder_move"),
  checkCatalogFolder: (path: string) => invoke<FolderCheck>("check_catalog_folder", { path }),
  /** Consistent copies of the databases in backup\ inside the catalog folder (for Kopia). */
  backup: () => invoke<{ folder: string; files: string[]; bytes: number; at: number; ms: number }>("backup_catalog"),
  lastBackup: () => invoke<number | null>("last_backup"),
  moveCatalog: (path: string) => invoke<void>("move_catalog", { path }),
  switchCatalog: (path: string) => invoke<void>("switch_catalog", { path }),
};

export interface PlanFile {
  path: string;
  size: number;
  /** "photo", "twin" (the RAW's camera JPEG) or "edits" (Argentum sidecar) */
  role: "photo" | "twin" | "edits";
  volumeId: number;
}

export interface PlanDisk {
  volumeId: number;
  label: string;
  mount: string;
  /** false: removable media, deleted for good. */
  recycle: boolean;
  photos: number;
  files: number;
  bytes: number;
}

export interface DeletePlan {
  marked: number;
  photos: number;
  /** Marked photos on unplugged disks or folders not found. */
  waiting: number;
  disks: PlanDisk[];
  files: PlanFile[];
  bytes: number;
  token: string;
}

export interface DeleteReport {
  deleted: number;
  bytes: number;
  recycled: number;
  permanent: number;
  photosRemoved: number;
  failed: string[];
}

/** Culling: marking is catalog-only; files go only after a confirmed plan. */
export const cull = {
  mark: (ids: number[], marked: boolean) => invoke<number>("mark_photos", { ids, marked }),
  unmarkAll: () => invoke<number>("unmark_all"),
  plan: () => invoke<DeletePlan>("delete_plan"),
  delete: (token: string, volumeIds: number[]) => invoke<DeleteReport>("delete_marked", { token, volumeIds }),
};

export const thumbUrl = (id: number, rev: number) => `http://shoebox.localhost/thumb/${id}?r=${rev}`;
export const previewUrl = (id: number, rev: number) => `http://shoebox.localhost/preview/${id}?r=${rev}`;
/** The photo read from its original (disk plugged in) at this long edge; 0 = full size. */
export const fullUrl = (id: number, long: number) => `http://shoebox.localhost/full/${id}?long=${long}`;

const nf = new Intl.NumberFormat();
export const fmtNum = (n: number) => nf.format(n);

const monthFmt = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric", timeZone: "UTC" });
const dayFmt = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  day: "numeric",
  month: "short",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  timeZone: "UTC",
});

/** Photo times are camera-local; formatting them as UTC shows them unshifted. */
export const fmtMonth = (ts: number) => (ts > 0 ? monthFmt.format(new Date(ts * 1000)) : "Unknown date");
const monthName = new Intl.DateTimeFormat(undefined, { month: "long", timeZone: "UTC" });
export const fmtMonthName = (m: number) => monthName.format(new Date(Date.UTC(2000, m - 1, 1)));
const dayFmtShort = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  day: "numeric",
  month: "short",
  year: "numeric",
  timeZone: "UTC",
});
/** "2026-09-26" -> "Sat, 26 Sep 2026", "2026-09" -> "September 2026", "2026" -> "2026" */
export function fmtDateKey(key: string) {
  if (key === "unknown") return "Unknown date";
  const [y, m, d] = key.split("-");
  if (d) return dayFmtShort.format(new Date(Date.UTC(+y, +m - 1, +d)));
  return m ? `${fmtMonthName(+m)} ${y}` : y;
}
const weekdayDay = new Intl.DateTimeFormat(undefined, { weekday: "short", day: "numeric", timeZone: "UTC" });
/** "2026-09-26" -> "Sat 26" */
export const fmtDayOfMonth = (key: string) => {
  const [y, m, d] = key.split("-");
  return weekdayDay.format(new Date(Date.UTC(+y, +m - 1, +d)));
};
export const fmtDay = (ts: number) => dayFmt.format(new Date(ts * 1000));

export function fmtBytes(n: number) {
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(2)} GB`;
}

export function fmtAgo(ts: number | null) {
  if (!ts) return "never scanned";
  const s = Date.now() / 1000 - ts;
  if (s < 90) return "scanned just now";
  if (s < 3600) return `scanned ${Math.round(s / 60)} min ago`;
  if (s < 86400 * 2) return `scanned ${Math.round(s / 3600)} h ago`;
  return `scanned ${Math.round(s / 86400)} days ago`;
}

export interface AiProgress {
  task: "search" | "faces";
  done: number;
  total: number;
  gpu: boolean;
  finished: boolean;
  error: string | null;
}

export const ai = {
  /** The first `clear` photos are clear matches, the rest possible ones. */
  search: (query: string, scope: Scope, filters: Filters) =>
    invoke<{ list: PhotoList; clear: number }>("search_photos", { query, scope, filters }),
  status: () => invoke<{ indexed: number; current: AiProgress | null }>("ai_status"),
};

/** The first `n` photos of a list. */
export function headOf(l: PhotoList, n: number): PhotoList {
  return {
    ids: l.ids.slice(0, n),
    taken: l.taken.slice(0, n),
    state: l.state.slice(0, n),
    w: l.w.slice(0, n),
    h: l.h.slice(0, n),
    rev: l.rev.slice(0, n),
    preview: l.preview.slice(0, n),
    marked: l.marked.slice(0, n),
    ext: l.ext.slice(0, n),
    raw: l.raw.slice(0, n),
    fresh: l.fresh.slice(0, n),
  };
}

export interface Person {
  id: number;
  name: string | null;
  faces: number;
  photos: number;
  /** Face id of the clearest face, for the person's picture. */
  cover: number;
  /** A stranger hidden from People. */
  ignored: boolean;
  /** None of their faces looked at yet: someone new. */
  isNew: boolean;
  /** Photos they were found in that you haven't looked at on their page yet. */
  newPhotos: number;
}

export interface PhotoFace {
  id: number;
  photoId: number;
  personId: number | null;
  name: string | null;
  /** 0..1 of the photo: x1, y1, x2, y2 */
  bbox: [number, number, number, number];
}

export const faceUrl = (faceId: number) => `http://shoebox.localhost/face/${faceId}`;

/** A name's words, lower case and without accents: "Mary-Ann" = "mary ann". */
const nameWords = (s: string) =>
  s
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean)
    .join(" ");

/** Two names that read the same ("jane  smith" and "Jane Smith"). Never a
 *  reason to join two people: only "Same person" does that. */
export const sameName = (a: string, b: string) => nameWords(a) === nameWords(b);

export interface Stray {
  faceId: number;
  photoId: number;
  /** Likeness to the person's usual face, 0..1. */
  similarity: number;
  /** Put in this person by hand. */
  byHand: boolean;
}

/** A face in someone else that may be this person ("Find more"). */
export interface Candidate {
  faceId: number;
  photoId: number;
  /** The person it's in now. */
  personId: number;
  /** Likeness to the person asked about, 0..1. */
  similarity: number;
  /** Likeness to the rest of the person it's in; null when it's their only face. */
  now: number | null;
  /** Put where it is by hand. */
  byHand: boolean;
}

/** One day's faces in "Find more", the most alike first. */
export interface CandidateDay {
  /** "2019-03-26"; null when the date is unknown. */
  day: string | null;
  /** Near the person's own photos: ticked to start with. */
  near: boolean;
  faces: Candidate[];
}

/** Where faces go: someone in People, someone new, or (not them, don't know
 *  who) whoever they resemble most. */
export type FaceTarget = { kind: "person"; id: number } | { kind: "new"; name: string | null } | { kind: "elsewhere" };

export interface Moved {
  /** Faces that changed person. */
  faces: number;
  /** Who they went to. */
  people: number[];
  /** Faces that made room (their new person was already in the photo): who each went to. */
  bumped: number[];
  /** Every face changed, as it was: `people.restore` puts them back. */
  undo: Placement[];
}

export interface Placement {
  faceId: number;
  personId: number | null;
  byHand: boolean;
  seen: boolean;
}

/** What "Same person" changed: `people.unmerge` puts it back (Undo). */
export interface Merged {
  into: number;
  /** Its name before (it takes the first name among them when it has none). */
  name: string | null;
  /** The people who joined, as they were, with their faces. */
  joined: { id: number; name: string | null; ignored: boolean; createdAt: number; faces: number[] }[];
}

/** A face in a People check finding. */
export interface CheckFace {
  faceId: number;
  photoId: number;
  /** Likeness to the rest of its person, 0..1. */
  similarity: number;
  /** Put in this person by hand. */
  byHand: boolean;
}

/** "Check" in People: likely mix-ups among named people. A face is in one finding at most. */
export interface PeopleCheck {
  /** Named people checked, and their faces. */
  people: number;
  faces: number;
  /** Someone twice in one photo: `elsewhere` are the faces to move out (all but the most alike). */
  twice: { personId: number; photoId: number; faces: CheckFace[]; elsewhere: number[] }[];
  /** A face unlike them that year (`own`), like them years apart (`like` in `likeYear`): a sibling, a child, a parent. */
  ageJumps: { personId: number; face: CheckFace; year: number; own: number; likeYear: number; like: number }[];
  /** A face much more like another named person; `near`: its photo is within a month of theirs (else unsure). */
  likeOthers: { personId: number; face: CheckFace; otherId: number; other: number; near: boolean }[];
  /** Two named people who may be one; "Same person" merges `otherId` into `personId`. Suggested only when `together` is 0. */
  samePeople: { personId: number; otherId: number; similarity: number; sameName: boolean; together: number }[];
}

export const people = {
  list: () => invoke<Person[]>("people"),
  /** People's search box: whoever's name matches, best first (any part of
   *  the name, case and accents ignored). */
  find: (query: string) => invoke<Person[]>("find_people", { query }),
  /** Their new photos have been seen (you left their page). */
  read: (ids: number[]) => invoke<number>("read_people", { ids }),
  /** "Mark all as read" in People. */
  readAll: () => invoke<number>("read_all_people"),
  rename: (personId: number, name: string) => invoke<void>("rename_person", { personId, name }),
  merge: (ids: number[], into: number) => invoke<Merged>("merge_people", { ids, into }),
  /** Undo of `merge`: the people are apart again. */
  unmerge: (merged: Merged) => invoke<number>("unmerge_people", { merged }),
  /** "Check": likely mix-ups among named people (nothing changes). */
  check: () => invoke<PeopleCheck>("check_people"),
  ignore: (ids: number[], ignored: boolean) => invoke<void>("ignore_people", { ids, ignored }),
  /** Re-sorts faces not named, ignored or placed by hand. [faces, new people] */
  regroup: () => invoke<[number, number]>("regroup_faces"),
  /** "Who is this?": the faces go there, placed by hand (never regrouped back). */
  reassign: (faceIds: number[], to: FaceTarget) => invoke<Moved>("reassign_faces", { faceIds, to }),
  /** Undo of `reassign`: the faces go back as they were. */
  restore: (placements: Placement[]) => invoke<number>("restore_faces", { placements }),
  /** Who these faces look like most, best first, to suggest who they are. */
  lookAlikes: (faceIds: number[], except: number[]) =>
    invoke<{ personId: number; similarity: number }[]>("look_alikes", { faceIds, except }),
  /** Faces in a person that don't look like them, worst first. */
  strays: (personId: number) => invoke<Stray[]>("person_strays", { personId }),
  /** "Find more": faces in other people that look more like this person than
   *  like the person they're in, by day, the days near their own photos first. */
  more: (personId: number) => invoke<CandidateDay[]>("more_faces", { personId }),
  /** Every face of a person, the least like them first. */
  faces: (personId: number) => invoke<Stray[]>("person_faces", { personId }),
  inPhoto: (photoId: number) => invoke<PhotoFace[]>("photo_faces", { photoId }),
  inPhotos: (photoIds: number[]) => invoke<PhotoFace[]>("faces_in_photos", { photoIds }),
};

/** One AI model in Settings > AI models. */
export interface ModelStatus {
  id: string;
  /** The feature it gives Shoebox: "Word search", "Faces"... */
  feature: string;
  name: string;
  about: string;
  publisher: string;
  licence: string;
  /** "partial": some files missing or wrong (a download fetches only those). */
  state: "ready" | "partial" | "missing";
  bytesHere: number;
  size: number;
  /** What a download fetches now. */
  downloadBytes: number;
  /** Not used by Shoebox yet: can be deleted, not downloaded. */
  later: boolean;
  /** The runtime Shoebox has loaded (can't be deleted until a restart). */
  inUse: boolean;
}

export interface ModelsOverview {
  dir: string;
  /** Word search and faces need one runtime. */
  runtimeReady: boolean;
  models: ModelStatus[];
}

/** A model being downloaded (`model-download` events). */
export interface ModelProgress {
  id: string;
  done: number;
  total: number;
  finished: boolean;
  error: string | null;
  /** Waiting their turn. */
  queued: string[];
}

/** A newer Shoebox: the latest published release on GitHub (a commit on a branch never counts). */
export interface UpdateStatus {
  current: string;
  /** None until a release is published. */
  latest: string | null;
  newer: boolean;
  url: string;
  published: string | null;
  /** When GitHub was last asked, unix seconds. */
  checked: number;
  /** Why the last check failed; what an earlier one found is kept. */
  error: string | null;
}

export const updatesApi = {
  /** What the last check found; null before the first one. */
  status: () => invoke<UpdateStatus | null>("update_status"),
  check: () => invoke<UpdateStatus>("check_updates"),
  /** Opens the release page in the browser. Nothing is downloaded by Shoebox. */
  openPage: () => invoke<void>("open_releases"),
};

/** AI models: downloaded from their publishers on request, checked by SHA-256. */
export const modelsApi = {
  list: () => invoke<ModelsOverview>("models_list"),
  download: (id: string) => invoke<void>("model_download", { id }),
  stop: (id: string) => invoke<void>("model_stop", { id }),
  progress: () => invoke<ModelProgress | null>("model_progress"),
  /** Returns the bytes freed. */
  delete: (id: string) => invoke<number>("model_delete", { id }),
  showFolder: () => invoke<void>("show_models_folder"),
};
