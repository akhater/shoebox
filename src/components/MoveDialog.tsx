import { ArrowLeft, ChevronRight, Folder, FolderInput, FolderPlus, HardDrive, Search, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { fmtBytes, fmtNum, type Browse, type FolderNode, type LeftWhy, type Overview, type PhotoMovePlan } from "../api";
import { diskPath } from "../paths";

/** A folder photos can go to. */
export interface MoveTarget {
  volumeId: number;
  relPath: string;
  /** As the sidebar shows it. */
  name: string;
  /** Where it is, as its disk writes paths. */
  path: string;
  /** Its disk's name in Shoebox. */
  disk: string;
}

/** Done without asking (with Undo after): every photo goes, all on their own disk, and - outside a
 *  folder, where it isn't plain which copy is meant - none leaves another copy behind. */
export const movesAtOnce = (plan: PhotoMovePlan, inFolder: boolean) =>
  plan.files.length > 0 && plan.left.length === 0 && !plan.files.some((f) => f.copied) && (inFolder || plan.copiesStay === 0);

const RECENT = "shoebox.moveTo";
const RECENT_KEPT = 5;

/** The last folders photos went to, newest first. */
function recentKeys(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(RECENT) ?? "[]");
    return Array.isArray(v) ? v.filter((k): k is string => typeof k === "string") : [];
  } catch {
    return [];
  }
}

/** Remembers a folder photos went to, for "Recent" next time. */
export function rememberMoveTarget(t: MoveTarget) {
  const key = folderKey(t.volumeId, t.relPath);
  try {
    localStorage.setItem(RECENT, JSON.stringify([key, ...recentKeys().filter((k) => k !== key)].slice(0, RECENT_KEPT)));
  } catch {
    /* per-viewer convenience only */
  }
}

const folderKey = (volumeId: number, relPath: string) => `${volumeId}:${relPath}`;

/** A row of the folder list: a disk (a heading) or a folder. */
interface Row {
  key: string;
  depth: number;
  disk?: { label: string; online: boolean };
  folder?: MoveTarget;
  count: number;
  /** Opens to show the folders in it. */
  kids: boolean;
  /** Why photos can't go there now. */
  off: string | null;
  /** Under a search: where it is. */
  where?: string;
}

/** Every folder of every source, disk by disk, as the sidebar has them. */
interface Tree {
  disks: { id: number; label: string; online: boolean; roots: { target: MoveTarget; count: number; off: string | null; kids: FolderNode[] }[] }[];
}

function treeOf(overview: Overview | null, browse: Browse | null): Tree {
  const kids = new Map((browse?.folders ?? []).map((f) => [f.rootId, f.children]));
  return {
    disks: (overview?.volumes ?? [])
      .filter((v) => v.roots.length)
      .sort((a, b) => Number(b.online) - Number(a.online))
      .map((v) => ({
        id: v.id,
        label: v.label,
        online: v.online,
        // By path, as the sidebar lists them.
        roots: [...v.roots].sort((a, b) => a.path.localeCompare(b.path, undefined, { numeric: true, sensitivity: "base" })).map((r) => ({
          target: { volumeId: v.id, relPath: r.relPath, name: r.name, path: r.path, disk: v.label },
          count: r.photos,
          off: !v.online ? "Its disk is unplugged" : !r.found ? "Not found on its disk" : null,
          kids: kids.get(r.id) ?? [],
        })),
      })),
  };
}

const LEFT_WHY: Record<LeftWhy, (n: number, to: string) => string> = {
  here: (n, to) => `${n === 1 ? "1 photo is" : `${fmtNum(n)} photos are`} already in “${to}”.`,
  copyThere: (n, to) =>
    `${n === 1 ? "1 photo has a copy" : `${fmtNum(n)} photos have copies`} in “${to}” already, under the same name: ${n === 1 ? "it stays" : "they stay"} where ${n === 1 ? "it is" : "they are"}.`,
  unplugged: (n) => `${n === 1 ? "1 photo is" : `${fmtNum(n)} photos are`} on an unplugged disk: plug it in to move ${n === 1 ? "it" : "them"}.`,
  notFound: (n) =>
    `${n === 1 ? "1 photo isn't" : `${fmtNum(n)} photos aren't`} where the catalog says (file or folder not found): ${n === 1 ? "it stays" : "they stay"}.`,
  missing: (n) => `${n === 1 ? "1 photo is" : `${fmtNum(n)} photos are`} missing from every disk: nothing to move.`,
  taken: () => "",
};
/** "IMG_1.JPG, IMG_2.JPG, IMG_3.JPG…" */
const someNames = (names: string[]) => names.slice(0, 3).join(", ") + (names.length > 3 ? "…" : "");
const ROLE_WORDS = {
  twin: ["JPEG twin of a RAW file", "JPEG twins of RAW files"],
  edits: ["edit file", "edit files"],
  look: ["look file", "look files"],
} as const;
const SHOWN_FILES = 500;

interface Props {
  /** How many photos are moving (a focus stack's frames each count). */
  count: number;
  overview: Overview | null;
  browse: Browse | null;
  /** The folder shown now, when it is one: its photos' copies there are the ones that move. */
  here: { volumeId: number; relPath: string } | null;
  /** Dropped on a folder: its plan, to confirm. Null: pick a folder first. */
  start: { to: MoveTarget; plan: PhotoMovePlan } | null;
  /** What moving the photos into `to` would do. */
  plan: (to: MoveTarget, numberClashes: boolean) => Promise<PhotoMovePlan>;
  /** Move them, as `plan` says. */
  onMove: (to: MoveTarget, plan: PhotoMovePlan, numberClashes: boolean) => void;
  /** "New folder" inside a folder: its path on its disk, or null if it failed. */
  onNewFolder: (volumeId: number, parentRel: string, name: string) => Promise<string | null>;
  onClose: () => void;
}

/** "Move to…": pick a folder (search, recent ones, every source's folders, or a new one), then -
 *  when the move isn't plain - what it will do, before it is done. */
export function MoveDialog(p: Props) {
  const tree = useMemo(() => treeOf(p.overview, p.browse), [p.overview, p.browse]);
  const [stage, setStage] = useState(p.start);
  const [numbering, setNumbering] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [picked, setPicked] = useState<MoveTarget | null>(null);
  const [naming, setNaming] = useState(false);
  const what = p.count === 1 ? "1 photo" : `${fmtNum(p.count)} photos`;

  // Sources open, and the folders down to the one shown now.
  const [open, setOpen] = useState<Set<string>>(() => {
    const keys = new Set<string>();
    for (const d of tree.disks)
      for (const r of d.roots) {
        keys.add(folderKey(d.id, r.target.relPath));
        if (p.here?.volumeId === d.id && p.here.relPath.startsWith(r.target.relPath + "\\")) {
          const parts = p.here.relPath.slice(r.target.relPath.length + 1).split("\\");
          parts.slice(0, -1).forEach((_, i) => keys.add(folderKey(d.id, [r.target.relPath, ...parts.slice(0, i + 1)].join("\\"))));
        }
      }
    return keys;
  });
  const toggle = (key: string) =>
    setOpen((o) => {
      const n = new Set(o);
      if (!n.delete(key)) n.add(key);
      return n;
    });

  const rows = useMemo(() => {
    const out: Row[] = [];
    const q = query.trim().toLowerCase();
    const node = (n: FolderNode, d: Tree["disks"][number], mount: string, depth: number) => {
      const target = { volumeId: d.id, relPath: n.relPath, name: n.name, path: diskPath(mount, n.relPath), disk: d.label };
      const key = folderKey(d.id, n.relPath);
      const off = d.online ? null : "Its disk is unplugged";
      if (!q) out.push({ key, depth, folder: target, count: n.count, kids: n.children.length > 0, off });
      else if (n.name.toLowerCase().includes(q)) out.push({ key, depth: 0, folder: target, count: n.count, kids: false, off, where: target.path });
      if (q || open.has(key)) n.children.forEach((c) => node(c, d, mount, depth + 1));
    };
    for (const d of tree.disks) {
      const v = p.overview?.volumes.find((x) => x.id === d.id);
      const mount = v?.mount ?? v?.lastMount ?? "?:\\";
      if (!q) out.push({ key: `disk:${d.id}`, depth: 0, disk: { label: d.label, online: d.online }, count: 0, kids: false, off: null });
      for (const r of d.roots) {
        const key = folderKey(d.id, r.target.relPath);
        if (!q) out.push({ key, depth: 1, folder: r.target, count: r.count, kids: r.kids.length > 0, off: r.off });
        else if (r.target.name.toLowerCase().includes(q))
          out.push({ key, depth: 0, folder: r.target, count: r.count, kids: false, off: r.off, where: r.target.path });
        if (q || open.has(key)) r.kids.forEach((n) => node(n, d, mount, q ? 0 : 2));
      }
    }
    return out;
  }, [tree, open, query, p.overview]);

  // Recent folders that are still there.
  const recent = useMemo(() => {
    if (query.trim()) return [];
    const all = new Map<string, Row>();
    const walk = (n: FolderNode, d: Tree["disks"][number], mount: string) => {
      const key = folderKey(d.id, n.relPath);
      const target = { volumeId: d.id, relPath: n.relPath, name: n.name, path: diskPath(mount, n.relPath), disk: d.label };
      all.set(key, { key: `recent:${key}`, depth: 0, folder: target, count: n.count, kids: false, off: d.online ? null : "Its disk is unplugged", where: target.path });
      n.children.forEach((c) => walk(c, d, mount));
    };
    for (const d of tree.disks) {
      const v = p.overview?.volumes.find((x) => x.id === d.id);
      const mount = v?.mount ?? v?.lastMount ?? "?:\\";
      for (const r of d.roots) {
        const key = folderKey(d.id, r.target.relPath);
        all.set(key, { key: `recent:${key}`, depth: 0, folder: r.target, count: r.count, kids: false, off: r.off, where: r.target.path });
        r.kids.forEach((n) => walk(n, d, mount));
      }
    }
    return recentKeys()
      .map((k) => all.get(k))
      .filter((r): r is Row => !!r && !r.off);
  }, [tree, query, p.overview]);

  const pickable = useMemo(() => [...recent, ...rows].filter((r) => r.folder && !r.off), [recent, rows]);
  const pickedKey = picked ? folderKey(picked.volumeId, picked.relPath) : null;
  // A search shows only what it finds: the folder picked is one of them (its first, else none).
  useEffect(() => {
    if (!query.trim() || !pickedKey) return;
    if (!pickable.some((r) => r.key === pickedKey)) setPicked(pickable[0]?.folder ?? null);
  }, [query, pickable, pickedKey]);
  const isHere = (t: MoveTarget) => p.here?.volumeId === t.volumeId && p.here.relPath === t.relPath;

  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    list.current?.querySelector(".move-row.picked")?.scrollIntoView({ block: "nearest" });
  }, [pickedKey]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || busy) return;
      e.stopPropagation(); // not also Back
      if (naming) setNaming(false);
      else p.onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [busy, naming, p]);

  const choose = async (to: MoveTarget, number = numbering) => {
    setBusy(true);
    setError(null);
    try {
      const plan = await p.plan(to, number);
      if (!stage && movesAtOnce(plan, p.here != null)) return p.onMove(to, plan, number);
      setStage({ to, plan });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const newFolder = async (name: string) => {
    const parent = picked;
    setNaming(false);
    if (!parent || !name.trim()) return;
    const rel = await p.onNewFolder(parent.volumeId, parent.relPath, name.trim());
    if (rel == null) return;
    setOpen((o) => new Set(o).add(folderKey(parent.volumeId, parent.relPath)));
    const shown = rel.slice(rel.lastIndexOf("\\") + 1);
    setPicked({ ...parent, relPath: rel, name: shown, path: `${parent.path}${parent.path.startsWith("/") ? "/" : "\\"}${shown}` });
  };

  const onListKey = (e: ReactKeyboardEvent) => {
    const i = pickable.findIndex((r) => r.key.replace(/^recent:/, "") === pickedKey);
    const at = pickable[i];
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const next = pickable[e.key === "ArrowDown" ? Math.min(pickable.length - 1, i + 1) : Math.max(0, i - 1)];
      if (next?.folder) setPicked(next.folder);
    } else if (e.key === "ArrowRight" && at?.kids && !open.has(at.key)) toggle(at.key);
    else if (e.key === "ArrowLeft" && at?.kids && open.has(at.key)) toggle(at.key);
    else if (e.key === "Enter" && !picked && pickable[0]?.folder) setPicked(pickable[0].folder);
    else if (e.key === "Enter" && picked && !isHere(picked)) choose(picked);
    else return;
    e.preventDefault();
  };

  const row = (r: Row) => {
    if (r.disk)
      return (
        <div key={r.key} className={`move-disk ${r.disk.online ? "" : "off"}`}>
          <HardDrive size={13} />
          <span className="grow ellipsis">{r.disk.label}</span>
          {!r.disk.online && <span className="muted small">unplugged</span>}
        </div>
      );
    const t = r.folder!;
    const key = r.key.replace(/^recent:/, "");
    const here = isHere(t);
    return (
      <div key={r.key}>
        <div
          className={`move-row ${key === pickedKey ? "picked" : ""} ${r.off || here ? "off" : ""}`}
          style={{ paddingLeft: 8 + r.depth * 16 }}
          title={r.off ?? (here ? "The photos are shown from here" : t.path)}
          onClick={() => !r.off && setPicked(t)}
          onDoubleClick={() => !r.off && !here && choose(t)}
        >
          {r.kids ? (
            <span
              className="twisty"
              onClick={(e) => {
                e.stopPropagation();
                toggle(key);
              }}
            >
              <ChevronRight size={12} className={`chev ${open.has(key) ? "open" : ""}`} />
            </span>
          ) : (
            <span className="twisty-space" />
          )}
          <Folder size={14} />
          <span className="grow move-name">
            <span className="ellipsis">{t.name}</span>
            {r.where && <span className="muted small ellipsis">{r.where}</span>}
          </span>
          {here && <span className="muted small">shown now</span>}
          <span className="count muted small">{fmtNum(r.count)}</span>
        </div>
        {naming && !r.where && key === pickedKey && (
          <div className="move-row naming" style={{ paddingLeft: 8 + (r.depth + 1) * 16 }}>
            <span className="twisty-space" />
            <FolderPlus size={14} />
            <input
              autoFocus
              className="grow"
              placeholder="New folder"
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") newFolder(e.currentTarget.value || "New folder");
              }}
              onBlur={() => setNaming(false)}
            />
          </div>
        )}
      </div>
    );
  };

  if (stage) {
    const { to, plan } = stage;
    const copied = plan.files.some((f) => f.copied);
    const byWhy = new Map<LeftWhy, number>();
    plan.left.forEach((l) => byWhy.set(l.why, (byWhy.get(l.why) ?? 0) + 1));
    const taken = plan.left.filter((l) => l.why === "taken");
    const renamed = plan.files.filter((f) => f.name !== f.from.slice(Math.max(f.from.lastIndexOf("\\"), f.from.lastIndexOf("/")) + 1));
    const roles = (["twin", "edits", "look"] as const)
      .map((role) => [role, plan.files.filter((f) => f.role === role).length] as const)
      .filter(([, n]) => n > 0)
      .map(([role, n]) => `${fmtNum(n)} ${ROLE_WORDS[role][n === 1 ? 0 : 1]}`);
    const moving = plan.photos === 1 ? "1 photo" : `${fmtNum(plan.photos)} photos`;
    return (
      <div className="modal-back" onClick={busy ? undefined : p.onClose}>
        <div className="modal move-modal" onClick={(e) => e.stopPropagation()}>
          <div className="modal-head">
            <h2>
              <button
                className="icon-btn"
                onClick={() => {
                  setStage(null);
                  setPicked(to);
                }}
                aria-label="Choose another folder"
                title="Choose another folder"
                disabled={busy}
              >
                <ArrowLeft size={17} />
              </button>
              Move {what} to “{to.name}”
            </h2>
            <button className="icon-btn" onClick={p.onClose} aria-label="Close" disabled={busy}>
              <X size={18} />
            </button>
          </div>
          <p className="muted small move-to-path">{to.path}</p>

          {plan.files.length > 0 ? (
            <div className="move-summary">
              <FolderInput size={18} />
              <div className="grow">
                <b>
                  {moving} · {fmtNum(plan.files.length)} {plan.files.length === 1 ? "file" : "files"} · {fmtBytes(plan.bytes)}
                </b>
                <span className="muted small">
                  {copied
                    ? `To another disk (${to.disk}): copied, checked byte for byte, then the originals removed. If anything goes wrong, or you press Stop, the photos not moved yet stay where they are.`
                    : `On the same disk: moved at once, nothing copied.`}
                </span>
                {roles.length > 0 && <span className="muted small">With {roles.join(" and ")}, which go along.</span>}
              </div>
            </div>
          ) : (
            <p>Nothing to move.</p>
          )}

          {(taken.length > 0 || renamed.length > 0 || numbering) && (
            <div className="note">
              {numbering
                ? `${renamed.length === 1 ? "1 file gets" : `${fmtNum(renamed.length)} files get`} a number to keep both: ${someNames(renamed.map((f) => f.name)).replace(/[^…]$/, "$&.")}`
                : taken.length === 1
                  ? `${taken[0].name} stays where it is: “${to.name}” already has a file of that name.`
                  : `${fmtNum(taken.length)} photos stay where they are: “${to.name}” already has files of their names (${someNames(taken.map((l) => l.name))}).`}{" "}
              Nothing is ever replaced.
              <label className="move-check">
                <input
                  type="checkbox"
                  checked={numbering}
                  disabled={busy}
                  onChange={(e) => {
                    setNumbering(e.target.checked);
                    choose(to, e.target.checked);
                  }}
                />
                {numbering || taken.length !== 1
                  ? "Keep both: the photos coming in get a number"
                  : `Keep both: the one coming in becomes ${numberedExample(taken[0].name)}`}
              </label>
            </div>
          )}
          {(["here", "copyThere", "unplugged", "notFound", "missing"] as const).map(
            (why) =>
              byWhy.has(why) && (
                <p key={why} className="note">
                  {LEFT_WHY[why](byWhy.get(why)!, to.name)}
                </p>
              ),
          )}
          {p.here == null && plan.copiesStay > 0 && (
            <p className="note">
              {plan.copiesStay === 1 ? "1 also has a copy" : `${fmtNum(plan.copiesStay)} also have copies`} somewhere else: one copy of
              each moves (the one on {to.disk} when there is one), the others stay where they are.
            </p>
          )}

          {plan.files.length > 0 && (
            <details className="del-files">
              <summary>Show the {fmtNum(plan.files.length)} files</summary>
              <ul>
                {plan.files.slice(0, SHOWN_FILES).map((f) => (
                  <li key={f.from} title={f.from}>
                    <span className="path">{f.from}</span>
                    <span className="muted">
                      {renamed.includes(f) ? `as ${f.name} · ` : ""}
                      {fmtBytes(f.size)}
                    </span>
                  </li>
                ))}
                {plan.files.length > SHOWN_FILES && <li className="muted">and {fmtNum(plan.files.length - SHOWN_FILES)} more</li>}
              </ul>
            </details>
          )}

          {error && <p className="note warn">{error}</p>}

          <div className="modal-foot">
            <button className="btn" onClick={p.onClose} disabled={busy}>
              {plan.files.length ? "Cancel" : "Close"}
            </button>
            {plan.files.length > 0 && (
              <button className="btn primary" disabled={busy} onClick={() => p.onMove(to, plan, numbering)}>
                <FolderInput size={15} />
                Move {moving}
                {copied ? ` (${fmtBytes(plan.copyBytes)})` : ""}
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="modal-back" onClick={busy ? undefined : p.onClose}>
      <div className="modal move-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Move {what}</h2>
          <button className="icon-btn" onClick={p.onClose} aria-label="Close" disabled={busy}>
            <X size={18} />
          </button>
        </div>
        <label className="move-search">
          <Search size={14} />
          <input
            autoFocus
            placeholder="Find a folder"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onListKey}
          />
        </label>
        <div className="move-list" ref={list} tabIndex={-1} onKeyDown={onListKey}>
          {recent.length > 0 && (
            <>
              <div className="move-heading">Recent</div>
              {recent.map(row)}
              <div className="move-heading">Folders</div>
            </>
          )}
          {rows.map(row)}
          {rows.length === 0 && <p className="muted small pad">{query.trim() ? "No folder by that name." : "No folders yet."}</p>}
        </div>
        {error && <p className="note warn">{error}</p>}
        <div className="modal-foot">
          <button
            className="btn ghost move-new"
            disabled={!picked || busy || !!query.trim()}
            onClick={() => setNaming(true)}
            title={picked && !query.trim() ? `A new folder in “${picked.name}”` : "Pick the folder to make it in"}
          >
            <FolderPlus size={15} /> New folder
          </button>
          <span className="grow" />
          <button className="btn" onClick={p.onClose} disabled={busy}>
            Cancel
          </button>
          <button className="btn primary" disabled={!picked || busy || isHere(picked)} onClick={() => picked && choose(picked)}>
            <FolderInput size={15} />
            {busy ? "Checking…" : "Move here"}
          </button>
        </div>
      </div>
    </div>
  );
}

/** "IMG_0001 (2).CR3" for "IMG_0001.CR3". */
function numberedExample(name: string) {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? `${name.slice(0, dot)} (2)${name.slice(dot)}` : `${name} (2)`;
}
