import { mockIPC } from "@tauri-apps/api/mocks";
import type {
  CandidateDay,
  CullFrame,
  CullStack,
  FaceTarget,
  Facets,
  LeftAsIs,
  Merged,
  Overview,
  PeopleCheck,
  Person,
  PhotoDetail,
  PhotoList,
  Placement,
  Problem,
  Scope,
  Shoot,
  Sort,
} from "../api";

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
  /** Photos missing everywhere (a test sets them), and those of them in the bin. */
  const missing = new Set<number>();
  const inBin = new Set<number>();
  /** Originals Cull couldn't read for a closer look (a test sets them). */
  let unreadable: Shoot["unreadable"] = [];
  let people: Person[] = [
    { id: 7, name: "Ana", faces: 3, photos: 3, cover: 1, ignored: false, isNew: false, newPhotos: 0 },
    { id: 8, name: null, faces: 2, photos: 2, cover: 2, ignored: false, isNew: false, newPhotos: 0 },
  ];
  const inPerson: Record<number, number[]> = { 7: [101, 201, 301], 8: [102, 202] };
  /** What "Check" in People finds (a test sets it). */
  let check: PeopleCheck = { people: 0, faces: 0, left: 0, twice: [], ageJumps: [], likeOthers: [], samePeople: [] };
  /** "Leave as is" (or back) on the check's findings: they keep it for the next check. */
  const leaveAsIs = (findings: LeftAsIs[], left: boolean) => {
    const about = (l: LeftAsIs) =>
      l.kind === "twice"
        ? `twice:${l.photoId}:${l.personId}`
        : l.kind === "samePerson"
          ? `same:${Math.min(l.personId, l.otherId)}:${Math.max(l.personId, l.otherId)}`
          : `${l.kind}:${l.faceId}`;
    const these = new Set(findings.map(about));
    const flag = <T extends { left: boolean }>(items: T[], as: (t: T) => LeftAsIs) =>
      items.map((t) => (these.has(about(as(t))) ? { ...t, left } : t));
    check = {
      ...check,
      twice: flag(check.twice, (t) => ({ kind: "twice", personId: t.personId, photoId: t.photoId, faces: [] })),
      ageJumps: flag(check.ageJumps, (a) => ({ kind: "ageJump", personId: a.personId, faceId: a.face.faceId })),
      likeOthers: flag(check.likeOthers, (l) => ({ kind: "likeOther", personId: l.personId, faceId: l.face.faceId, otherId: l.otherId })),
      samePeople: flag(check.samePeople, (p) => ({ kind: "samePerson", personId: p.personId, otherId: p.otherId })),
    };
    return findings.length;
  };
  /** The check as Shoebox answers it: those left as is counted, and listed only when `all`. */
  const checkAnswer = (all: boolean): PeopleCheck => {
    const keep = <T extends { left: boolean }>(items: T[]) => items.filter((t) => all || !t.left);
    const lists = [check.twice, check.ageJumps, check.likeOthers, check.samePeople];
    const left = lists.reduce((n, l) => n + l.filter((t) => t.left).length, 0);
    return {
      ...check,
      left,
      twice: keep(check.twice),
      ageJumps: keep(check.ageJumps),
      likeOthers: keep(check.likeOthers),
      samePeople: keep(check.samePeople),
    };
  };
  /** People merged into someone, as they were, for Undo. */
  const unmerged = new Map<number, { person: Person; photos: number[] }>();
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
    missing: photos.filter((p) => missing.has(p.id)).length,
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
      case "missing":
        return photos.filter((p) => missing.has(p.id));
      default:
        return [];
    }
  };

  const listOf = (ps: Photo[]): PhotoList => ({
    ids: ps.map((p) => p.id),
    taken: ps.map((p) => p.taken),
    state: ps.map((p) => (missing.has(p.id) ? 0 : 2)),
    w: ps.map(() => 1500),
    h: ps.map(() => 1000),
    rev: ps.map(() => 1),
    preview: ps.map(() => 1),
    marked: ps.map((p) => +marked.has(p.id)),
    ext: ps.map(() => "jpg"),
    raw: ps.map(() => 0),
    fresh: ps.map(() => 0),
    // The fourth photo of each source is blurry.
    flags: ps.map((p) => (p.id % 10 === 4 ? 4 : 0)),
  });

  /**
   * Cull's shoot: in each source, its first two photos are a burst (the
   * second is sharper and kept), the third is fine, the fourth blurry.
   */
  const shootOf = (ps: Photo[]): Shoot => {
    const frame = (p: Photo, suggest: CullFrame["suggest"], reasons: string[], problems: Problem[] = []): CullFrame => ({
      id: p.id,
      name: `IMG_${p.id}.jpg`,
      taken: p.taken,
      state: 2,
      w: 150,
      h: 100,
      rev: 1,
      marked: marked.has(p.id),
      suggest,
      reasons,
      problems,
      blur: 0.3,
      closer: false,
      alike: null,
      framing: null,
      focus: [0.5, 0.3],
    });
    const stacks: CullStack[] = [];
    for (const p of ps) {
      const n = p.id % 10;
      const prev = stacks[stacks.length - 1];
      if (n === 2 && prev?.frames[0].id === p.id - 1) {
        prev.frames[0] = { ...prev.frames[0], suggest: "reject", reasons: ["Same shot: frame 2 is better"] };
        prev.frames.push(frame(p, "keep", ["The best of these 2 frames"]));
        prev.keeper = 1;
      } else if (n === 4) {
        stacks.push({ frames: [frame(p, "reject", ["Blurry everywhere: shake, motion or focus missed"], ["blurry"])], keeper: 0 });
      } else {
        stacks.push({ frames: [frame(p, "keep", [])], keeper: 0 });
      }
    }
    return { photos: ps.length, measured: ps.length, closer: 0, described: ps.length, canLookCloser: ps.map((p) => p.id), unreadable, stacks };
  };

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

  /** A face of the unnamed person (8) in this photo, as "Find more" offers it. */
  const face = (photoId: number, similarity: number) => ({
    faceId: 1000 + photoId,
    photoId,
    personId: 8,
    similarity,
    now: 0.4,
    byHand: false,
  });

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
      case "missing_photos":
        return {
          bin: "Trash",
          binRead: true,
          photos: (a.ids as number[])
            .filter((id) => missing.has(id) && photos.some((p) => p.id === id))
            .map((id) => ({ id, paths: [`D:\Gone\IMG_${id}.jpg`], inBin: inBin.has(id), deletedAt: 0 })),
        };
      case "forget_missing": {
        const gone = (a.ids as number[]).filter((id) => missing.has(id) && photos.some((p) => p.id === id));
        for (const id of gone) photos.splice(photos.findIndex((p) => p.id === id), 1);
        return { photos: gone.length, files: gone.length };
      }
      // ask(): answered with its OK button.
      case "plugin:dialog|message":
        return (a.buttons as { OkCancelCustom?: [string, string] } | undefined)?.OkCancelCustom?.[0] ?? "Yes";
      case "mark_photos":
        for (const id of a.ids as number[]) (a.marked ? marked.add(id) : marked.delete(id));
        return (a.ids as number[]).length;
      case "cull_shoot":
        return shootOf(sorted(inScope(a.scope as Scope), "oldest"));
      case "cull_progress":
      case "cull_look_closer":
      case "cull_stop":
        return null;
      case "read_people":
        return 0;
      case "check_people":
        return checkAnswer(a.all === true);
      case "leave_as_is":
        return leaveAsIs(a.findings as LeftAsIs[], a.left as boolean);
      case "more_faces": {
        // Faces in others that look like Ana: one a day after her photos, one long before.
        if (a.personId !== 7) return [];
        const days: CandidateDay[] = [
          { day: "2026-01-06", near: true, faces: [face(102, 0.8)] },
          { day: "2024-06-01", near: false, faces: [face(202, 0.6)] },
        ];
        return days
          .map((d) => ({ ...d, faces: d.faces.filter((c) => !inPerson[7].includes(c.photoId)) }))
          .filter((d) => d.faces.length);
      }
      case "reassign_faces": {
        const ids = a.faceIds as number[];
        // From 1000, a face id is 1000 + its photo id ("Find more"): the photos move.
        const moved = ids.filter((f) => f >= 1000).map((f) => f - 1000);
        if (moved.length) {
          const to = a.to as FaceTarget;
          if (to.kind !== "person") throw new Error("only moves to someone in People are faked");
          for (const id of Object.keys(inPerson)) inPerson[+id] = inPerson[+id].filter((p) => !moved.includes(p));
          inPerson[to.id] = [...inPerson[to.id], ...moved];
          people = people.map((p) => ({ ...p, photos: inPerson[p.id].length, faces: inPerson[p.id].length }));
          return { faces: moved.length, people: [to.id], bumped: [], undo: [] };
        }
        // The check's faces: every one was Ana's (7) before, seen, Shoebox's guess.
        const undo: Placement[] = ids.map((faceId) => ({ faceId, personId: 7, byHand: false, seen: true }));
        return { faces: ids.length, people: [], bumped: [], undo };
      }
      case "restore_faces":
        return (a.placements as Placement[]).length;
      case "merge_people": {
        const into = a.into as number;
        const gone = people.filter((p) => p.id !== into && (a.ids as number[]).includes(p.id));
        people = people.filter((p) => !gone.includes(p));
        // Their photos are the merged person's now.
        for (const p of gone) {
          unmerged.set(p.id, { person: p, photos: inPerson[p.id] ?? [] });
          inPerson[into] = [...(inPerson[into] ?? []), ...(inPerson[p.id] ?? [])];
          delete inPerson[p.id];
        }
        people = people.map((p) => (p.id === into ? { ...p, photos: inPerson[into].length } : p));
        const joined = gone.map((p) => ({ id: p.id, name: p.name, ignored: p.ignored, createdAt: 0, faces: [p.cover] }));
        return { into, name: people.find((p) => p.id === into)?.name ?? null, joined } satisfies Merged;
      }
      case "unmerge_people": {
        const m = a.merged as Merged;
        let next = Math.max(...people.map((p) => p.id)) + 1;
        const back = m.joined.map((j) => {
          const { person, photos } = unmerged.get(j.id)!;
          // Their own id, unless someone new got it since.
          const id = people.some((p) => p.id === j.id) ? next++ : j.id;
          inPerson[m.into] = inPerson[m.into].filter((ph) => !photos.includes(ph));
          inPerson[id] = photos;
          return { ...person, id };
        });
        people = [...people.map((p) => (p.id === m.into ? { ...p, photos: inPerson[m.into].length } : p)), ...back];
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
    missing,
    inBin,
    /** What Cull says it couldn't read. */
    unreadableOriginals: (u: Shoot["unreadable"]) => {
      unreadable = u;
    },
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
