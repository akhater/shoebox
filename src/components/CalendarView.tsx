import { useLayoutEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { fmtDateKey, fmtNum, thumbUrl, type PhotoList } from "../api";

interface Props {
  /** What the grid shows (scope, filters and search applied). */
  list: PhotoList;
  /** Years newest first (the grid's "newest first" order). */
  newestFirst: boolean;
  /** The photo the grid had at the top: its year opens in view, its day is outlined. */
  startIndex: number;
  /** A day, month or year was picked: its first photo in the grid's order. */
  onPick: (index: number) => void;
}

const pad2 = (n: number) => String(n).padStart(2, "0");
const DAY_MS = 86_400_000;
/** Thumbnails in the hover strip, and their height. */
const STRIP = 6;
const STRIP_H = 76;
const MONTHS = Array.from({ length: 12 }, (_, m) =>
  new Intl.DateTimeFormat(undefined, { month: "short", timeZone: "UTC" }).format(new Date(Date.UTC(2000, m, 1))),
);
const WEEKDAYS = ["Mon", "", "Wed", "", "Fri", "", ""];

/** "2006-01-14" for a camera time (seconds since 1970 read as UTC). */
const dayKey = (ts: number) => {
  const d = new Date(ts * 1000);
  return `${d.getUTCFullYear()}-${pad2(d.getUTCMonth() + 1)}-${pad2(d.getUTCDate())}`;
};

interface Year {
  year: number;
  photos: number;
  /** First photo of the year, and of each month (-1 = none), in the grid's order. */
  first: number;
  months: number[];
}

/**
 * Every year as a grid of days, shaded by how many photos were taken that day.
 * Hovering a day shows a few of its photos; clicking it goes there in the grid.
 */
export function CalendarView({ list, newestFirst, startIndex, onPick }: Props) {
  const scroller = useRef<HTMLDivElement>(null);
  const body = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<{ key: string; x: number; y: number; above: boolean } | null>(null);
  // Kept out of the years below, so hovering doesn't redraw thousands of days.
  const pick = useRef(onPick);
  pick.current = onPick;

  const cal = useMemo(() => {
    // Each day's photos (indices in the grid's order), and each year's.
    const days = new Map<string, number[]>();
    const years = new Map<number, Year>();
    let undated = 0;
    let firstUndated = -1;
    for (let i = 0; i < list.ids.length; i++) {
      const ts = list.taken[i];
      if (ts <= 0) {
        undated++;
        if (firstUndated < 0) firstUndated = i;
        continue;
      }
      const key = dayKey(ts);
      let at = days.get(key);
      if (!at) days.set(key, (at = []));
      at.push(i);
      const year = +key.slice(0, 4);
      let y = years.get(year);
      if (!y) years.set(year, (y = { year, photos: 0, first: i, months: Array(12).fill(-1) }));
      y.photos++;
      const m = +key.slice(5, 7) - 1;
      if (y.months[m] < 0) y.months[m] = i;
    }
    // Four shades, split at the quartiles of the days with photos.
    const counts = [...days.values()].map((d) => d.length).sort((a, b) => a - b);
    const q = (f: number) => counts[Math.floor(f * (counts.length - 1))] ?? 0;
    const cuts = [q(0.25), q(0.5), q(0.75)];
    const order = [...years.values()].sort((a, b) => (newestFirst ? b.year - a.year : a.year - b.year));
    return { days, years: order, cuts, undated, firstUndated };
  }, [list, newestFirst]);

  const startDay = startIndex >= 0 && list.taken[startIndex] > 0 ? dayKey(list.taken[startIndex]) : null;

  // Open on the year the grid was showing.
  useLayoutEffect(() => {
    const el = scroller.current?.querySelector<HTMLElement>(`[data-year="${startDay?.slice(0, 4)}"]`);
    if (el && scroller.current) scroller.current.scrollTop = el.offsetTop - 12;
    scroller.current?.focus({ preventScroll: true });
  }, []); // only when it opens

  const years = useMemo(() => {
    const shade = (n: number) => (n <= cal.cuts[0] ? 1 : n <= cal.cuts[1] ? 2 : n <= cal.cuts[2] ? 3 : 4);
    return cal.years.map((y, yi) => {
      const prev = cal.years[yi - 1];
      const gap = prev ? Math.abs(y.year - prev.year) - 1 : 0;
      const from = prev ? Math.min(y.year, prev.year) + 1 : 0;
      const jan1 = Date.UTC(y.year, 0, 1);
      // Weeks are columns, Monday first.
      const offset = (new Date(jan1).getUTCDay() + 6) % 7;
      const length = (Date.UTC(y.year + 1, 0, 1) - jan1) / DAY_MS;
      const cells = [];
      for (let d = 0; d < length; d++) {
        const date = new Date(jan1 + d * DAY_MS);
        const key = `${y.year}-${pad2(date.getUTCMonth() + 1)}-${pad2(date.getUTCDate())}`;
        const at = cal.days.get(key);
        const slot = d + offset;
        cells.push(
          <div
            key={key}
            className={`cal-day ${at ? `l${shade(at.length)}` : ""} ${key === startDay ? "here" : ""}`}
            data-day={at ? key : undefined}
            style={{ gridColumn: Math.floor(slot / 7) + 2, gridRow: (slot % 7) + 2 }}
          />,
        );
      }
      return (
        <section key={y.year} data-year={y.year} className="cal-year-block">
          {gap > 0 && <div className="cal-gap">{gap === 1 ? from : `${from} – ${from + gap - 1}`} · no photos</div>}
          <div className="cal-year-head">
            <button className="cal-year" onClick={() => pick.current(y.first)} title={`Go to ${y.year} in the grid`}>
              {y.year}
            </button>
            <span className="muted">{fmtNum(y.photos)} photos</span>
          </div>
          <div className="cal-grid">
            {y.months.map((first, m) => (
              <button
                key={m}
                className="cal-month"
                disabled={first < 0}
                style={{ gridColumn: `${Math.floor(((Date.UTC(y.year, m, 1) - jan1) / DAY_MS + offset) / 7) + 2} / span 4` }}
                onClick={() => pick.current(first)}
                title={first < 0 ? undefined : `Go to ${MONTHS[m]} ${y.year} in the grid`}
              >
                {MONTHS[m]}
              </button>
            ))}
            {WEEKDAYS.map((w, i) => (
              <span key={i} className="cal-wd" style={{ gridRow: i + 2 }}>
                {w}
              </span>
            ))}
            {cells}
          </div>
        </section>
      );
    });
  }, [cal, startDay]);

  const dayAt = (e: ReactMouseEvent) => (e.target as HTMLElement).closest<HTMLElement>("[data-day]");
  const onOver = (e: ReactMouseEvent) => {
    const cell = dayAt(e);
    const t = e.target as HTMLElement;
    // The gaps between days keep the last one, so moving along a week doesn't flicker.
    if (!cell && t.classList.contains("cal-grid")) return;
    if (!cell || !body.current || !scroller.current) return setHover(null);
    const key = cell.dataset.day!;
    if (hover?.key === key) return;
    const r = cell.getBoundingClientRect();
    const b = body.current.getBoundingClientRect();
    const half = 240;
    const x = Math.max(half, Math.min(b.width - half, r.left + r.width / 2 - b.left));
    // Above the day, unless that's off the top of the calendar.
    const above = r.top - scroller.current.getBoundingClientRect().top > STRIP_H + 60;
    setHover({ key, x, y: above ? r.top - b.top - 8 : r.bottom - b.top + 8, above });
  };

  const hovered = hover ? cal.days.get(hover.key) : undefined;
  const n = Math.min(STRIP, hovered?.length ?? 0);
  // Spread through the day, first photo first.
  const strip = Array.from({ length: n }, (_, k) => hovered![Math.floor((k * hovered!.length) / n)]);

  return (
    <div className="calendar" ref={scroller} tabIndex={-1}>
      <div
        className="cal-body"
        ref={body}
        onMouseOver={onOver}
        onMouseLeave={() => setHover(null)}
        onClick={(e) => {
          const cell = dayAt(e);
          const at = cell && cal.days.get(cell.dataset.day!);
          if (at) pick.current(at[0]);
        }}
      >
        {!cal.years.length && <p className="muted cal-empty">No photos with a date here.</p>}
        {years}
        {cal.undated > 0 && (
          <button className="btn small ghost cal-undated" onClick={() => pick.current(cal.firstUndated)}>
            {fmtNum(cal.undated)} {cal.undated === 1 ? "photo" : "photos"} without a date
          </button>
        )}
        {hover && hovered && (
          <div
            className="cal-pop"
            style={{ left: hover.x, top: hover.y, transform: `translate(-50%, ${hover.above ? "-100%" : "0"})` }}
          >
            <div className="cal-pop-head">
              <span>{fmtDateKey(hover.key)}</span>
              <span className="muted">
                {fmtNum(hovered.length)} {hovered.length === 1 ? "photo" : "photos"}
              </span>
            </div>
            <div className="cal-pop-strip">
              {strip.map((i) => {
                const a = list.w[i] && list.h[i] ? list.w[i] / list.h[i] : 1.5;
                const w = Math.round(STRIP_H * Math.min(1.8, Math.max(0.6, a)));
                return list.preview[i] === 1 ? (
                  <img key={i} src={thumbUrl(list.ids[i], list.rev[i])} style={{ width: w }} alt="" draggable={false} />
                ) : (
                  <span key={i} style={{ width: w }} />
                );
              })}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
