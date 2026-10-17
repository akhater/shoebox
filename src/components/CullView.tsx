import { listen } from "@tauri-apps/api/event";
import { Check, CheckCheck, Focus, LoaderCircle, ScanSearch, Square, Star, Trash2, Undo2, X } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  cull,
  fmtNum,
  previewUrl,
  thumbUrl,
  type AiProgress,
  type CloserProgress,
  type CullFrame,
  type CullStack,
  type FileLoc,
  type Filters,
  type Person,
  type PhotoList,
  type Scope,
  type Shoot,
} from "../api";
import { Viewer } from "./Viewer";

type Decision = "picked" | "rejected" | "undecided";

const decisionOf = (f: CullFrame): Decision => (f.picked ? "picked" : f.marked ? "rejected" : "undecided");

interface Props {
  scope: Scope;
  filters: Filters;
  title: string;
  /** Back to the photos (Done, Escape). */
  onClose: () => void;
  /** Picks or rejects changed (the sidebar's counts follow). */
  onChanged: () => void;
  onError: (text: string) => void;
  everyone: Person[];
}

/** A row of the shots list, px. */
const ROW = 76;

/**
 * Cull: a shoot, one shot at a time. Bursts come as stacks with the frame
 * Cull would keep first in line; each frame says what Cull suggests and why.
 * Picks are a flag, rejects are marks for deletion: nothing is deleted here.
 */
export const CullView = memo(function CullView({ scope, filters, title, onClose, onChanged, onError, everyone }: Props) {
  const [shoot, setShoot] = useState<Shoot | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  // Decisions made here, shown at once (the next answer has them too).
  const [mine, setMine] = useState<Map<number, Decision>>(() => new Map());
  const [at, setAt] = useState<{ stack: number; frame: number }>({ stack: 0, frame: 0 });
  // The frame looked at, by id: kept when the shoot is reloaded.
  const atId = useRef<number | null>(null);
  const [closer, setCloser] = useState<CloserProgress | null>(null);
  const [viewing, setViewing] = useState(false);

  // Only the latest answer counts.
  const seq = useRef(0);
  const load = useCallback(() => {
    const n = ++seq.current;
    cull
      .shoot(scope, filters)
      .then((s) => {
        if (n !== seq.current) return;
        setShoot(s);
        setFailed(null);
        setMine(new Map());
      })
      .catch((e) => n === seq.current && setFailed(String(e)));
  }, [scope, filters]);
  useEffect(load, [load]);

  // Measures come in (a closer look, photos measured in the background): reloaded, at most every 1.5 s.
  const timer = useRef<number | undefined>(undefined);
  const loadSoon = useCallback(() => {
    if (timer.current) return;
    timer.current = window.setTimeout(() => {
      timer.current = undefined;
      load();
    }, 1500);
  }, [load]);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  useEffect(() => {
    cull
      .progress()
      .then((p) => setCloser(p))
      .catch(() => {});
    const subs = [
      listen<CloserProgress>("cull-progress", (e) => setCloser(e.payload.finished ? null : e.payload)),
      listen("cull-changed", loadSoon),
      listen<AiProgress>("ai-progress", (e) => {
        if (e.payload.finished || e.payload.task === "quality") loadSoon();
      }),
    ];
    return () => subs.forEach((p) => p.then((un) => un()));
  }, [loadSoon]);

  const stacks = useMemo(() => shoot?.stacks ?? [], [shoot]);
  const decision = useCallback((f: CullFrame) => mine.get(f.id) ?? decisionOf(f), [mine]);

  // Back on the frame looked at after a reload; the first shot to start with.
  useEffect(() => {
    if (!stacks.length) return;
    const id = atId.current;
    for (let s = 0; id != null && s < stacks.length; s++) {
      const f = stacks[s].frames.findIndex((x) => x.id === id);
      if (f >= 0) return setAt({ stack: s, frame: f });
    }
    setAt({ stack: 0, frame: stacks[0].keeper });
  }, [stacks]);

  const stack: CullStack | undefined = stacks[Math.min(at.stack, stacks.length - 1)];
  const frame = stack?.frames[Math.min(at.frame, stack.frames.length - 1)];

  /** Goes to a frame (and stays on it when the shoot is reloaded). */
  const go = useCallback(
    (s: number, f: number) => {
      atId.current = stacks[s]?.frames[f]?.id ?? null;
      setAt({ stack: s, frame: f });
    },
    [stacks],
  );

  const counts = useMemo(() => {
    const c = { picked: 0, rejected: 0, undecided: 0, suggested: 0 };
    for (const s of stacks)
      for (const f of s.frames) {
        const d = decision(f);
        c[d]++;
        if (d === "undecided" && f.suggest !== "none") c.suggested++;
      }
    return c;
  }, [stacks, decision]);

  const decide = useCallback(
    (ids: number[], d: Decision) => {
      if (!ids.length) return;
      setMine((m) => {
        const next = new Map(m);
        for (const id of ids) next.set(id, d);
        return next;
      });
      const asked = d === "picked" ? { picks: ids } : d === "rejected" ? { rejects: ids } : { undecided: ids };
      cull
        .decide(asked)
        .then(onChanged)
        .catch((e) => {
          onError(String(e));
          load();
        });
    },
    [onChanged, onError, load],
  );

  /** What Cull suggests for these frames, where nothing is decided yet. */
  const accept = useCallback(
    (frames: CullFrame[]) => {
      const open = frames.filter((f) => decision(f) === "undecided");
      const picks = open.filter((f) => f.suggest === "pick").map((f) => f.id);
      const rejects = open.filter((f) => f.suggest === "reject").map((f) => f.id);
      if (!picks.length && !rejects.length) return;
      setMine((m) => {
        const next = new Map(m);
        for (const id of picks) next.set(id, "picked");
        for (const id of rejects) next.set(id, "rejected");
        return next;
      });
      cull
        .decide({ picks, rejects })
        .then(onChanged)
        .catch((e) => {
          onError(String(e));
          load();
        });
    },
    [decision, onChanged, onError, load],
  );

  const goStack = useCallback(
    (s: number) => {
      if (!stacks.length) return;
      const i = Math.max(0, Math.min(stacks.length - 1, s));
      go(i, stacks[i].keeper);
    },
    [stacks],
  );
  /** The next frame, on to the next shot after the last. */
  const next = useCallback(() => {
    if (!stack) return;
    if (at.frame < stack.frames.length - 1) go(at.stack, at.frame + 1);
    else if (at.stack < stacks.length - 1) go(at.stack + 1, stacks[at.stack + 1].keeper);
  }, [stack, at, stacks]);

  const toggle = useCallback(
    (d: Decision) => {
      if (!frame) return;
      decide([frame.id], decision(frame) === d ? "undecided" : d);
      if (decision(frame) !== d) next();
    },
    [frame, decide, decision, next],
  );

  useEffect(() => {
    if (viewing) return;
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT")) return;
      if (e.ctrlKey || e.metaKey || e.altKey || !stack) {
        if (e.key === "Escape") onClose();
        return;
      }
      switch (e.key) {
        case "ArrowRight":
          next();
          break;
        case "ArrowLeft":
          if (at.frame > 0) go(at.stack, at.frame - 1);
          else if (at.stack > 0) go(at.stack - 1, stacks[at.stack - 1].frames.length - 1);
          break;
        case "ArrowDown":
          goStack(at.stack + 1);
          break;
        case "ArrowUp":
          goStack(at.stack - 1);
          break;
        case "Home":
          goStack(0);
          break;
        case "End":
          goStack(stacks.length - 1);
          break;
        case "p":
        case "P":
          toggle("picked");
          break;
        case "x":
        case "X":
        case "Delete":
          toggle("rejected");
          break;
        case "u":
        case "U":
          if (frame) decide([frame.id], "undecided");
          break;
        case "Enter":
          accept(stack.frames);
          goStack(at.stack + 1);
          break;
        case "z":
        case "Z":
          setViewing(true);
          break;
        case "Escape":
          onClose();
          break;
        default:
          return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [viewing, stack, frame, at, stacks, next, goStack, toggle, decide, accept, onClose]);

  // The viewer, for a close look at the shot's frames (Z there zooms to 100%).
  const stackList: PhotoList | null = useMemo(() => {
    if (!stack) return null;
    const fr = stack.frames;
    return {
      ids: fr.map((f) => f.id),
      taken: fr.map((f) => f.taken),
      state: fr.map((f) => f.state),
      w: fr.map((f) => f.w),
      h: fr.map((f) => f.h),
      rev: fr.map((f) => f.rev),
      preview: fr.map(() => 1),
      marked: fr.map((f) => (decision(f) === "rejected" ? 1 : 0)),
      ext: fr.map(() => ""),
      raw: fr.map(() => 0),
      fresh: fr.map(() => 0),
      picked: fr.map((f) => (decision(f) === "picked" ? 1 : 0)),
      flags: fr.map(() => 0),
    };
  }, [stack, decision]);

  if (failed) {
    return (
      <div className="cull">
        <div className="empty">
          <div className="empty-card">
            <h2>Cull</h2>
            <p>{failed}</p>
            <button className="btn" onClick={onClose}>
              Back to the photos
            </button>
          </div>
        </div>
      </div>
    );
  }
  if (!shoot) {
    return (
      <div className="cull">
        <div className="cull-loading muted">
          <LoaderCircle size={16} className="spinning" /> Gathering the shots of {title}…
        </div>
      </div>
    );
  }
  if (!stack || !frame) {
    return (
      <div className="cull">
        <div className="empty">
          <div className="empty-card">
            <h2>Nothing to cull here</h2>
            <button className="btn" onClick={onClose}>
              Back to the photos
            </button>
          </div>
        </div>
      </div>
    );
  }

  const d = decision(frame);
  const lookable = shoot.canLookCloser.length;
  return (
    <div className="cull">
      <div className="cull-head">
        <span>
          <b>{fmtNum(stacks.length)}</b> shots · {fmtNum(shoot.photos)} photos ·{" "}
          <span className="cull-count picked">{fmtNum(counts.picked)} picked</span> ·{" "}
          <span className="cull-count rejected">{fmtNum(counts.rejected)} rejected</span> · {fmtNum(counts.undecided)} to decide
        </span>
        {shoot.measured < shoot.photos && (
          <span className="muted">
            <LoaderCircle size={13} className="spinning" /> Measuring: {fmtNum(shoot.measured)} of {fmtNum(shoot.photos)}
          </span>
        )}
        {shoot.described < shoot.photos && (
          <span className="muted" title="Bursts are told apart by word search's descriptions; without them, only frames a second apart are grouped">
            {fmtNum(shoot.photos - shoot.described)} not described yet
          </span>
        )}
        <span className="grow" />
        {closer ? (
          <span className="cull-closer">
            <LoaderCircle size={13} className="spinning" /> Looking closer · {fmtNum(closer.done)} of {fmtNum(closer.total)}
            <button className="btn small ghost" onClick={() => cull.stop().catch(() => {})}>
              <X size={13} /> Stop
            </button>
          </span>
        ) : (
          lookable > 0 && (
            <button
              className="btn small ghost"
              onClick={() => cull.lookCloser(shoot.canLookCloser).catch((e) => onError(String(e)))}
              title="Read the originals on the plugged-in disks (at half size, read-only) and measure them again: shows the photos only soft at 100%"
            >
              <ScanSearch size={14} /> Look closer at {fmtNum(lookable)} originals
            </button>
          )
        )}
        <button
          className="btn small"
          disabled={!counts.suggested}
          onClick={() => accept(stacks.flatMap((s) => s.frames))}
          title="Pick and reject as Cull suggests, wherever nothing is decided yet. Rejected photos are only marked for deletion."
        >
          <CheckCheck size={14} /> Accept all suggestions{counts.suggested ? ` (${fmtNum(counts.suggested)})` : ""}
        </button>
        <button className="btn small ghost" onClick={onClose} title="Back to the photos (Esc)">
          Done
        </button>
      </div>

      <div className="cull-body">
        <ShotList stacks={stacks} current={at.stack} decision={decision} onGo={goStack} />
        <div className="cull-main">
          <div className={`cull-photo ${d}`} onDoubleClick={() => setViewing(true)}>
            <img key={frame.id} src={previewUrl(frame.id, frame.rev)} alt="" draggable={false} />
            {d !== "undecided" && (
              <span className={`cull-stamp ${d}`}>
                {d === "picked" ? <Check size={16} /> : <Trash2 size={16} />} {d === "picked" ? "Picked" : "Rejected"}
              </span>
            )}
          </div>
          <div className="cull-info">
            <div className="cull-says">
              <span className={`cull-suggest ${frame.suggest}`}>
                {frame.suggest === "pick" ? "Cull suggests: keep" : frame.suggest === "reject" ? "Cull suggests: reject" : "Your call"}
              </span>
              {frame.reasons.map((r, i) => (
                <span key={i} className={i === 0 ? "" : "muted"}>
                  {r}
                </span>
              ))}
            </div>
            <div className="muted small cull-meta">
              Frame {at.frame + 1} of {stack.frames.length}
              {frame.name && ` · ${frame.name}`}
              {frame.blur != null &&
                ` · edges ${frame.blur.toFixed(2)}‰ of the width${frame.closer ? " (original)" : " (preview)"}`}
            </div>
            <div className="cull-actions">
              <button className={`btn small ${d === "picked" ? "on" : "ghost"}`} onClick={() => toggle("picked")} title="Pick (P)">
                <Check size={14} /> Pick
              </button>
              <button className={`btn small ${d === "rejected" ? "danger" : "ghost"}`} onClick={() => toggle("rejected")} title="Reject: mark for deletion (X or Delete)">
                <Trash2 size={14} /> Reject
              </button>
              <button className="btn small ghost" disabled={d === "undecided"} onClick={() => decide([frame.id], "undecided")} title="Neither (U)">
                <Undo2 size={14} /> Undecide
              </button>
              <span className="grow" />
              <button
                className="btn small ghost"
                onClick={() => {
                  accept(stack.frames);
                  goStack(at.stack + 1);
                }}
                title="Do what Cull suggests for this shot, then go to the next one (Enter)"
              >
                <CheckCheck size={14} /> Accept this shot
              </button>
            </div>
          </div>
          <div className="cull-frames">
            {stack.frames.map((f, i) => {
              const fd = decision(f);
              return (
                <button
                  key={f.id}
                  className={`cull-frame ${fd} ${i === at.frame ? "current" : ""}`}
                  onClick={() => go(at.stack, i)}
                  onDoubleClick={() => setViewing(true)}
                  title={f.reasons.join(" · ")}
                >
                  <img src={thumbUrl(f.id, f.rev)} alt="" loading="lazy" draggable={false} />
                  {i === stack.keeper && stack.frames.length > 1 && (
                    <span className="cull-keeper" title="The frame Cull would keep">
                      <Star size={12} />
                    </span>
                  )}
                  <span className={`cull-mark ${fd !== "undecided" ? fd : f.suggest}`}>
                    {fd === "picked" ? (
                      <Check size={12} />
                    ) : fd === "rejected" ? (
                      <Trash2 size={12} />
                    ) : f.problems.length ? (
                      <Focus size={12} />
                    ) : (
                      <Square size={10} />
                    )}
                  </span>
                </button>
              );
            })}
          </div>
          <div className="cull-keys muted small">
            ← → frames · ↑ ↓ shots · P pick · X reject · U undecide · Enter accept the shot · Z or double-click: look closely · Esc done
          </div>
        </div>
      </div>

      {viewing && stackList && (
        <Viewer
          list={stackList}
          index={at.frame}
          onIndex={(i) => go(at.stack, i)}
          onToggleMark={() => {
            const f = stack.frames[at.frame];
            decide([f.id], decision(f) === "rejected" ? "undecided" : "rejected");
          }}
          onTagsChanged={() => {}}
          onPerson={() => {}}
          onFolder={(_: FileLoc) => {}}
          everyone={everyone}
          onFacesChanged={() => {}}
          onClose={() => setViewing(false)}
        />
      )}
    </div>
  );
});

/** The shots of the shoot, the current one in view; only those on screen are drawn. */
const ShotList = memo(function ShotList({
  stacks,
  current,
  decision,
  onGo,
}: {
  stacks: CullStack[];
  current: number;
  decision: (f: CullFrame) => Decision;
  onGo: (i: number) => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: 600 });
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const measure = () => setView({ top: el.scrollTop, height: el.clientHeight || 600 });
    measure();
    el.addEventListener("scroll", measure, { passive: true });
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(measure) : null;
    ro?.observe(el);
    return () => {
      el.removeEventListener("scroll", measure);
      ro?.disconnect();
    };
  }, []);
  // The current shot stays in view.
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const top = current * ROW;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW - el.clientHeight;
  }, [current]);

  const first = Math.max(0, Math.floor(view.top / ROW) - 4);
  const last = Math.min(stacks.length, Math.ceil((view.top + view.height) / ROW) + 4);
  const rows = [];
  for (let i = first; i < last; i++) {
    const s = stacks[i];
    const k = s.frames[s.keeper];
    const open = s.frames.filter((f) => decision(f) === "undecided").length;
    const picked = s.frames.some((f) => decision(f) === "picked");
    rows.push(
      <button
        key={i}
        className={`cull-shot ${i === current ? "current" : ""} ${open ? "" : "done"}`}
        style={{ top: i * ROW }}
        onClick={() => onGo(i)}
        title={s.frames.length > 1 ? `${s.frames.length} frames of the same shot` : "A single frame"}
      >
        <img src={thumbUrl(k.id, k.rev)} alt="" loading="lazy" draggable={false} />
        {s.frames.length > 1 && <span className="cull-shot-count">{s.frames.length}</span>}
        {!open && <span className={`cull-shot-done ${picked ? "picked" : "rejected"}`}>{picked ? <Check size={12} /> : <Trash2 size={12} />}</span>}
      </button>,
    );
  }
  return (
    <div className="cull-shots" ref={box}>
      <div style={{ height: stacks.length * ROW, position: "relative" }}>{rows}</div>
    </div>
  );
});
