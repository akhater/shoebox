import { listen } from "@tauri-apps/api/event";
import { Check, CheckCheck, LoaderCircle, ScanSearch, Star, Trash2, TriangleAlert, X } from "lucide-react";
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type RefObject,
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
  type Focus,
  type Scope,
  type Shoot,
} from "../api";
import { FocusPoints } from "./FocusPoints";

/** Where the photo is looked at, at 100%: the point in the middle of the screen, 0..1 of the photo. */
type Spot = { x: number; y: number };

/** Where Z looks at 100% first, in words. */
const firstLook = (f: CullFrame) => (f.focusPoints.length ? "where the camera focused" : f.focus ? "on the face" : "the middle");

/** The camera's points, drawn on a frame whose focus missed them: where "the camera focused here" is. */
const missedPoints = (f: CullFrame): Focus | null =>
  f.problems.includes("focusMissed") && f.focusPoints.length ? { mode: null, points: f.focusPoints } : null;

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
 * the next frame at the same spot, to compare sharpness.
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

  // Measures come in (a closer look; the background pass while some of these
  // photos aren't measured yet): reloaded, at most every 1.5 s.
  const waiting = useRef(false);
  waiting.current = !!shoot && shoot.measured < shoot.photos;
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
        if (e.payload.finished || (e.payload.task === "quality" && waiting.current)) loadSoon();
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

  /** The frames X and P act on: those selected, else those of the shots selected, else the one shown. */
  const targets = useCallback((): CullFrame[] => {
    if (!stack || !frame) return [];
    if (selFrames.size) return stack.frames.filter((f) => selFrames.has(f.id));
    if (selShots.size) return [...selShots].sort((a, b) => a - b).flatMap((s) => stacks[s]?.frames ?? []);
    return [frame];
  }, [stack, frame, selFrames, selShots, stacks]);
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
      if (several) clearSelection();
      else if (marked || !reject) next();
    },
    [targets, isMarked, mark, several, clearSelection, next],
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
  const zoomOn = useCallback(() => setZoom(frame?.focus ? { x: frame.focus[0], y: frame.focus[1] } : { x: 0.5, y: 0.5 }), [frame]);
  const clickPhoto = (e: ReactMouseEvent<HTMLDivElement>) => {
    const img = e.currentTarget.querySelector("img");
    const r = img?.getBoundingClientRect();
    if (!r || !r.width || !r.height) return zoomOn();
    const x = (e.clientX - r.left) / r.width;
    const y = (e.clientY - r.top) / r.height;
    if (x < 0 || x > 1 || y < 0 || y > 1) return zoomOn();
    setZoom({ x, y });
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
        // A selection first, then 100%, then Cull itself.
        if (several) clearSelection();
        else if (zoom) setZoom(null);
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
          next();
          break;
        case "ArrowLeft":
          previous();
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
        default:
          return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [stack, at, stacks, zoom, several, next, previous, goStack, act, accept, zoomOn, clearSelection, onClose]);

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
  const n = several ? ` ${fmtNum(chosen.length)}` : "";
  return (
    <div className="cull">
      <div className="cull-head">
        <span>
          <b>{fmtNum(stacks.length)}</b> shots · {fmtNum(shoot.photos)} photos ·{" "}
          <span className="cull-count rejected">{fmtNum(counts.marked)} rejected</span>
        </span>
        {several && <span className="cull-selected">{fmtNum(chosen.length)} selected · X or P for all · Esc to clear</span>}
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
          {zoom ? (
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
              {frame.name && ` · ${frame.name}`}
              {frame.blur != null &&
                ` · edges ${frame.blur.toFixed(2)}‰ of the width${frame.closer ? " (original)" : " (preview)"}`}
            </div>
            <div className="cull-actions">
              <button
                className={`btn small ${!several && marked ? "danger" : "ghost"}`}
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
                  className={`cull-frame ${fm ? "rejected" : ""} ${i === at.frame ? "current" : ""} ${selFrames.has(f.id) ? "selected" : ""}`}
                  onClick={(e) => clickFrame(i, e)}
                  title={f.reasons.join(" · ")}
                >
                  <img src={thumbUrl(f.id, f.rev)} alt="" loading="lazy" draggable={false} />
                  {i === stack.keeper && stack.frames.length > 1 && (
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
            ← → frames · ↑ ↓ shots · X reject · P or K keep · Enter accept the shot · Z or click: 100% (drag to move, ← → keep
            the spot) · Ctrl or Shift click: several · Esc back
          </div>
        </div>
      </div>
    </div>
  );
});

/**
 * The frame at 100%, from its original (disk plugged in), `spot` in the
 * middle; drag to move, a click without dragging goes back.
 */
function Zoomed({
  frame,
  points,
  spot,
  onMove,
  onExit,
}: {
  frame: CullFrame;
  /** Drawn over the photo. */
  points: Focus | null;
  spot: Spot;
  onMove: (s: Spot) => void;
  onExit: () => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [boxSize, setBoxSize] = useState({ w: 0, h: 0 });
  // The original's size once its picture is here (for this frame).
  const [size, setSize] = useState<{ id: number; w: number; h: number } | null>(null);
  const drag = useRef<{ x: number; y: number; spot: Spot; moved: boolean } | null>(null);
  const reachable = frame.state === 2;

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const measure = () => setBoxSize({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(measure) : null;
    ro?.observe(el);
    return () => ro?.disconnect();
  }, []);

  const shown = size?.id === frame.id ? size : null;
  // The original at 100%, the spot looked at in the middle.
  const placed = shown ? { left: boxSize.w / 2 - spot.x * shown.w, top: boxSize.h / 2 - spot.y * shown.h } : undefined;
  const down = (e: ReactMouseEvent) => {
    drag.current = { x: e.clientX, y: e.clientY, spot, moved: false };
    const move = (m: MouseEvent) => {
      const g = drag.current;
      if (!g) return;
      const dx = m.clientX - g.x;
      const dy = m.clientY - g.y;
      if (Math.abs(dx) + Math.abs(dy) > 3) g.moved = true;
      if (g.moved && shown) {
        onMove({
          x: Math.min(1, Math.max(0, g.spot.x - dx / shown.w)),
          y: Math.min(1, Math.max(0, g.spot.y - dy / shown.h)),
        });
      }
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      if (drag.current && !drag.current.moved) onExit();
      drag.current = null;
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  return (
    <div className={`cull-zoom ${shown ? "ready" : ""}`} ref={box} onMouseDown={reachable ? down : onExit}>
      {!shown && <img className="cull-zoom-wait" src={previewUrl(frame.id, frame.rev)} alt="" draggable={false} />}
      {reachable && (
        <img
          key={frame.id}
          className="cull-zoom-full"
          src={fullUrl(frame.id, 0, frame.rev)}
          alt=""
          draggable={false}
          onLoad={(e) => setSize({ id: frame.id, w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
          style={placed}
        />
      )}
      {placed && points && (
        <div className="cull-on-picture" style={{ ...placed, width: shown?.w, height: shown?.h }}>
          <FocusPoints focus={points} />
        </div>
      )}
      {!shown && (
        <span className="cull-zoom-note">
          {reachable ? (
            <>
              <LoaderCircle size={13} className="spinning" /> Reading the original at 100%…
            </>
          ) : (
            "The disk with this photo is unplugged: 100% needs the original. Click to go back."
          )}
        </span>
      )}
    </div>
  );
}

/** A box over `picture` where `.cull-photo` fits it, for what is drawn on the photo;
 *  hidden until the picture is in. */
function OnPicture({ picture, children }: { picture: RefObject<HTMLImageElement | null>; children: ReactNode }) {
  const [box, setBox] = useState<{ left: number; top: number; width: number; height: number } | null>(null);
  useLayoutEffect(() => {
    const el = picture.current;
    if (!el) return;
    const measure = () => setBox({ left: el.offsetLeft, top: el.offsetTop, width: el.offsetWidth, height: el.offsetHeight });
    measure();
    el.addEventListener("load", measure);
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(measure) : null;
    ro?.observe(el);
    if (el.parentElement) ro?.observe(el.parentElement);
    return () => {
      el.removeEventListener("load", measure);
      ro?.disconnect();
    };
  }, [picture]);
  return (
    <div className="cull-on-picture" style={box?.width ? box : { visibility: "hidden" }}>
      {children}
    </div>
  );
}

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
        title={`${s.frames.length > 1 ? `${s.frames.length} frames of the same shot` : "A single frame"} · Ctrl or Shift click: select several shots`}
      >
        <img src={thumbUrl(k.id, k.rev)} alt="" loading="lazy" draggable={false} />
        {s.frames.length > 1 && <span className="cull-shot-count">{s.frames.length}</span>}
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
