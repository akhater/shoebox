import { listen } from "@tauri-apps/api/event";
import { Check, CheckCheck, Layers, LoaderCircle, ScanSearch, Star, Trash2, TriangleAlert, X } from "lucide-react";
import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
} from "react";
import {
  cull,
  fmtNum,
  fullUrl,
  previewUrl,
  thumbUrl,
  type AiProgress,
  type CloserProgress,
  type CullFrame,
  type CullStack,
  type Filters,
  type Scope,
  type Shoot,
} from "../api";
import { compareSet, CullCompare } from "./CullCompare";
import { clickedSpot, firstSpot, missedPoints, OnPicture, Zoomed, type Spot } from "./CullPicture";
import { FocusPoints } from "./FocusPoints";

/** Where Z looks at 100% first, in words. */
const firstLook = (f: CullFrame) => (f.focusPoints.length ? "where the camera focused" : f.focus ? "on the face" : "the middle");

/** Where a shot's frames were compared, in words: what a frame's `blur` measures. */
const comparedAt: Record<CullStack["compared"], string> = {
  face: "edges at the face",
  focusPoint: "edges at the focus point",
  sharpest: "sharpest edges",
};


interface Props {
  scope: Scope;
  filters: Filters;
  title: string;
  /** Back to the photos (Done, Escape). */
  onClose: () => void;
  /** Marks changed (the sidebar's count follows). */
  onChanged: () => void;
  onError: (text: string) => void;
}

/** A row of the shots list, px. */
const ROW = 76;

/** How many frames compare shows, remembered (2 to 4). */
const COMPARE_COUNT = "shoebox.cullCompare";
const comparedCount = () => {
  try {
    const n = Number(localStorage.getItem(COMPARE_COUNT));
    return n >= 2 && n <= 4 ? n : 3;
  } catch {
    return 3;
  }
};

/** A click that adds to a selection, or extends it. */
const howClicked = (e: { ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }) =>
  e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : "plain";

/** `set` with `item` in or out. */
function toggled<T>(set: Set<T>, item: T) {
  const next = new Set(set);
  if (next.has(item)) next.delete(item);
  else next.add(item);
  return next;
}

const range = (a: number, b: number) => Array.from({ length: Math.abs(b - a) + 1 }, (_, i) => Math.min(a, b) + i);

/**
 * Cull: a shoot, one shot at a time, to send what isn't worth keeping to the
 * trash. Bursts come as stacks with the frame Cull would keep first in line;
 * each frame says what Cull suggests and why. A reject is a mark for
 * deletion: nothing is deleted here. Z or a click shows the frame at 100%
 * from its original (Z where the camera focused, else on the main face), and
 * the next frame at the same spot, to compare sharpness. C compares frames
 * side by side, with their faces (CullCompare).
 */
export const CullView = memo(function CullView({ scope, filters, title, onClose, onChanged, onError }: Props) {
  const [shoot, setShoot] = useState<Shoot | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  // Marks made here, shown at once (the next answer has them too).
  const [mine, setMine] = useState<Map<number, boolean>>(() => new Map());
  const [at, setAt] = useState<{ stack: number; frame: number }>({ stack: 0, frame: 0 });
  // The frame looked at, by id: kept when the shoot is reloaded.
  const atId = useRef<number | null>(null);
  // The frame's picture, fitted (what is drawn on it follows it).
  const picture = useRef<HTMLImageElement>(null);
  const [closer, setCloser] = useState<CloserProgress | null>(null);
  // At 100%: where. Kept from frame to frame.
  const [zoom, setZoom] = useState<Spot | null>(null);
  // Selected frames (of the shot shown), and selected shots: X and P act on them all.
  const [selFrames, setSelFrames] = useState<Set<number>>(() => new Set());
  const [selShots, setSelShots] = useState<Set<number>>(() => new Set());
  // Compare: frames side by side, how many.
  const [compare, setCompare] = useState(false);
  const [count, setCount] = useState(comparedCount);
  const chooseCount = useCallback((n: number) => {
    setCount(n);
    try {
      localStorage.setItem(COMPARE_COUNT, String(n));
    } catch {
      /* not remembered */
    }
  }, []);

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

  // Measures come in (this shoot's, measured as it opens; a closer look), or
  // faces and descriptions are done: reloaded, at most every 1.5 s.
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
        if (e.payload.finished) loadSoon();
      }),
    ];
    return () => subs.forEach((p) => p.then((un) => un()));
  }, [loadSoon]);

  const stacks = useMemo(() => shoot?.stacks ?? [], [shoot]);
  const isMarked = useCallback((f: CullFrame) => mine.get(f.id) ?? f.marked, [mine]);

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

  // The frames compared: those selected (up to 4), else `count` from the one looked at on. They stay
  // while the one looked at moves among them, and slide when it goes past them.
  const compared = useRef<{ key: string; from: number }>({ key: "", from: 0 });
  const shown = useMemo(() => {
    if (!compare || !stack) return null;
    const selected = stack.frames.flatMap((f, i) => (selFrames.has(f.id) ? [i] : []));
    const order = selected.length > 1 ? selected : stack.frames.map((_, i) => i);
    const key = `${at.stack}|${selected.length > 1 ? selected.join(",") : ""}`;
    const last = compared.current;
    const set = compareSet(order, at.frame, last.key === key ? last.from : null, selected.length > 1 ? 4 : count);
    compared.current = { key, from: set.from };
    return { order, frames: set.frames };
  }, [compare, stack, at, selFrames, count]);

  /** Goes to a frame (and stays on it when the shoot is reloaded). Another shot drops the frames selected. */
  const go = useCallback(
    (s: number, f: number) => {
      atId.current = stacks[s]?.frames[f]?.id ?? null;
      setAt((cur) => {
        if (cur.stack !== s) setSelFrames(new Set());
        return { stack: s, frame: f };
      });
    },
    [stacks],
  );

  const counts = useMemo(() => {
    const c = { marked: 0, suggested: 0 };
    for (const s of stacks)
      for (const f of s.frames) {
        if (isMarked(f)) c.marked++;
        else if (f.suggest === "reject") c.suggested++;
      }
    return c;
  }, [stacks, isMarked]);

  /** Marks (or unmarks) for deletion, shown at once. */
  const mark = useCallback(
    (ids: number[], marked: boolean) => {
      if (!ids.length) return;
      setMine((m) => {
        const next = new Map(m);
        for (const id of ids) next.set(id, marked);
        return next;
      });
      cull
        .mark(ids, marked)
        .then(onChanged)
        .catch((e) => {
          onError(String(e));
          load();
        });
    },
    [onChanged, onError, load],
  );

  /** Marks what Cull suggests rejecting among these frames (those not marked yet). */
  const accept = useCallback(
    (frames: CullFrame[]) =>
      mark(
        frames.filter((f) => !isMarked(f) && f.suggest === "reject").map((f) => f.id),
        true,
      ),
    [isMarked, mark],
  );

  const goStack = useCallback(
    (s: number) => {
      if (!stacks.length) return;
      const i = Math.max(0, Math.min(stacks.length - 1, s));
      go(i, stacks[i].keeper);
    },
    [stacks, go],
  );
  /** The next frame, on to the next shot after the last. */
  const next = useCallback(() => {
    if (!stack) return;
    if (at.frame < stack.frames.length - 1) go(at.stack, at.frame + 1);
    else if (at.stack < stacks.length - 1) go(at.stack + 1, stacks[at.stack + 1].keeper);
  }, [stack, at, stacks, go]);
  const previous = useCallback(() => {
    if (at.frame > 0) go(at.stack, at.frame - 1);
    else if (at.stack > 0) go(at.stack - 1, stacks[at.stack - 1].frames.length - 1);
  }, [at, stacks, go]);
  /** The next or previous frame: among those selected while they are compared. */
  const step = useCallback(
    (by: 1 | -1) => {
      if (shown && selFrames.size > 1) {
        const to = shown.order[shown.order.indexOf(at.frame) + by];
        if (to != null) go(at.stack, to);
      } else if (by > 0) next();
      else previous();
    },
    [shown, selFrames, at, go, next, previous],
  );
  /** Looks at a frame of the shot shown, by id (the selection stays). */
  const lookAt = useCallback(
    (id: number) => {
      const i = stack?.frames.findIndex((f) => f.id === id) ?? -1;
      if (i >= 0) go(at.stack, i);
    },
    [stack, at.stack, go],
  );

  /** The frames X and P act on: those selected, else those of the shots selected, else the one shown
   *  (the one looked at, when comparing). */
  const targets = useCallback((): CullFrame[] => {
    if (!stack || !frame) return [];
    if (compare) return [frame];
    if (selFrames.size) return stack.frames.filter((f) => selFrames.has(f.id));
    if (selShots.size) return [...selShots].sort((a, b) => a - b).flatMap((s) => stacks[s]?.frames ?? []);
    return [frame];
  }, [stack, frame, compare, selFrames, selShots, stacks]);
  const several = selFrames.size > 0 || selShots.size > 0;
  const clearSelection = useCallback(() => {
    setSelFrames(new Set());
    setSelShots(new Set());
  }, []);

  /** X: to the trash (back out when all already are). P or K: not to the trash. One frame moves on. */
  const act = useCallback(
    (reject: boolean) => {
      const fr = targets();
      if (!fr.length) return;
      const marked = reject && !fr.every((f) => isMarked(f));
      mark(
        fr.map((f) => f.id),
        marked,
      );
      if (several && !compare) clearSelection();
      else if (marked || !reject) step(1);
    },
    [targets, isMarked, mark, several, compare, clearSelection, step],
  );

  const clickFrame = useCallback(
    (i: number, e: ReactMouseEvent) => {
      if (!stack) return;
      const how = howClicked(e);
      const id = stack.frames[i].id;
      if (how === "plain") {
        clearSelection();
      } else {
        setSelShots(new Set());
        setSelFrames((sel) => {
          const from = sel.size ? sel : new Set([stack.frames[at.frame].id]);
          return how === "toggle" ? toggled(from, id) : new Set(range(at.frame, i).map((k) => stack.frames[k].id));
        });
      }
      go(at.stack, i);
    },
    [stack, at, go, clearSelection],
  );

  const clickShot = useCallback(
    (i: number, e: ReactMouseEvent) => {
      const how = howClicked(e);
      if (how === "plain") {
        clearSelection();
      } else {
        setSelFrames(new Set());
        setSelShots((sel) => {
          const from = sel.size ? sel : new Set([at.stack]);
          return how === "toggle" ? toggled(from, i) : new Set(range(at.stack, i));
        });
      }
      goStack(i);
    },
    [at, goStack, clearSelection],
  );

  // 100%: Z toggles it, where the camera focused (else on the main face, or the middle); a click on the photo goes there.
  const zoomOn = useCallback(() => frame && setZoom(firstSpot(frame)), [frame]);
  const clickPhoto = (e: ReactMouseEvent<HTMLDivElement>) => {
    const spot = clickedSpot(e);
    if (spot) setZoom(spot);
    else zoomOn();
  };

  // At 100%, the frames before and after are read ahead (kept a while by the browser).
  const zoomed = zoom != null;
  useEffect(() => {
    if (!zoomed || !stack) return;
    for (const f of [stack.frames[at.frame + 1], stack.frames[at.frame - 1]]) {
      if (f && f.state === 2) new Image().src = fullUrl(f.id, 0, f.rev);
    }
  }, [zoomed, stack, at.frame]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT")) return;
      if (e.key === "Escape") {
        // A selection first, then 100%, then compare, then Cull itself.
        if (several) clearSelection();
        else if (zoom) setZoom(null);
        else if (compare) setCompare(false);
        else onClose();
        e.preventDefault();
        return;
      }
      if (!stack) return;
      if ((e.ctrlKey || e.metaKey) && (e.key === "a" || e.key === "A")) {
        setSelShots(new Set());
        setSelFrames(new Set(stack.frames.map((f) => f.id)));
        e.preventDefault();
        return;
      }
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      switch (e.key) {
        case "ArrowRight":
          step(1);
          break;
        case "ArrowLeft":
          step(-1);
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
        case "x":
        case "X":
        case "Delete":
          act(true);
          break;
        case "p":
        case "P":
        case "k":
        case "K":
          act(false);
          break;
        case "Enter":
          accept(stack.frames);
          goStack(at.stack + 1);
          break;
        case "z":
        case "Z":
          if (zoom) setZoom(null);
          else zoomOn();
          break;
        case "c":
        case "C":
          setCompare((c) => !c);
          break;
        case "2":
        case "3":
        case "4":
          if (!compare) return;
          chooseCount(Number(e.key));
          break;
        default:
          return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [stack, at, stacks, zoom, several, compare, step, goStack, act, accept, zoomOn, clearSelection, chooseCount, onClose]);

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

  const marked = isMarked(frame);
  const missed = missedPoints(frame);
  const lookable = shoot.canLookCloser.length;
  const chosen = targets();
  const n = several && !compare ? ` ${fmtNum(chosen.length)}` : "";
  return (
    <div className="cull">
      <div className="cull-head">
        <span>
          <b>{fmtNum(stacks.length)}</b> shots · {fmtNum(shoot.photos)} photos ·{" "}
          <span className="cull-count rejected">{fmtNum(counts.marked)} rejected</span>
        </span>
        {several &&
          (compare ? (
            <span className="cull-selected">Comparing the frames selected · Esc to clear</span>
          ) : (
            <span className="cull-selected">{fmtNum(chosen.length)} selected · X or P for all · Esc to clear</span>
          ))}
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
        {shoot.unreadable.length > 0 && (
          <span
            className="cull-unreadable"
            title={shoot.unreadable.map((u) => `${u.name ?? `Photo ${u.id}`}: ${u.reason}`).join("\n")}
          >
            <TriangleAlert size={13} /> {fmtNum(shoot.unreadable.length)} original{shoot.unreadable.length === 1 ? "" : "s"} couldn't be
            read
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
          title="Reject what Cull suggests rejecting. Rejected photos are only marked for deletion."
        >
          <CheckCheck size={14} /> Accept all suggestions{counts.suggested ? ` (${fmtNum(counts.suggested)})` : ""}
        </button>
        <button className="btn small ghost" onClick={onClose} title="Back to the photos (Esc)">
          Done
        </button>
      </div>

      <div className="cull-body">
        <ShotList stacks={stacks} current={at.stack} selected={selShots} isMarked={isMarked} onClick={clickShot} />
        <div className="cull-main">
          {shown ? (
            <CullCompare
              stack={stack}
              shown={shown.frames}
              current={at.frame}
              isMarked={isMarked}
              zoom={zoom}
              onLook={lookAt}
              onZoom={setZoom}
            />
          ) : zoom ? (
            <Zoomed frame={frame} points={missed} spot={zoom} onMove={setZoom} onExit={() => setZoom(null)} />
          ) : (
            <div
              className={`cull-photo ${marked ? "rejected" : ""}`}
              onClick={clickPhoto}
              title={`Click: 100% here (Z: ${firstLook(frame)})`}
            >
              <img key={frame.id} ref={picture} src={previewUrl(frame.id, frame.rev)} alt="" draggable={false} />
              {missed && (
                <OnPicture key={frame.id} picture={picture}>
                  <FocusPoints focus={missed} />
                </OnPicture>
              )}
              {marked && (
                <span className="cull-stamp rejected">
                  <Trash2 size={16} /> Rejected
                </span>
              )}
            </div>
          )}
          <div className="cull-info">
            <div className="cull-says">
              <span className={`cull-suggest ${frame.suggest}`}>
                {frame.suggest === "keep" ? "Cull suggests: keep" : frame.suggest === "reject" ? "Cull suggests: reject" : "Your call"}
              </span>
              {frame.reasons.map((r, i) => (
                <span key={i} className={i === 0 ? "" : "muted"}>
                  {r}
                </span>
              ))}
            </div>
            <div className="muted small cull-meta">
              Frame {at.frame + 1} of {stack.frames.length}
              {stack.kind === "focus" && " of a focus stack"}
              {frame.name && ` · ${frame.name}`}
              {frame.blur != null &&
                ` · ${comparedAt[stack.compared]} ${frame.blur.toFixed(2)}‰ of the width${stack.closer ? " (original)" : " (preview)"}`}
            </div>
            <div className="cull-actions">
              <button
                className={`btn small ${!n && marked ? "danger" : "ghost"}`}
                onClick={() => act(true)}
                title="Reject: mark for deletion (X or Delete)"
              >
                <Trash2 size={14} /> Reject{n}
              </button>
              <button className="btn small ghost" onClick={() => act(false)} title="Keep: not for the trash (P or K)">
                <Check size={14} /> Keep{n}
              </button>
              <span className="grow" />
              <button
                className="btn small ghost"
                onClick={() => {
                  accept(stack.frames);
                  goStack(at.stack + 1);
                }}
                title="Reject what Cull suggests rejecting in this shot, then go to the next one (Enter)"
              >
                <CheckCheck size={14} /> Accept this shot
              </button>
            </div>
          </div>
          <div className="cull-frames">
            {stack.frames.map((f, i) => {
              const fm = isMarked(f);
              return (
                <button
                  key={f.id}
                  className={`cull-frame ${fm ? "rejected" : ""} ${i === at.frame ? "current" : ""} ${selFrames.has(f.id) ? "selected" : ""} ${shown?.frames.includes(i) ? "compared" : ""}`}
                  onClick={(e) => clickFrame(i, e)}
                  title={f.reasons.join(" · ")}
                >
                  <img src={thumbUrl(f.id, f.rev)} alt="" loading="lazy" draggable={false} />
                  {i === stack.keeper && stack.frames.length > 1 && stack.kind !== "focus" && (
                    <span className="cull-keeper" title="The frame Cull would keep">
                      <Star size={12} />
                    </span>
                  )}
                  {(fm || f.suggest === "reject") && (
                    <span className={`cull-mark ${fm ? "rejected" : "suggested"}`}>
                      <Trash2 size={12} />
                    </span>
                  )}
                </button>
              );
            })}
          </div>
          <div className="cull-keys muted small">
            {compare
              ? "← → frames · ↑ ↓ shots · X reject · P or K keep the frame looked at · 2 3 4 how many · Z or click it: all at 100% (drag moves all) · click a face: its frame · C or Esc: one frame"
              : "← → frames · ↑ ↓ shots · X reject · P or K keep · Enter accept the shot · Z or click: 100% (drag to move, ← → keep the spot) · C compare · Ctrl or Shift click: several · Esc back"}
          </div>
        </div>
      </div>
    </div>
  );
});

/** The shots of the shoot, the current one in view; only those on screen are drawn. */
const ShotList = memo(function ShotList({
  stacks,
  current,
  selected,
  isMarked,
  onClick,
}: {
  stacks: CullStack[];
  current: number;
  selected: Set<number>;
  isMarked: (f: CullFrame) => boolean;
  onClick: (i: number, e: ReactMouseEvent) => void;
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
    const rejected = s.frames.filter(isMarked).length;
    rows.push(
      <button
        key={i}
        className={`cull-shot ${i === current ? "current" : ""} ${selected.has(i) ? "selected" : ""}`}
        style={{ top: i * ROW }}
        onClick={(e) => onClick(i, e)}
        title={`${
          s.kind === "focus"
            ? `A focus stack of ${s.frames.length} frames, to be merged: all kept`
            : s.frames.length > 1
              ? `${s.frames.length} frames of the same shot`
              : "A single frame"
        } · Ctrl or Shift click: select several shots`}
      >
        <img src={thumbUrl(k.id, k.rev)} alt="" loading="lazy" draggable={false} />
        {(s.frames.length > 1 || s.kind === "focus") && (
          <span className="cull-shot-count">
            {s.kind === "focus" && <Layers size={10} />}
            {s.frames.length}
          </span>
        )}
        {rejected > 0 && (
          <span className="cull-shot-done rejected">
            <Trash2 size={12} />
          </span>
        )}
      </button>,
    );
  }
  return (
    <div className="cull-shots" ref={box}>
      <div style={{ height: stacks.length * ROW, position: "relative" }}>{rows}</div>
    </div>
  );
});
