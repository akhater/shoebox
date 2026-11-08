import { LoaderCircle } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode, type RefObject } from "react";
import { fullUrl, previewUrl, type CullFrame, type Focus } from "../api";
import { FocusPoints } from "./FocusPoints";

/** Where the photo is looked at, at 100%: the point in the middle of the screen, 0..1 of the photo. */
export type Spot = { x: number; y: number };

/** Where Z looks at 100% first: where the camera focused, else on the main face, else the middle. */
export const firstSpot = (f: CullFrame): Spot => (f.focus ? { x: f.focus[0], y: f.focus[1] } : { x: 0.5, y: 0.5 });

/** The spot of the picture in the box clicked (0..1 of it); null when the click missed it. */
export function clickedSpot(e: ReactMouseEvent<HTMLElement>): Spot | null {
  const r = e.currentTarget.querySelector("img")?.getBoundingClientRect();
  if (!r || !r.width || !r.height) return null;
  const x = (e.clientX - r.left) / r.width;
  const y = (e.clientY - r.top) / r.height;
  return x < 0 || x > 1 || y < 0 || y > 1 ? null : { x, y };
}

/** The camera's points, drawn on a frame whose focus missed them: where "the camera focused here" is. */
export const missedPoints = (f: CullFrame): Focus | null =>
  f.problems.includes("focusMissed") && f.focusPoints.length ? { mode: null, points: f.focusPoints } : null;

/**
 * The frame at 100%, from its original (disk plugged in), `spot` in the
 * middle; drag to move, a click without dragging goes back.
 */
export function Zoomed({
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
export function OnPicture({ picture, children }: { picture: RefObject<HTMLImageElement | null>; children: ReactNode }) {
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
