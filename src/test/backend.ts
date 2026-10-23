import { mockIPC } from "@tauri-apps/api/mocks";
import type {
  CandidateDay,
  CullFrame,
  CullStack,
  DateChange,
  DateCheck,
  FaceTarget,
  Facets,
  Filters,
  Focus,
  Found,
  LeftAsIs,
  Merged,
  Overview,
  PeopleCheck,
  Person,
  PhotoDetail,
  PhotoList,
  PhotoMoveAsk,
  PhotoMovePlan,
  PhotoMoveResult,
  Placement,
  Problem,
  Repoint,
  Scope,
  Shifted,
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
  /** Photos the upright model would turn (a test sets them): quarter turns clockwise. */
  const sideways = new Map<number, number>();
  /** How photos were turned, and their previews' revisions (a turn makes a new one). */
  const turns = new Map<number, number>();
  const revs = new Map<number, number>();
  /** Where the camera focused, for the photos a test says. */
  const focusOf = new Map<number, Focus>();
  /** Dates moved by "Fix dates": seconds added to each photo's date. */
  const shifts = new Map<number, number>();
  const fileDate = (id: number) => JAN_2026 + id * 3600;
  const takenOf = (id: number) => fileDate(id) + (shifts.get(id) ?? 0);
  const redate = (moves: [number, number][]): Shifted => {
    const done = moves
      .filter(([, by]) => by !== 0)
      .map(([id, by]) => {
        const from = takenOf(id);
        shifts.set(id, (shifts.get(id) ?? 0) + by);
        return { id, by, from, to: from + by };
      });
    return { photos: done, unchanged: moves.length - done.length };
  };
  /** The date check's one finding (Trip's photos, five days early) was left as is. */
  let datesLeft = false;
  const dateCheck = (): DateCheck => {
    const ids = photosOf(30);
    const first = takenOf(ids[0]);
    return {
      days: 3,
      folders: 1,
      photos: 4,
      corrected: [...shifts.values()].filter(Boolean).length,
      left: datesLeft ? 1 : 0,
      findings: [
        {
          folderId: 3,
          volumeId: 1,
          relPath: "2026-01-06 Trip\\Guest",
          dated: "2026-01-06 Trip",
          from: "2026-01-06",
          to: "2026-01-06",
          camera: "EOS 30D",
          serial: null,
          fileDate: false,
          key: "EOS 30D||camera",
          photos: ids,
          first,
          last: takenOf(ids[3]),
          daysOff: -5,
          shift: 5 * 86_400,
          others: 0,
          othersFrom: null,
          othersTo: null,
          corrected: 0,
          left: datesLeft,
        },
      ],
    };
  };
  /** What Cull found wrong with photos, as their details say it (a test sets them). */
  const foundIn = new Map<number, Found[]>();
  /** Private photos, by password: each password is its own space, never refused. */
  const spaces = new Map<string, Set<number>>();
  let open: string | null = null;
  const mine = () => (open == null ? new Set<number>() : (spaces.get(open) ?? new Set<number>()));
  const isPrivate = (id: number) => [...spaces.values()].some((s) => s.has(id));
  const shownPhotos = () => photos.filter((p) => !isPrivate(p.id) || mine().has(p.id));
  /** Names by password; an empty space's is forgotten once locked, as Shoebox does. */
  const names = new Map<string, string>();
  const status = () => ({ unlocked: open != null, photos: mine().size, name: (open != null && names.get(open)) || null });
  let settings: Record<string, unknown> = { privateLockMinutes: 5, privateWarning: true };
  const hide = (ids: number[], confirm: unknown) => {
    if (open == null) throw new Error("Private photos are locked.");
    if (mine().size === 0 && confirm == null) throw new Error("Type the password again: no photos are private with it yet.");
    if (mine().size === 0 && confirm !== open) throw new Error("The two passwords aren't the same.");
    const space = spaces.get(open) ?? new Set<number>();
    spaces.set(open, space);
    const made = ids.filter((id) => !isPrivate(id));
    made.forEach((id) => space.add(id));
    return made.length;
  };
  const turn = (ids: number[], by: (id: number) => number) => {
    const photos = ids.map((id) => {
      const q = ((by(id) % 4) + 4) % 4;
      turns.set(id, ((turns.get(id) ?? 0) + q) % 4);
      revs.set(id, (revs.get(id) ?? 1) + 1);
      sideways.delete(id);
      const odd = turns.get(id)! % 2 === 1;
      return { id, by: q, turn: turns.get(id)!, rev: revs.get(id)!, w: odd ? 1000 : 1500, h: odd ? 1500 : 1000 };
    });
    return { turned: photos.length, faces: 0, toReadAgain: [], photos };
  };
  /** Originals Cull couldn't read for a closer look (a test sets them). */
  let unreadable: Shoot["unreadable"] = [];
  /** Frames Cull finds soft where the camera focused (a test sets them, with `focusOf`). */
  const focusMissed = new Set<number>();
  /** Photos Cull hasn't measured yet (a test sets them), with what it finds once asked (cull_flags). */
  const unmeasured = new Map<number, number>();
  const measuredAs = new Map<number, number>();
  // The fourth photo of each source is blurry.
  const flagsOf = (id: number) => (unmeasured.has(id) ? 0 : (measuredAs.get(id) ?? (id % 10 === 4 ? 4 : 0)));
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
  /** Sources not found where they were, and where they were found (a test sets them). */
  const elsewhere = new Map<number, Repoint>();
  /** Extra time for some answers: by command, and by view (`viewName`) for the commands named. */
  const cmdDelays = new Map<string, number>();
  const viewDelays = new Map<string, { ms: number; cmds: string[] }>();
  const extra = (cmd: string, scope: Scope | undefined) => {
    const v = scope && viewDelays.get(viewName(scope));
    return (cmdDelays.get(cmd) ?? 0) + (v && v.cmds.includes(cmd) ? v.ms : 0);
  };

  const overview = (): Overview => {
    const photos = shownPhotos();
    const roots = (here: boolean, mount: string) =>
      ROOTS.filter((r) => elsewhere.has(r.id) !== here).map((r) => ({
        id: r.id,
        found: true,
        name: r.name,
        path: `${mount}${r.name}`,
        relPath: r.name,
        lastScan: null,
        photos: 4,
        elsewhere: elsewhere.get(r.id) ?? null,
      }));
    const away = roots(false, "C:\\");
    return {
      volumes: [
        {
          id: 1,
          label: "Disk",
          online: true,
          mount: "D:\\",
          lastMount: "D:\\",
          photos: photos.length,
          roots: roots(true, "D:\\"),
        },
        // Sources found elsewhere: on a disk unplugged for good (an old computer's).
        ...(away.length
          ? [{ id: 2, label: "OS (C:)", online: false, mount: null, lastMount: "C:\\", photos: away.length * 4, roots: away }]
          : []),
      ],
      total: photos.length,
      online: photos.length,
      offline: 0,
      missing: photos.filter((p) => missing.has(p.id)).length,
      marked: marked.size,
      sideways: photos.filter((p) => sideways.has(p.id)).length,
      private: mine().size,
      dataDir: "data",
    };
  };

  const inScope = (s: Scope): Photo[] => {
    const photos = shownPhotos();
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
      case "sideways":
        return photos.filter((p) => sideways.has(p.id));
      case "private":
        return photos.filter((p) => mine().has(p.id));
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
    rev: ps.map((p) => revs.get(p.id) ?? 1),
    preview: ps.map(() => 1),
    marked: ps.map((p) => +marked.has(p.id)),
    ext: ps.map(() => "jpg"),
    raw: ps.map(() => 0),
    fresh: ps.map(() => 0),
    flags: ps.map((p) => flagsOf(p.id)),
    sideways: ps.map((p) => sideways.get(p.id) ?? 0),
    private: ps.map((p) => +isPrivate(p.id)),
  });

  /**
   * Cull's shoot: in each source, its first two photos are a burst (the
   * second is sharper and kept), the third is fine, the fourth blurry. A
   * photo whose file says where the camera focused is looked at there first.
   */
  const shootOf = (ps: Photo[]): Shoot => {
    const frame = (p: Photo, suggest: CullFrame["suggest"], reasons: string[], problems: Problem[] = []): CullFrame => {
      const said = focusOf.get(p.id)?.points ?? [];
      const inFocus = said.filter((f) => f.state === "inFocus");
      const focusPoints = inFocus.length ? inFocus : said.filter((f) => f.state === "chosen");
      const missed = focusMissed.has(p.id);
      const at = focusPoints[0];
      return {
        id: p.id,
        name: `IMG_${p.id}.jpg`,
        taken: p.taken,
        state: 2,
        w: 150,
        h: 100,
        rev: 1,
        marked: marked.has(p.id),
        suggest: missed && suggest === "keep" ? "none" : suggest,
        reasons: missed ? [...reasons, "Focus missed: the camera focused here, but the sharpest part is elsewhere"] : reasons,
        problems: missed ? [...problems, "focusMissed"] : problems,
        blur: 0.3,
        closer: false,
        alike: null,
        framing: null,
        focus: at ? [at.x + at.w / 2, at.y + at.h / 2] : [0.5, 0.3],
        focusPoints,
      };
    };
    const stacks: CullStack[] = [];
    for (const p of ps) {
      const n = p.id % 10;
      const prev = stacks[stacks.length - 1];
      if (n === 2 && prev?.frames[0].id === p.id - 1) {
        prev.frames[0] = { ...prev.frames[0], suggest: "reject", reasons: ["Same shot: frame 2 is better"] };
        prev.frames.push(frame(p, "keep", ["The best of these 2 frames"]));
        prev.keeper = 1;
      } else if (n === 4) {
        stacks.push({ frames: [frame(p, "reject", ["Blurry everywhere: shake, motion or focus missed"], ["blurry"])], keeper: 0, compared: "sharpest", closer: false, kind: "shot" });
      } else {
        stacks.push({ frames: [frame(p, "keep", [])], keeper: 0, compared: "sharpest", closer: false, kind: "shot" });
      }
    }
    // Compared where the camera focused when every frame's file says and the point is sharp
    // in one of them at least (no faces measured here).
    for (const s of stacks) {
      const atPoint = s.frames.every((f) => f.focusPoints.length) && s.frames.some((f) => !f.problems.includes("focusMissed"));
      s.compared = atPoint ? "focusPoint" : "sharpest";
    }
    return { photos: ps.length, measured: ps.length, closer: 0, described: ps.length, canLookCloser: ps.map((p) => p.id), unreadable, stacks };
  };

  const sorted = (ps: Photo[], sort: Sort) =>
    sort === "newest" ? [...ps].sort((a, b) => b.taken - a.taken) : [...ps].sort((a, b) => a.taken - b.taken);

  const facets = (s: Scope): Facets => {
    const here = inScope(s);
    const priv = here.filter((p) => isPrivate(p.id)).length;
    return {
      dates: [],
      cameras: [],
      lenses: [],
      types: [],
      status: [],
      people: [],
      tags: [],
      private:
        open == null
          ? []
          : [
              { value: "private", count: priv },
              { value: "notPrivate", count: here.length - priv },
            ].filter((v) => v.count > 0),
    };
  };
  /** The Private filter, as Shoebox applies it: only while a space is open. */
  const privateFilter = (ps: Photo[], f: Filters | undefined) => {
    const want = f?.private ?? [];
    if (open == null || want.length !== 1) return ps;
    return ps.filter((p) => isPrivate(p.id) === (want[0] === "private"));
  };

  const detail = (id: number): PhotoDetail => {
    const root = ROOTS.find((r) => r.id === photos.find((p) => p.id === id)?.root)!;
    return {
      id,
      kind: "image",
      takenTs: takenOf(id),
      takenFromCamera: true,
      takenOriginal: shifts.get(id) ? fileDate(id) : null,
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
      previewRev: revs.get(id) ?? 1,
      error: null,
      turn: turns.get(id) ?? 0,
      sideways: sideways.get(id) ?? 0,
      focus: focusOf.get(id) ?? null,
      stack: null,
      problems: foundIn.get(id) ?? [],
      private: isPrivate(id),
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

  /** Moving photos into another source's folder (a folder is a source's name; one disk, so
   *  nothing is copied). A test says which photos meet a file of their name there. */
  const nameTakenIn = new Map<string, Set<number>>();
  const fileName = (id: number) => `IMG_${id}.JPG`;
  const rootNamed = (name: string) => ROOTS.find((r) => r.name === name)!;
  const movePlan = (a: PhotoMoveAsk): PhotoMovePlan => {
    const files: PhotoMovePlan["files"] = [];
    const left: PhotoMovePlan["left"] = [];
    for (const id of a.photoIds) {
      const p = photos.find((x) => x.id === id)!;
      const taken = nameTakenIn.get(a.toRel)?.has(id) ?? false;
      if (p.root === rootNamed(a.toRel).id) left.push({ photoId: id, name: fileName(id), why: "here" });
      else if (taken && !a.numberClashes) left.push({ photoId: id, name: fileName(id), why: "taken" });
      else {
        const from = `D:\\${ROOTS.find((r) => r.id === p.root)!.name}\\${fileName(id)}`;
        files.push({ photoId: id, from, name: taken ? `IMG_${id} (2).JPG` : fileName(id), size: 2_000_000, role: "photo", copied: false });
      }
    }
    return {
      to: `D:\\${a.toRel}`,
      photos: files.length,
      files,
      bytes: files.length * 2_000_000,
      copyBytes: 0,
      numbered: files.filter((f) => !f.from.endsWith(f.name)).length,
      copiesStay: 0,
      left,
      token: JSON.stringify(a),
    };
  };
  const moveResult = (moved: PhotoMoveResult["moved"], plan?: PhotoMovePlan): PhotoMoveResult => ({
    photos: moved.length,
    files: moved.length,
    bytes: moved.length * 2_000_000,
    copied: 0,
    numbered: plan?.numbered ?? 0,
    left: plan?.left ?? [],
    failed: [],
    leftBehind: [],
    stopped: false,
    moved,
  });

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
        return listOf(sorted(privateFilter(inScope(a.scope as Scope), a.filters as Filters), a.sort as Sort));
      case "search_photos": {
        const here = new Set(inScope(a.scope as Scope).map((p) => p.id));
        const found = SEARCH.ids.filter((id) => here.has(id)).map((id) => photos.find((p) => p.id === id)!);
        return { list: listOf(found), clear: SEARCH.clear };
      }
      case "facets":
        return facets(a.scope as Scope);
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
      case "cull_flags":
        return (a.ids as number[]).map((id) => {
          if (unmeasured.has(id)) measuredAs.set(id, unmeasured.get(id)!);
          unmeasured.delete(id);
          return flagsOf(id);
        });
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
      case "turn_photos":
        return turn(a.ids as number[], () => a.by as number);
      case "turn_back":
        return turn(a.ids as number[], (id) => -(turns.get(id) ?? 0));
      case "sideways_accept": {
        const ids = (a.ids as number[]).filter((id) => sideways.has(id));
        return turn(ids, (id) => sideways.get(id)!);
      }
      case "sideways_leave": {
        const ids = (a.ids as number[]).filter((id) => sideways.delete(id));
        return ids.length;
      }
      case "private_status":
        return status();
      case "private_unlock":
        if (String(a.password).length < 6) throw new Error("A password needs at least 6 characters.");
        open = a.password as string;
        return status();
      case "private_lock":
        if (open != null && mine().size === 0) names.delete(open);
        open = null;
        return status();
      case "private_rename":
        if (open == null) throw new Error("Private photos are locked.");
        names.set(open, String(a.name).trim());
        return status();
      case "private_hide":
        return hide(a.ids as number[], a.confirm);
      case "private_hide_in":
        return hide(
          inScope(a.scope as Scope).map((p) => p.id),
          a.confirm,
        );
      case "private_show": {
        if (open == null) throw new Error("Private photos are locked.");
        const space = mine();
        return (a.ids as number[]).filter((id) => space.delete(id)).length;
      }
      case "check_dates":
        return dateCheck();
      case "leave_dates":
        datesLeft = a.left as boolean;
        return 1;
      case "dates_of": {
        const ids = a.ids as number[];
        const ts = ids.map(takenOf);
        return {
          photos: ids.length,
          dated: ids.length,
          first: Math.min(...ts),
          last: Math.max(...ts),
          corrected: ids.filter((id) => shifts.get(id)).length,
          folder: null,
          folderDate: null,
        };
      }
      case "change_dates": {
        const ids = a.ids as number[];
        const how = a.how as DateChange;
        const firstDay = Math.floor(Math.min(...ids.map(takenOf)) / 86_400) * 86_400;
        const by = how.kind === "by" ? how.seconds : how.kind === "to" ? Date.parse(`${how.date}T00:00:00Z`) / 1000 - firstDay : 0;
        return redate(ids.map((id) => [id, how.kind === "back" ? -(shifts.get(id) ?? 0) : by]));
      }
      case "undo_dates":
        return redate((a.done as Shifted).photos.map((p) => [p.id, -p.by]));
      case "get_settings":
        return { ...settings };
      case "save_settings":
        settings = { ...(a.settings as Record<string, unknown>) };
        return { ...settings };
      case "check_repoint":
        return elsewhere.get(a.rootId as number) ?? { rootId: a.rootId, path: a.path, disk: "Disk", checked: 4, found: 0, problem: "not there" };
      case "repoint_folder": {
        const r = elsewhere.get(a.rootId as number)!;
        elsewhere.delete(r.rootId);
        return r;
      }
      case "scan_status":
        return { current: null, queued: [] };
      case "plan_photo_move":
        return movePlan(a.ask as PhotoMoveAsk);
      case "move_photos": {
        const ask = a.ask as PhotoMoveAsk;
        const plan = movePlan(ask);
        if (a.token !== plan.token) throw new Error("The photos or the folder changed since you looked.");
        const moved = plan.files.map((f) => {
          const p = photos.find((x) => x.id === f.photoId)!;
          const was = { volumeId: 1, rel: ROOTS.find((r) => r.id === p.root)!.name, name: fileName(p.id) };
          p.root = rootNamed(ask.toRel).id;
          return { now: { volumeId: 1, rel: ask.toRel, name: f.name }, was };
        });
        return moveResult(moved, plan);
      }
      case "move_photos_back": {
        const moved = a.moved as PhotoMoveResult["moved"];
        for (const m of moved) photos.find((p) => fileName(p.id) === m.was.name)!.root = rootNamed(m.was.rel).id;
        return moveResult(moved);
      }
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
    /** Photos meeting a file of their name in a folder (a source's name) when moved there. */
    nameTakenIn,
    /** The source a photo is in now. */
    rootOf: (id: number) => photos.find((p) => p.id === id)!.root,
    elsewhere,
    inBin,
    sideways,
    turns,
    /** Seconds each photo's date was moved by Fix dates (0 or none: as its file says). */
    shifts,
    focusOf,
    foundIn,
    /** Photos private under a password (its space), as a test sets them. */
    privateIn: (password: string, ids: number[]) => spaces.set(password, new Set([...(spaces.get(password) ?? []), ...ids])),
    /** The private photos of a password now. */
    privateOf: (password: string) => [...(spaces.get(password) ?? [])].sort(),
    focusMissed,
    /** What Cull says it couldn't read. */
    unreadableOriginals: (u: Shoot["unreadable"]) => {
      unreadable = u;
    },
    /** A photo Cull hasn't measured yet, and what it finds once asked (bits of PROBLEMS). */
    unmeasured: (id: number, flags: number) => {
      unmeasured.set(id, flags);
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
