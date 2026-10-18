import { Focus, ImageOff, Trash, Trash2, TriangleAlert, Unplug } from "lucide-react";
import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { BIN, fmtDeleted, fmtNum, problemsOf, thumbUrl, type MissingPhoto, type PhotoList } from "../api";
import { buildLayout, firstVisible, type Row, type Tile } from "../layout";
import { TimelineRail } from "./TimelineRail";

const GAP = 4;
const PAD = 20;
const OVERSCAN = 800;
/** Room kept above/below the current photo when the keyboard scrolls to it. */
const MARGIN = 24;

interface Props {
  list: PhotoList;
  rowHeight: number;
  /** The current photo (keyboard cursor; scrolled into view). */
  focusIndex: number | null;
  /** Selected photo ids. */
  selected: Set<number>;
  /** Missing photos whose files are in the system's bin (deleted, not moved). */
  binned?: Map<number, MissingPhoto> | null;
  /** The cursor moved and/or the selection changed. */
  onSelect: (cursor: number, selected: Set<number>) => void;
  onOpen: (index: number) => void;
  /** Delete: mark (or unmark) these photos for deletion. */
  onMark: (ids: number[], marked: boolean) => void;
  /** T: tag these photos. */
  onTag: (ids: number[]) => void;
  /** R / Shift+R: turn these photos a quarter turn clockwise (1) or counter-clockwise (-1). */
  onTurn?: (ids: number[], by: number) => void;
  /** Drawn turned by this many quarter turns clockwise (the turns suggested, to look them over);
   *  the list's sizes are already those of the photos turned. */
  turned?: Map<number, number> | null;
  /** Quarter turns the selected photos would get: pointing at Turn draws them so, before the click. */
  previewTurn?: number;
  /** False while something covers the grid (viewer, dialog): keys are theirs. */
  keyboard: boolean;
  /** Changes when the grid should take the keyboard back (viewer closed). */
  focusRequest: number;
  /** Set for search results: one ranked group under this label, no months. */
  flatLabel?: string;
  /** The file type (CR3, JPG...) in a corner of each thumbnail. */
  showType: boolean;
  /** What Cull found wrong (blurry, dark...) in a corner of each thumbnail. */
  showProblems: boolean;
  /** Brings this photo to the top of the grid (a day picked in the calendar); a new `seq` each time. */
  jump?: { index: number; seq: number } | null;
  /** The first photo on screen changed (where the calendar opens). */
  onTopPhoto?: (index: number) => void;
}

type PhotosRow = Extract<Row, { kind: "photos" }>;
const centre = (t: Tile) => t.left + t.width / 2;

/** A thumbnail drawn turned clockwise by `turn` quarter turns in a tile `w` x `h`: a quarter
 *  turn lays it out the other way round first, so once turned it fills the tile. */
function turnedStyle(turn: number, w: number, h: number) {
  const odd = turn % 2 === 1;
  return {
    width: odd ? h : w,
    height: odd ? w : h,
    transform: `translate(-50%, -50%) rotate(${turn * 90}deg)`,
  };
}

/** Drawn again only when what it shows changes (not for every update of the window). */
export const PhotoGrid = memo(function PhotoGrid(p: Props) {
  const { list, rowHeight, focusIndex, selected, onSelect, flatLabel } = p;
  const scroller = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [viewport, setViewport] = useState({ top: 0, height: 0 });
  // Where Shift+click / Shift+arrows ranges start.
  const anchor = useRef<number | null>(null);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const measure = () => {
      setWidth(el.clientWidth - PAD * 2);
      setViewport({ top: el.scrollTop, height: el.clientHeight });
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Take the keyboard only when asked (the viewer closed) - not when the grid
  // is rebuilt for another folder, which would steal it from the sidebar.
  const askedAtMount = useRef(p.focusRequest);
  useEffect(() => {
    if (p.focusRequest !== askedAtMount.current) scroller.current?.focus({ preventScroll: true });
  }, [p.focusRequest]);

  const layout = useMemo(
    () => (width > 0 ? buildLayout(list, width, rowHeight, GAP, flatLabel) : { rows: [], height: 0 }),
    [list, width, rowHeight, flatLabel],
  );

  // Which row each photo is in, for moving up and down.
  const rowOf = useMemo(() => {
    const m = new Int32Array(list.ids.length).fill(-1);
    layout.rows.forEach((r, ri) => r.kind === "photos" && r.tiles.forEach((t) => (m[t.index] = ri)));
    return m;
  }, [layout, list.ids.length]);

  // A photo picked in the calendar goes to the top, under its month's header
  // when it starts the month.
  const jumped = useRef(p.jump?.seq ?? 0); // a grid that mounts later ignores an old jump
  useEffect(() => {
    const el = scroller.current;
    if (!p.jump || p.jump.seq === jumped.current || !el) return;
    const ri = rowOf[p.jump.index];
    const row = layout.rows[ri];
    if (!row) return;
    jumped.current = p.jump.seq;
    const above = layout.rows[ri - 1];
    el.scrollTop = above?.kind === "header" ? above.top : row.top - MARGIN;
  }, [p.jump, layout, rowOf]);

  // Keep the current photo on screen.
  useEffect(() => {
    if (focusIndex == null || focusIndex < 0 || !scroller.current) return;
    const row = layout.rows[rowOf[focusIndex]];
    if (!row) return;
    const el = scroller.current;
    if (row.top < el.scrollTop) el.scrollTop = Math.max(0, row.top - MARGIN);
    else if (row.top + row.height > el.scrollTop + el.clientHeight)
      el.scrollTop = row.top + row.height - el.clientHeight + MARGIN;
  }, [focusIndex, layout, rowOf]);

  const frame = useRef(0);
  const onScroll = () => {
    cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => {
      const el = scroller.current;
      if (!el) return;
      setViewport({ top: el.scrollTop, height: el.clientHeight });
      p.onTopPhoto?.(firstPhotoAt(el.scrollTop));
    });
  };

  /** Moves the cursor to `i`: alone, extending a range (shift), toggling (ctrl), or leaving the selection (keep). */
  const moveTo = (i: number, how: { shift?: boolean; ctrl?: boolean; keep?: boolean } = {}) => {
    if (i < 0 || i >= list.ids.length) return;
    const cur = focusIndex != null && focusIndex >= 0 ? focusIndex : i;
    let next: Set<number>;
    if (how.shift) {
      const a = anchor.current ?? cur;
      anchor.current = a;
      next = how.ctrl ? new Set(selected) : new Set();
      for (let k = Math.min(a, i); k <= Math.max(a, i); k++) next.add(list.ids[k]);
    } else if (how.ctrl) {
      next = new Set(selected);
      const id = list.ids[i];
      if (next.has(id)) next.delete(id);
      else next.add(id);
      anchor.current = i;
    } else if (how.keep) {
      next = selected;
    } else {
      next = new Set([list.ids[i]]);
      anchor.current = i;
    }
    onSelect(i, next);
  };

  /** The photo above/below `i` (dir -1/+1), `skip` pixels away at least, nearest horizontally. */
  const vertical = (i: number, dir: number, skip = 0) => {
    const ri = rowOf[i];
    const from = layout.rows[ri] as PhotosRow | undefined;
    if (!from) return i;
    const t = from.tiles.find((x) => x.index === i)!;
    let best: number | null = null;
    for (let r = ri + dir; r >= 0 && r < layout.rows.length; r += dir) {
      const row = layout.rows[r];
      if (row.kind !== "photos") continue;
      best = row.tiles.reduce((a, b) => (Math.abs(centre(b) - centre(t)) < Math.abs(centre(a) - centre(t)) ? b : a)).index;
      if (Math.abs(row.top - from.top) >= skip) break;
    }
    return best ?? i;
  };

  /** First photo below `y`. */
  const firstPhotoAt = (y: number) => {
    for (let r = firstVisible(layout.rows, y); r < layout.rows.length; r++) {
      const row = layout.rows[r];
      if (row.kind === "photos") return row.tiles[0].index;
    }
    return 0;
  };
  /** First photo on screen, where the keyboard starts when nothing is current. */
  const firstOnScreen = () => firstPhotoAt(viewport.top);

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (!p.keyboard || !list.ids.length) return;
    const n = list.ids.length;
    const cur = focusIndex != null && focusIndex >= 0 ? focusIndex : -1;
    const ctrl = e.ctrlKey || e.metaKey;
    const how = { shift: e.shiftKey, keep: ctrl && !e.shiftKey, ctrl: ctrl && e.shiftKey };
    const step = (target: number) => moveTo(cur < 0 ? firstOnScreen() : target, cur < 0 ? {} : how);
    switch (e.key) {
      case "ArrowRight":
        step(Math.min(n - 1, cur + 1));
        break;
      case "ArrowLeft":
        step(Math.max(0, cur - 1));
        break;
      case "ArrowDown":
        step(vertical(cur, 1));
        break;
      case "ArrowUp":
        step(vertical(cur, -1));
        break;
      case "PageDown":
        step(vertical(cur, 1, viewport.height * 0.8));
        break;
      case "PageUp":
        step(vertical(cur, -1, viewport.height * 0.8));
        break;
      case "Home":
        moveTo(0, how);
        break;
      case "End":
        moveTo(n - 1, how);
        break;
      case " ":
        if (!ctrl || cur < 0) return;
        moveTo(cur, { ctrl: true }); // Ctrl+Space: add or remove the current photo
        break;
      case "a":
      case "A":
        if (!ctrl) return;
        onSelect(cur, new Set(list.ids));
        break;
      case "Enter":
        if (cur >= 0) p.onOpen(cur);
        break;
      case "Escape":
        if (!selected.size) return; // nothing selected: Escape means Back
        onSelect(cur, new Set());
        e.stopPropagation();
        break;
      case "t":
      case "T": {
        if (ctrl) return;
        const ids = list.ids.filter((id) => selected.has(id));
        if (!ids.length && cur >= 0) ids.push(list.ids[cur]);
        if (!ids.length) return;
        p.onTag(ids);
        break;
      }
      case "r":
      case "R": {
        if (ctrl || e.altKey || !p.onTurn) return;
        const ids = list.ids.filter((id) => selected.has(id));
        if (!ids.length && cur >= 0) ids.push(list.ids[cur]);
        if (!ids.length) return;
        p.onTurn(ids, e.shiftKey ? -1 : 1);
        break;
      }
      case "Delete": {
        const ids = list.ids.filter((id) => selected.has(id));
        if (!ids.length && cur >= 0) ids.push(list.ids[cur]);
        if (!ids.length) return;
        const pos = new Map(list.ids.map((id, i) => [id, i]));
        const allMarked = ids.every((id) => list.marked[pos.get(id)!] === 1);
        p.onMark(ids, !allMarked);
        break;
      }
      default:
        return;
    }
    e.preventDefault();
  };

  const visible = [];
  const start = firstVisible(layout.rows, viewport.top - OVERSCAN);
  const bottom = viewport.top + viewport.height + OVERSCAN;
  for (let r = start; r < layout.rows.length && layout.rows[r].top < bottom; r++) visible.push(layout.rows[r]);

  // The date rail, for lists in date order that don't fit on screen.
  const marks = useMemo(
    () => (flatLabel ? [] : layout.rows.flatMap((r) => (r.kind === "header" ? [{ top: r.top, ts: r.ts }] : []))),
    [layout, flatLabel],
  );
  const rail = marks.length >= 2 && layout.height + PAD > viewport.height;

  return (
    <div className="grid-area">
      <div
        className={`grid-scroller ${rail ? "with-rail" : ""}`}
        ref={scroller}
        onScroll={onScroll}
        onKeyDown={onKeyDown}
        tabIndex={0}
      >
        <div className="grid-canvas" style={{ height: layout.height + PAD }}>
          {visible.map((row) =>
            row.kind === "header" ? (
              <div key={`h${row.top}`} className="grid-header" style={{ top: row.top, left: PAD, right: PAD }}>
                <span>{row.label}</span>
                <span className="muted">{fmtNum(row.count)}</span>
              </div>
            ) : (
              row.tiles.map((t) => {
                const id = list.ids[t.index];
                const state = list.state[t.index];
                const preview = list.preview[t.index];
                const marked = list.marked[t.index] === 1;
                const inBin = state === 0 ? p.binned?.get(id) : undefined;
                const problems = problemsOf(list.flags[t.index] ?? 0);
                const turn = ((((p.turned?.get(id) ?? 0) + (p.previewTurn && selected.has(id) ? p.previewTurn : 0)) % 4) + 4) % 4;
                return (
                  <button
                    key={id}
                    tabIndex={-1}
                    className={`tile ${state === 1 ? "is-offline" : ""} ${state === 0 ? "is-missing" : ""} ${marked ? "is-marked" : ""} ${selected.has(id) ? "is-selected" : ""} ${t.index === focusIndex ? "is-focus" : ""}`}
                    style={{ top: row.top, left: PAD + t.left, width: t.width, height: row.height }}
                    // Tiles come and go as the grid scrolls, so the grid keeps the keyboard.
                    onMouseDown={(e) => {
                      e.preventDefault();
                      scroller.current?.focus({ preventScroll: true });
                    }}
                    onClick={(e) => moveTo(t.index, { shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey })}
                    onDoubleClick={() => p.onOpen(t.index)}
                    title={
                      marked
                        ? "Marked for deletion (Delete to unmark)"
                        : state === 1
                          ? "Not reachable now: disk unplugged or folder not found"
                          : inBin
                            ? `In the ${BIN}${inBin.deletedAt ? ` since ${fmtDeleted(inBin.deletedAt)}` : ""}: deleted, not moved`
                            : state === 0
                              ? "Missing: not found where it was"
                              : undefined
                    }
                  >
                    {preview === 1 ? (
                      <img
                        src={thumbUrl(id, list.rev[t.index])}
                        loading="lazy"
                        decoding="async"
                        draggable={false}
                        alt=""
                        className={turn ? "turned" : undefined}
                        style={turn ? turnedStyle(turn, t.width, row.height) : undefined}
                      />
                    ) : preview === 2 ? (
                      <span className="tile-empty">
                        <ImageOff size={22} />
                      </span>
                    ) : (
                      <span className="tile-empty pending" />
                    )}
                    {state === 1 && (
                      <span className="badge offline">
                        <Unplug size={13} />
                      </span>
                    )}
                    {state === 0 && (
                      <span className={`badge missing ${inBin ? "in-bin" : ""}`}>
                        {inBin ? <Trash size={13} /> : <TriangleAlert size={13} />}
                      </span>
                    )}
                    {list.fresh[t.index] === 1 && (
                      <span className="new-badge" title="New: found since you last looked at this person">
                        New
                      </span>
                    )}
                    {marked && (
                      <span className="badge marked">
                        <Trash2 size={13} />
                      </span>
                    )}
                    {p.showProblems && problems.length > 0 && !marked && (
                      <span
                        className={`badge problem ${list.fresh[t.index] === 1 ? "after-new" : ""}`}
                        title={problems.map((x) => x.label).join(" · ")}
                      >
                        <Focus size={13} />
                      </span>
                    )}
                    {p.showType && list.ext[t.index] && (
                      <span className={`type-tag ${list.raw[t.index] === 1 ? "raw" : ""}`}>
                        {list.ext[t.index].toUpperCase()}
                      </span>
                    )}
                  </button>
                );
              })
            ),
          )}
        </div>
      </div>
      {rail && (
        <TimelineRail
          marks={marks}
          content={layout.height + PAD}
          viewport={viewport.height}
          scrollTop={viewport.top}
          onScrollTo={(top) => scroller.current && (scroller.current.scrollTop = top)}
          onWheel={(dy) => scroller.current && (scroller.current.scrollTop += dy)}
        />
      )}
    </div>
  );
});
