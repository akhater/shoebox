import { invoke } from "@tauri-apps/api/core";
import { pictureBase } from "./paths";

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
  | { kind: "marked" }
  /** Photos the upright model finds sideways or upside down, waiting for you. */
  | { kind: "sideways" }
  /** The private photos of the space open now (none while locked). */
  | { kind: "private" }
  /** The frames of a focus stack, by its first frame's id. */
  | { kind: "stack"; id: number };

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
  /** "private" and/or "notPrivate": only while private photos are open. */
  private: ("private" | "notPrivate")[];
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
  private: [],
};
export const filtersOn = (f: Filters) =>
  f.dates.length +
    f.cameras.length +
    f.lenses.length +
    f.types.length +
    f.status.length +
    f.people.length +
    f.tags.length +
    f.private.length >
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
  /** "private" and "notPrivate" with counts, while private photos are open (empty otherwise). */
  private: FacetValue[];
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
  /** Not here (disk unplugged, or folder not found on it), but found at the same path on a disk plugged in now. */
  elsewhere: Repoint | null;
}

/** What pointing a source at a folder found there: a sample of its photos looked for. */
export interface Repoint {
  rootId: number;
  /** The folder, as the system writes it. */
  path: string;
  /** The disk holding it, by its name in Shoebox. */
  disk: string;
  /** Photos of the source looked for there. */
  checked: number;
  /** Of those, the ones there, unchanged. */
  found: number;
  /** Why the source can't be pointed there; null when it can. */
  problem: string | null;
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
  /** Suggested for turning by the upright model, waiting for you. */
  sideways: number;
  /** Private photos of the space open now (0 while locked). */
  private: number;
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
  /** What Cull found wrong with each photo: bits of PROBLEMS (0 = nothing, or not measured yet). */
  flags: number[];
  /** Quarter turns clockwise the upright model suggests (0 = none). */
  sideways: number[];
  /** 1 = a private photo (listed only while its space is open). */
  private: number[];
  /** The focus stack each photo is a frame of (its first frame's id), 0 for none. */
  stack?: number[];
  /** How many frames each tile stands for: a focus stack is one tile in most views (1 for a
   *  photo on its own). */
  frames?: number[];
  /** The frames of each tile that stands for several, by the id it is shown as (that one first). */
  members?: Record<number, number[]>;
}

/** These photos of a list, each tile with all the frames it stands for: what an action on
 *  selected tiles applies to. */
export const withFrames = (l: PhotoList, ids: number[]) => ids.flatMap((id) => l.members?.[id] ?? [id]);

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
  /** The date its file gives, when you corrected `takenTs` ("Fix dates"); null otherwise. */
  takenOriginal: number | null;
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
  /** Quarter turns clockwise you turned it, on top of what its file says. */
  turn: number;
  /** Quarter turns clockwise the upright model suggests (0 = none). */
  sideways: number;
  /** Where the camera focused, as the photo is shown; null when its file doesn't say. */
  focus: Focus | null;
  /** What Cull found wrong with it, once measured (empty: nothing, or not measured yet). */
  problems: Found[];
  /** A private photo (shown only while its space is open). */
  private: boolean;
  files: FileLoc[];
  /** Its place in a focus stack, when it is a frame of one. */
  stack: { stack: number; frame: number; frames: number } | null;
}

/** What an autofocus point was in a photo: where focus was found, chosen
 *  (focus not confirmed there), the zone the camera was given, or one of its other points. */
export type FocusState = "inFocus" | "chosen" | "area" | "other";

export interface FocusPoint {
  /** Top-left corner and size, fractions (0..1) of the picture as shown. */
  x: number;
  y: number;
  w: number;
  h: number;
  state: FocusState;
}

export interface Focus {
  /** How the camera was set to focus: "Single point", "Zone", "Whole area"... */
  mode: string | null;
  points: FocusPoint[];
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
  /** Photos and folders left out because their names can't be kept in the catalog on this system. */
  skipped: number;
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
  /** Private photos lock again after this many minutes without using Shoebox. */
  privateLockMinutes: number;
  /** Warn each time a password opens private photos. Off only for a password holding photos:
   *  one opening an empty space (new, or mistyped) is always warned about. */
  privateWarning: boolean;
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
  /** Which of these photos are missing everywhere, and which of those are in the bin. */
  missingPhotos: (ids: number[]) => invoke<MissingReport>("missing_photos", { ids }),
  /** Takes the missing photos among these out of the catalog. No file is touched. */
  forgetMissing: (ids: number[]) => invoke<{ photos: number; files: number }>("forget_missing", { ids }),
  addFolder: (path: string) => invoke<number>("add_folder", { path }),
  /** What pointing a source at a folder would find there. Reads only. */
  checkRepoint: (rootId: number, path: string) => invoke<Repoint>("check_repoint", { rootId, path }),
  /** Points a source at the folder where its photos are now (catalog only), then scans it there. */
  repointFolder: (rootId: number, path: string) => invoke<Repoint>("repoint_folder", { rootId, path }),
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
  /** Marked photos already gone from every disk: the delete only takes them out of the catalog. */
  gone: number[];
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
  /** Of those, marked photos that were already gone from every disk. */
  forgotten: number;
  failed: string[];
}

/** Photos with no copy left on any disk, and which of them are in the system's bin. */
export interface MissingReport {
  /** False when the bin couldn't be read: nothing is known about it. */
  binRead: boolean;
  photos: MissingPhoto[];
}

export interface MissingPhoto {
  id: number;
  /** Where its files were. */
  paths: string[];
  /** Every one of its files is in the bin: deleted, not moved. */
  inBin: boolean;
  /** When the last of them went to the bin (seconds since 1970; 0 = unknown). */
  deletedAt: number;
}

/** What this system calls its bin. */
export const BIN = /Windows/i.test(navigator.userAgent) ? "Recycle Bin" : "Trash";

const deletedFmt = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
/** When a file went to the bin (seconds since 1970), in this computer's time. */
export const fmtDeleted = (ts: number) => deletedFmt.format(new Date(ts * 1000));

/** What Cull can find wrong with a photo; `bit` is its bit in PhotoList.flags. */
export type Problem = "empty" | "dark" | "blurry" | "soft" | "faceSoft" | "blown" | "bright" | "focusMissed";
export const PROBLEMS: { problem: Problem; bit: number; label: string }[] = [
  { problem: "empty", bit: 1, label: "Nothing in it" },
  { problem: "dark", bit: 2, label: "Very dark" },
  { problem: "blurry", bit: 4, label: "Blurry" },
  { problem: "soft", bit: 8, label: "Soft at 100%" },
  { problem: "faceSoft", bit: 16, label: "Face out of focus" },
  { problem: "blown", bit: 32, label: "Highlights blown" },
  { problem: "bright", bit: 64, label: "Very bright" },
  { problem: "focusMissed", bit: 128, label: "Focus missed" },
];
export const problemsOf = (flags: number) => PROBLEMS.filter((p) => flags & p.bit);
/** The problems about sharpness: what the camera's focus confirmation can't tell. */
export const BLUR_PROBLEMS: Problem[] = ["blurry", "soft", "faceSoft", "focusMissed"];

/** A problem Cull found, in its words ("Blurry everywhere: 1.6 s at 37 mm is slow..."). */
export interface Found {
  problem: Problem;
  text: string;
}

/** A frame of a shoot in Cull, with what Cull suggests for it and why. */
export interface CullFrame {
  id: number;
  name: string | null;
  taken: number;
  /** 2 = on a plugged-in disk, 1 = unplugged, 0 = missing. */
  state: number;
  w: number;
  h: number;
  rev: number;
  /** Marked for deletion. */
  marked: boolean;
  suggest: "keep" | "reject" | "none";
  /** Why, the first one the main reason. */
  reasons: string[];
  problems: Problem[];
  /** Edge blur compared between frames, thousandths of the long edge (lower is sharper):
   *  where the stack's `compared` says. */
  blur: number | null;
  /** Measured from the original, not only the preview. */
  closer: boolean;
  /** How alike CLIP finds it and the frame before (0..1). */
  alike: number | null;
  /** How alike its layout and the frame before's are (-1..1). */
  framing: number | null;
  /** Where to look at 100% first (0..1 of the picture): where the camera focused, when its
   *  file says, else the middle of its largest face. */
  focus: [number, number] | null;
  /** The points the camera focused on (those in focus, else those chosen), as the photo is
   *  shown: `focus` is on one of them. Empty when its file doesn't say. */
  focusPoints: FocusPoint[];
}
/** Frames of the same shot; `keeper` = the one Cull would keep. */
export interface CullStack {
  frames: CullFrame[];
  keeper: number;
  /** Where the frames' sharpness was compared: around the eyes of the largest face (every
   *  frame has one), where the camera focused (every file says, and the point is sharp in one
   *  frame at least), else the sharpest part. */
  compared: "face" | "focusPoint" | "sharpest";
  /** Compared on what the originals showed: every frame was looked at closer. */
  closer: boolean;
  /** "shot": frames of the same shot (a burst), the best one kept. "focus": the frames of a
   *  focus bracket, to be merged into one picture: every one kept. */
  kind: "shot" | "focus";
}
export interface Shoot {
  photos: number;
  measured: number;
  closer: number;
  described: number;
  /** Photos whose original is reachable now and not looked at closer yet. */
  canLookCloser: number[];
  /** Originals a closer look couldn't read, and why. */
  unreadable: { id: number; name: string | null; reason: string }[];
  stacks: CullStack[];
}
export interface CloserProgress {
  done: number;
  total: number;
  finished: boolean;
}

/** Culling: marking is catalog-only; files go only after a confirmed plan. */
export const cull = {
  mark: (ids: number[], marked: boolean) => invoke<number>("mark_photos", { ids, marked }),
  unmarkAll: () => invoke<number>("unmark_all"),
  plan: () => invoke<DeletePlan>("delete_plan"),
  delete: (token: string, volumeIds: number[]) => invoke<DeleteReport>("delete_marked", { token, volumeIds }),
  /** A view as a shoot: bursts in stacks, problems, suggestions. Its photos not measured yet are
   *  measured in the background ("cull-changed" says when to ask again). */
  shoot: (scope: Scope, filters: Filters) => invoke<Shoot>("cull_shoot", { scope, filters }),
  /** What Cull finds wrong with these photos (bits of PROBLEMS), those not measured yet measured first. */
  flags: (ids: number[]) => invoke<number[]>("cull_flags", { ids }),
  /** Measure these photos again from their originals (those plugged in), in the background. */
  lookCloser: (ids: number[]) => invoke<void>("cull_look_closer", { ids }),
  stop: () => invoke<void>("cull_stop"),
  progress: () => invoke<CloserProgress | null>("cull_progress"),
};

/** Where Shoebox's pictures are asked at (see `pictureBase`). */
const BASE = pictureBase(navigator.userAgent);

export const thumbUrl = (id: number, rev: number) => `${BASE}/thumb/${id}?r=${rev}`;
export const previewUrl = (id: number, rev: number) => `${BASE}/preview/${id}?r=${rev}`;
/** The photo read from its original (disk plugged in) at this long edge; 0 = full size. `rev`
 *  is its preview's: a turned photo is asked for again, turned. */
export const fullUrl = (id: number, long: number, rev: number) => `${BASE}/full/${id}?long=${long}&r=${rev}`;

export interface PrivateStatus {
  /** A private space is open (its photos shown). Nothing is said about any other. */
  unlocked: boolean;
  /** Photos in the space open now: 0 for a new or mistyped password (and while locked). */
  photos: number;
  /** The name of the space open now, if it has one (shown next to Private). */
  name: string | null;
}

/** Private photos: every password is its own space, like a wallet's passphrase - never
 *  refused, a new one opens an empty space, none shows another's. Hidden from every view,
 *  count, search and agent until their password opens them; Shoebox's copies of them are
 *  kept sealed. Photo files are never touched. */
export const privateApi = {
  status: () => invoke<PrivateStatus>("private_status"),
  /** Opens the space of a password (in this window only, until locked). */
  unlock: (password: string) => invoke<PrivateStatus>("private_unlock", { password }),
  lock: () => invoke<PrivateStatus>("private_lock"),
  /** Names the space open now ("" = no name); kept with its photos, sealed. */
  rename: (name: string) => invoke<PrivateStatus>("private_rename", { name }),
  /** Into the space open now; its first photos need its password again (`confirm`). */
  hide: (ids: number[], confirm?: string) => invoke<number>("private_hide", { ids, confirm: confirm ?? null }),
  /** Every photo of a view (a folder). */
  hideIn: (scope: Scope, confirm?: string) => invoke<number>("private_hide_in", { scope, confirm: confirm ?? null }),
  show: (ids: number[]) => invoke<number>("private_show", { ids }),
};
/** Opens or locks private photos. */
export const PRIVATE_SHORTCUT = "Ctrl+Shift+L";
/** A password shorter than this is refused (as the backend does). */
export const MIN_PASSWORD = 6;
/** A private space's name has at most this many characters (as the backend checks). */
export const PRIVATE_NAME_CHARS = 40;
/** "Private", or "Private · <its name>" for a named space. */
export const privateTitle = (name: string | null | undefined) => (name ? `Private · ${name}` : "Private");

/** Photos turned. Only the catalog and Shoebox's copies change, never a file. */
export interface TurnReport {
  turned: number;
  /** Their faces, turned with them (names kept). */
  faces: number;
  /** Photos whose preview couldn't be turned: made again, turned, from the original when its disk is plugged in. */
  toReadAgain: number[];
  /** Each photo turned, as it stands now. */
  photos: Turned[];
}

/** A photo just turned: `by` quarter turns clockwise this time (Undo turns it back by as many). */
export interface Turned {
  id: number;
  by: number;
  /** How it is turned now, on top of what its file says. */
  turn: number;
  rev: number;
  w: number;
  h: number;
}

/** Turning photos (catalog only) and the upright model's suggestions. */
export const turnApi = {
  /** Turns photos clockwise by quarter turns (-1: counter-clockwise, 2: upside down). */
  turn: (ids: number[], by: number) => invoke<TurnReport>("turn_photos", { ids, by }),
  /** Back to how their files show them. */
  back: (ids: number[]) => invoke<TurnReport>("turn_back", { ids }),
  /** Turns them the way the upright model suggests. */
  accept: (ids: number[]) => invoke<TurnReport>("sideways_accept", { ids }),
  /** Fine as they are: never suggested again. */
  leave: (ids: number[]) => invoke<number>("sideways_leave", { ids }),
};

/** What the date check found: folders named with a date whose photos from one camera are
 *  days away from it and from the other photos there (a camera clock set wrong). */
export interface DateCheck {
  /** Photos further than this many days from their folder's dates are listed. */
  days: number;
  /** Folders looked at (named with a date, or in one) and their photos with a date. */
  folders: number;
  photos: number;
  /** Photos of the whole catalog whose date was corrected. */
  corrected: number;
  /** Findings left as is (listed only when all were asked for, `left` set). */
  left: number;
  findings: DateFinding[];
}

/** Photos from one camera in one folder, days away from the folder's dates. */
export interface DateFinding {
  folderId: number;
  volumeId: number;
  relPath: string;
  /** The folder whose name gives the date: the same one, or a folder it is in. */
  dated: string;
  /** Its dates, "2006-11-04" (the same day twice unless a range). */
  from: string;
  to: string;
  camera: string | null;
  serial: string | null;
  /** No date in the files: theirs is the time the file was written. */
  fileDate: boolean;
  /** The camera, as "Leave as is" keeps it. */
  key: string;
  /** Oldest first. */
  photos: number[];
  first: number;
  last: number;
  /** Days from the folder's dates (negative: before them). */
  daysOff: number;
  /** Seconds that move the first photo to the folder's day, its time of day kept. */
  shift: number;
  /** The other photos there that agree with the folder's dates, and their first and last day. */
  others: number;
  othersFrom: string | null;
  othersTo: string | null;
  /** Photos among them whose date was corrected already. */
  corrected: number;
  left: boolean;
}

/** A finding left as is (a finding itself will do). */
export interface LeftDates {
  folderId: number;
  key: string;
  first: number;
}

/** How to move some photos' dates. */
export type DateChange =
  /** Later by this many seconds (earlier when negative). */
  | { kind: "by"; seconds: number }
  /** The first of them to this day ("2006-11-04"), its time of day kept; the others by as much. */
  | { kind: "to"; date: string }
  /** Back to the dates their files give. */
  | { kind: "back" };

/** What a date change did: `undo` puts exactly that back. */
export interface Shifted {
  photos: { id: number; by: number; from: number; to: number }[];
  /** Left as they were: no date, already there, or it would fall before 1970. */
  unchanged: number;
}

/** Some photos' dates, as "Fix dates" shows them first. */
export interface PhotoDates {
  photos: number;
  dated: number;
  first: number | null;
  last: number | null;
  corrected: number;
  /** The folder whose name dates most of them, and its first day. */
  folder: string | null;
  folderDate: string | null;
}

/** Dates corrected in the catalog only (a camera clock set wrong): never written into a photo. */
export const datesApi = {
  /** In a folder and those in it (a source: its folder; a disk: relPath ""), or the whole catalog. */
  check: (where: { volumeId: number; relPath: string } | null, all = true) =>
    invoke<DateCheck>("check_dates", { volumeId: where?.volumeId ?? null, relPath: where?.relPath ?? null, all }),
  leave: (findings: LeftDates[], left: boolean) => invoke<number>("leave_dates", { findings, left }),
  of: (ids: number[]) => invoke<PhotoDates>("dates_of", { ids }),
  change: (ids: number[], how: DateChange) => invoke<Shifted>("change_dates", { ids, how }),
  undo: (done: Shifted) => invoke<Shifted>("undo_dates", { done }),
};

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
  task: "upright" | "search" | "faces";
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
    flags: l.flags.slice(0, n),
    sideways: l.sideways.slice(0, n),
    private: l.private.slice(0, n),
    stack: l.stack?.slice(0, n),
    frames: l.frames?.slice(0, n),
    members: l.members,
  };
}

/** The list without its private photos: what the grid keeps the moment they are locked,
 *  before the next answer comes. */
export function withoutPrivate(l: PhotoList): PhotoList {
  if (!l.private?.includes(1)) return l;
  const keep = l.private.map((p) => p !== 1);
  return Object.fromEntries(
    Object.entries(l).map(([k, a]) => [k, Array.isArray(a) ? a.filter((_, i) => keep[i]) : a]),
  ) as unknown as PhotoList;
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

export const faceUrl = (faceId: number) => `${BASE}/face/${faceId}`;

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

/** "Check" in People: likely mix-ups among named people. A face is in one finding at most.
 *  Each finding's `left`: the user left it as is (listed only when all are asked for). */
export interface PeopleCheck {
  /** Named people checked, and their faces. */
  people: number;
  faces: number;
  /** Findings left as is: left out, or (all asked for) listed with `left` set. */
  left: number;
  /** Someone twice in one photo: `elsewhere` are the faces to move out (all but the most alike). */
  twice: { personId: number; photoId: number; faces: CheckFace[]; elsewhere: number[]; left: boolean }[];
  /** A face unlike them that year (`own`), like them years apart (`like` in `likeYear`): a sibling, a child, a parent. */
  ageJumps: { personId: number; face: CheckFace; year: number; own: number; likeYear: number; like: number; left: boolean }[];
  /** A face much more like another named person; `near`: its photo is within a month of theirs (else unsure). */
  likeOthers: { personId: number; face: CheckFace; otherId: number; other: number; near: boolean; left: boolean }[];
  /** Two named people who may be one; "Same person" merges `otherId` into `personId`. Suggested only when `together` is 0. */
  samePeople: { personId: number; otherId: number; similarity: number; sameName: boolean; together: number; left: boolean }[];
}

/** A check finding the user looked at and kept ("Leave as is"), as it was seen:
 *  the check leaves it out until what it is about changes. */
export type LeftAsIs =
  | { kind: "twice"; personId: number; photoId: number; faces: number[] }
  | { kind: "ageJump"; personId: number; faceId: number }
  | { kind: "likeOther"; personId: number; faceId: number; otherId: number }
  | { kind: "samePerson"; personId: number; otherId: number };

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
  /** "Check": likely mix-ups among named people (nothing changes); `all`: those left as is too. */
  check: (all = false) => invoke<PeopleCheck>("check_people", { all }),
  /** "Leave as is" on check findings (`left` false: back in the check). How many changed. */
  leave: (findings: LeftAsIs[], left: boolean) => invoke<number>("leave_as_is", { findings, left }),
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
