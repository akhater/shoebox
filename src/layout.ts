import { fmtMonth, type PhotoList } from "./api";

export interface Tile {
  index: number;
  left: number;
  width: number;
}

export type Row =
  | { kind: "header"; top: number; height: number; label: string; count: number; ts: number }
  | { kind: "photos"; top: number; height: number; tiles: Tile[] };

export interface Layout {
  rows: Row[];
  height: number;
}

const HEADER = 52;
const MIN_ASPECT = 0.3;
const MAX_ASPECT = 4;

function aspect(list: PhotoList, i: number) {
  const w = list.w[i];
  const h = list.h[i];
  if (!w || !h) return 1.5;
  return Math.min(MAX_ASPECT, Math.max(MIN_ASPECT, w / h));
}

function monthKey(ts: number) {
  if (ts <= 0) return -1;
  const d = new Date(ts * 1000);
  return d.getUTCFullYear() * 12 + d.getUTCMonth();
}

/** Justified rows (no cropping), grouped by month. */
export function buildLayout(list: PhotoList, width: number, target: number, gap: number, flatLabel?: string): Layout {
  const rows: Row[] = [];
  let top = 0;
  const n = list.ids.length;
  let i = 0;
  while (i < n) {
    // Search results keep their ranking: one group, no month headers.
    const key = flatLabel ? 0 : monthKey(list.taken[i]);
    let end = i;
    while (end < n && (flatLabel ? 0 : monthKey(list.taken[end])) === key) end++;

    rows.push({
      kind: "header",
      top,
      height: HEADER,
      label: flatLabel ?? fmtMonth(list.taken[i]),
      count: end - i,
      ts: list.taken[i],
    });
    top += HEADER;

    let row: number[] = [];
    let sum = 0;
    const flush = (fill: boolean) => {
      if (!row.length) return;
      const gaps = gap * (row.length - 1);
      const h = fill ? (width - gaps) / sum : Math.min(target, (width - gaps) / sum);
      let left = 0;
      const tiles = row.map((index) => {
        const w = aspect(list, index) * h;
        const t = { index, left, width: w };
        left += w + gap;
        return t;
      });
      rows.push({ kind: "photos", top, height: h, tiles });
      top += h + gap;
      row = [];
      sum = 0;
    };
    for (let j = i; j < end; j++) {
      row.push(j);
      sum += aspect(list, j);
      if (sum * target + gap * (row.length - 1) >= width) flush(true);
    }
    flush(false);
    i = end;
  }
  return { rows, height: top };
}

/** First row whose bottom edge is below `y`. */
export function firstVisible(rows: Row[], y: number) {
  let lo = 0;
  let hi = rows.length - 1;
  let ans = rows.length;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (rows[mid].top + rows[mid].height >= y) {
      ans = mid;
      hi = mid - 1;
    } else {
      lo = mid + 1;
    }
  }
  return ans;
}
