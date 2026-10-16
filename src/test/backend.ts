import { mockIPC } from "@tauri-apps/api/mocks";
import type { Facets, Merged, Overview, PeopleCheck, Person, PhotoDetail, PhotoList, Placement, Scope, Sort } from "../api";

/** A photo in the fake catalog. */
interface Photo {
  id: number;
  /** The source it is in. */
  root: number;
  taken: number;
}

/** The sources: one disk, three folders of four photos each. */
export const ROOTS = [
  { id: 10, name: "Slow" },
  { id: 20, name: "Fast" },
  { id: 30, name: "Trip" },
];
/** Photo ids of a source, in the order they were taken. */
export const photosOf = (root: number) => [1, 2, 3, 4].map((n) => root * 10 + n);
export const ALL = ROOTS.flatMap((r) => photosOf(r.id));

/** What a word search finds: the first `clear` are clear matches. */
export const SEARCH = { ids: [301, 201, 101, 302, 202], clear: 2 };

const JAN_2026 = 1_767_225_600;

/** One call the window made. */
export interface Call {
  cmd: string;
  args: Record<string, unknown>;
}

/**
 * Shoebox's backend, faked: a small catalog in memory, every answer after a
 * delay (10 ms unless a test makes a view slower), and a log of every call.
 */
export function fakeBackend() {
  const photos: Photo[] = ROOTS.flatMap((r) => photosOf(r.id).map((id) => ({ id, root: r.id, taken: JAN_2026 + id * 3600 })));
  const marked = new Set<number>();
  let people: Person[] = [
    { id: 7, name: "Ana", faces: 3, photos: 3, cover: 1, ignored: false, isNew: false, newPhotos: 0 },
    { id: 8, name: null, faces: 2, photos: 2, cover: 2, ignored: false, isNew: false, newPhotos: 0 },
  ];
  const inPerson: Record<number, number[]> = { 7: [101, 201, 301], 8: [102, 202] };
  /** What "Check" in People finds (a test sets it). */
  let check: PeopleCheck = { people: 0, faces: 0, twice: [], ageJumps: [], likeOthers: [], samePeople: [] };
  /** People merged into someone, as they were, for Undo. */
  const unmerged = new Map<number, Person>();
  const calls: Call[] = [];
  /** Extra time for some answers: by command, and by view (`viewName`) for the commands named. */
  const cmdDelays = new Map<string, number>();
  const viewDelays = new Map<string, { ms: number; cmds: string[] }>();
  const extra = (cmd: string, scope: Scope | undefined) => {
    const v = scope && viewDelays.get(viewName(scope));
    return (cmdDelays.get(cmd) ?? 0) + (v && v.cmds.includes(cmd) ? v.ms : 0);
  };

  const overview = (): Overview => ({
    volumes: [
      {
        id: 1,
        label: "Disk",
        online: true,
        mount: "D:\\",
        lastMount: "D:\\",
        photos: photos.length,
        roots: ROOTS.map((r) => ({
          id: r.id,
          found: true,
          name: r.name,
          path: `D:\\${r.name}`,
          relPath: r.name,
          lastScan: null,
          photos: 4,
        })),
      },
    ],
    total: photos.length,
    online: photos.length,
    offline: 0,
    missing: 0,
    marked: marked.size,
    dataDir: "data",
  });

  const inScope = (s: Scope): Photo[] => {
    switch (s.kind) {
      case "all":
        return photos;
      case "root":
        return photos.filter((p) => p.root === s.id);
      case "folder":
        return photos.filter((p) => ROOTS.find((r) => r.id === p.root)?.name === s.relPath);
      case "person":
        return photos.filter((p) => inPerson[s.id]?.includes(p.id));
      case "marked":
        return photos.filter((p) => marked.has(p.id));
      default:
        return [];
    }
  };

  const listOf = (ps: Photo[]): PhotoList => ({
    ids: ps.map((p) => p.id),
    taken: ps.map((p) => p.taken),
    state: ps.map(() => 2),
    w: ps.map(() => 1500),
    h: ps.map(() => 1000),
    rev: ps.map(() => 1),
    preview: ps.map(() => 1),
    marked: ps.map((p) => +marked.has(p.id)),
    ext: ps.map(() => "jpg"),
    raw: ps.map(() => 0),
    fresh: ps.map(() => 0),
  });

  const sorted = (ps: Photo[], sort: Sort) =>
    sort === "newest" ? [...ps].sort((a, b) => b.taken - a.taken) : [...ps].sort((a, b) => a.taken - b.taken);

  const facets = (): Facets => ({ dates: [], cameras: [], lenses: [], types: [], status: [], people: [], tags: [] });

  const detail = (id: number): PhotoDetail => {
    const root = ROOTS.find((r) => r.id === photos.find((p) => p.id === id)?.root)!;
    return {
      id,
      kind: "image",
      takenTs: JAN_2026 + id * 3600,
      takenFromCamera: true,
      make: null,
      model: null,
      camera: null,
      lens: null,
      lensInFile: null,
      width: 1500,
      height: 1000,
      iso: null,
      fnumber: null,
      exposure: null,
      focal: null,
      previewState: 1,
      previewRev: 1,
      error: null,
      files: [
        {
          id: id * 10,
          name: `IMG_${id}.jpg`,
          size: 2_000_000,
          state: "online",
          error: null,
          volumeId: 1,
          volumeLabel: "Disk",
          path: `D:\\${root.name}\\IMG_${id}.jpg`,
          folder: root.name,
        },
      ],
    };
  };

  const answer = (cmd: string, a: Record<string, unknown>): unknown => {
    switch (cmd) {
      case "get_overview":
        return overview();
      case "browse":
        return { years: [], unknownDate: 0, gear: { cameras: [], lenses: [] }, folders: [] };
      case "people":
        return people;
      case "find_people":
        return people.filter((p) => p.name?.toLowerCase().includes(String(a.query).toLowerCase()));
      case "list_photos":
        return listOf(sorted(inScope(a.scope as Scope), a.sort as Sort));
      case "search_photos": {
        const here = new Set(inScope(a.scope as Scope).map((p) => p.id));
        const found = SEARCH.ids.filter((id) => here.has(id)).map((id) => photos.find((p) => p.id === id)!);
        return { list: listOf(found), clear: SEARCH.clear };
      }
      case "facets":
        return facets();
      case "get_photo":
        return detail(a.id as number);
      case "photo_faces":
      case "tags_on":
      case "tags_all":
        return [];
      case "mark_photos":
        for (const id of a.ids as number[]) (a.marked ? marked.add(id) : marked.delete(id));
        return (a.ids as number[]).length;
      case "read_people":
        return 0;
      case "check_people":
        return check;
      case "reassign_faces": {
        // Every face was Ana's (7) before, seen, Shoebox's guess.
        const ids = a.faceIds as number[];
        const undo: Placement[] = ids.map((faceId) => ({ faceId, personId: 7, byHand: false, seen: true }));
        return { faces: ids.length, people: [], bumped: [], undo };
      }
      case "restore_faces":
        return (a.placements as Placement[]).length;
      case "merge_people": {
        const into = a.into as number;
        const gone = people.filter((p) => p.id !== into && (a.ids as number[]).includes(p.id));
        people = people.filter((p) => !gone.includes(p));
        for (const p of gone) unmerged.set(p.id, p);
        const joined = gone.map((p) => ({ id: p.id, name: p.name, ignored: p.ignored, createdAt: 0, faces: [p.cover] }));
        return { into, name: people.find((p) => p.id === into)?.name ?? null, joined } satisfies Merged;
      }
      case "unmerge_people": {
        const m = a.merged as Merged;
        people = [...people, ...m.joined.map((j) => unmerged.get(j.id)!)];
        return m.joined.reduce((n, j) => n + j.faces.length, 0);
      }
      case "scan_status":
        return { current: null, queued: [] };
      case "update_status":
        return null;
      case "plugin:app|version":
        return "0.0.0";
      default:
        throw new Error(`the fake backend has no "${cmd}"`);
    }
  };

  mockIPC(
    (cmd, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      calls.push({ cmd, args });
      const ms = 10 + extra(cmd, args.scope as Scope | undefined);
      return new Promise((resolve, reject) =>
        setTimeout(() => {
          try {
            resolve(answer(cmd, args));
          } catch (e) {
            reject(e);
          }
        }, ms),
      );
    },
    { shouldMockEvents: true },
  );

  return {
    calls,
    /** The commands called since `from` (an index into `calls`), in order. */
    cmdsSince: (from: number) => calls.slice(from).map((c) => c.cmd),
    /** Makes answers about this view (its photos and counts, unless `cmds` says) take `ms` longer. */
    slow: (scope: Scope, ms: number, cmds = ["list_photos", "search_photos", "facets"]) =>
      viewDelays.set(viewName(scope), { ms, cmds }),
    /** Makes every answer to `cmd` take `ms` longer. */
    delay: (cmd: string, ms: number) => cmdDelays.set(cmd, ms),
    marked,
    /** Face finding: someone new, or more photos of someone. */
    addPerson: (p: Person, photoIds: number[]) => {
      people = [...people, p];
      inPerson[p.id] = photoIds;
    },
    /** What the next "Check" in People finds. */
    peopleCheck: (c: PeopleCheck) => {
      check = c;
    },
    addPhotosOf: (personId: number, photoIds: number[]) => {
      inPerson[personId] = [...inPerson[personId], ...photoIds];
      people = people.map((p) => (p.id === personId ? { ...p, photos: inPerson[personId].length } : p));
    },
  };
}

export type FakeBackend = ReturnType<typeof fakeBackend>;

/** Names a view for `slow`. */
export const viewName = (s: Scope) => (s.kind === "root" || s.kind === "person" ? `${s.kind}:${s.id}` : s.kind);
