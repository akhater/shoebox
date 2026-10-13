import { useEffect, useLayoutEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { fmtMonth } from "../api";

/** Where a month starts in the grid (px from the top) and the time of its first photo. */
export interface RailMark {
  top: number;
  ts: number;
}

interface Props {
  /** The grid's month headers, top to bottom. */
  marks: RailMark[];
  /** Height of the whole grid, and of the part on screen. */
  content: number;
  viewport: number;
  scrollTop: number;
  onScrollTo: (top: number) => void;
  onWheel: (dy: number) => void;
}

/** Room kept above and below the marks. */
const PAD = 12;
/** Year labels closer than this give way to a dot. */
const LABEL_GAP = 18;

const yearOf = (ts: number) => (ts > 0 ? new Date(ts * 1000).getUTCFullYear() : null);

/**
 * The date rail on the grid's right edge (it replaces the scrollbar): years
 * placed where their photos are, so a busy year takes more room. Drag along it
 * to fly through the years; a bubble names the month you'll land on.
 */
export function TimelineRail(p: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [h, setH] = useState(0);
  // The rail position under the mouse, and whether it's held down.
  const [pointer, setPointer] = useState<number | null>(null);
  const [dragging, setDragging] = useState(false);
  // The bubble also shows for a moment after any scroll.
  const [scrolled, setScrolled] = useState(false);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setH(Math.max(0, el.clientHeight - PAD * 2));
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const firstScroll = useRef(true);
  useEffect(() => {
    if (firstScroll.current) {
      firstScroll.current = false;
      return;
    }
    setScrolled(true);
    const t = window.setTimeout(() => setScrolled(false), 900);
    return () => window.clearTimeout(t);
  }, [p.scrollTop]);

  const span = Math.max(1, p.content);
  const toY = (top: number) => PAD + (top / span) * h;
  const toTop = (y: number) => ((y - PAD) / Math.max(1, h)) * span;

  /** The month at grid position `top`: the last one starting at or above it. */
  const markAt = (top: number) => {
    let lo = 0;
    let hi = p.marks.length - 1;
    let ans = 0;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (p.marks[mid].top <= top + 1) {
        ans = mid;
        lo = mid + 1;
      } else hi = mid - 1;
    }
    return p.marks[ans];
  };

  // One label per year where there's room, a dot where there isn't.
  const years = useMemo(() => {
    const out: { y: number; year: number | null; label: boolean }[] = [];
    let lastLabel = -Infinity;
    let prev: number | null | undefined;
    for (const m of p.marks) {
      const year = yearOf(m.ts);
      if (year === prev) continue;
      prev = year;
      const y = PAD + (m.top / span) * h;
      const label = y - lastLabel >= LABEL_GAP;
      if (label) lastLabel = y;
      out.push({ y, year, label });
    }
    return out;
  }, [p.marks, h, span]);

  if (!p.marks.length || h <= 0) return <div className="rail" ref={ref} />;

  const here = markAt(p.scrollTop);
  const nowYear = yearOf(here.ts);
  const localY = (e: ReactPointerEvent) => e.clientY - ref.current!.getBoundingClientRect().top;
  const go = (y: number) => p.onScrollTo(Math.max(0, Math.min(p.content - p.viewport, toTop(y))));
  const bubble =
    pointer != null
      ? { y: Math.max(PAD, Math.min(PAD + h, pointer)), text: fmtMonth(markAt(Math.max(0, toTop(pointer))).ts) }
      : scrolled
        ? { y: toY(p.scrollTop), text: fmtMonth(here.ts) }
        : null;

  return (
    <div
      ref={ref}
      className={`rail ${dragging ? "dragging" : ""}`}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        setDragging(true);
        go(localY(e));
      }}
      onPointerMove={(e) => {
        setPointer(localY(e));
        if (dragging) go(localY(e));
      }}
      onPointerUp={(e) => {
        e.currentTarget.releasePointerCapture(e.pointerId);
        setDragging(false);
        // Let go away from the rail: no bubble left behind.
        const r = e.currentTarget.getBoundingClientRect();
        if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) setPointer(null);
      }}
      onPointerLeave={() => !dragging && setPointer(null)}
      onLostPointerCapture={() => setDragging(false)}
      onWheel={(e) => p.onWheel(e.deltaY)}
    >
      <span className="rail-view" style={{ top: toY(p.scrollTop), height: Math.max(3, (p.viewport / span) * h) }} />
      {years.map((y, i) =>
        y.label ? (
          <span key={i} className={`rail-year ${y.year === nowYear ? "now" : ""}`} style={{ top: y.y }}>
            {y.year ?? "?"}
          </span>
        ) : (
          <span key={i} className="rail-dot" style={{ top: y.y }} />
        ),
      )}
      {pointer != null && <span className="rail-pointer" style={{ top: bubble!.y }} />}
      {bubble && (
        <span className="rail-bubble" style={{ top: bubble.y }}>
          {bubble.text}
        </span>
      )}
    </div>
  );
}
