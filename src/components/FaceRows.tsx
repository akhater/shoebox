import { memo, useEffect, useRef, useState, type CSSProperties } from "react";
import { fullUrl, originalUrl, people, type CullFrame, type PhotoFace } from "../api";

/** How long the frames compared stay before their originals are read for the faces, ms. */
const SETTLE = 400;

const width = (f: PhotoFace) => f.bbox[2] - f.bbox[0];
const middle = (f: PhotoFace) => [(f.bbox[0] + f.bbox[2]) / 2, (f.bbox[1] + f.bbox[3]) / 2];

/**
 * The faces of frames, a row per face across them (`columns[c]` the faces of
 * frame c): the same person when the catalog knows who both are, else the
 * nearest face within a face's width of where it was in the last frame that
 * has it. Larger faces first.
 */
export function matchFaces(columns: PhotoFace[][]): (PhotoFace | null)[][] {
  const rows: (PhotoFace | null)[][] = [];
  columns.forEach((faces, c) => {
    for (const f of [...faces].sort((a, b) => width(b) - width(a))) {
      let best = -1;
      let bestD = Infinity;
      rows.forEach((row, r) => {
        if (row[c]) return;
        let last: PhotoFace | null = null;
        for (let k = c - 1; k >= 0 && !last; k--) last = row[k];
        if (!last) return;
        let d: number;
        if (f.personId != null && last.personId != null) {
          if (f.personId !== last.personId) return;
          d = 0;
        } else {
          const [ax, ay] = middle(f);
          const [bx, by] = middle(last);
          d = Math.hypot(ax - bx, ay - by);
          if (d > Math.max(width(f), width(last))) return;
        }
        if (d < bestD) {
          best = r;
          bestD = d;
        }
      });
      if (best < 0) {
        rows.push(columns.map(() => null));
        best = rows.length - 1;
      }
      rows[best][c] = f;
    }
  });
  const largest = (row: (PhotoFace | null)[]) => Math.max(...row.map((f) => (f ? width(f) : 0)));
  return rows.sort((a, b) => largest(b) - largest(a));
}

/**
 * Where a photo goes in a square box so that its face (`bbox`, 0..1 of the
 * photo) fills the middle with a margin around it. `aspect`: the photo's
 * width over its height.
 */
export function cropStyle(bbox: PhotoFace["bbox"], aspect: number): CSSProperties {
  const [x1, y1, x2, y2] = bbox;
  // The box's side, in widths of the photo.
  const side = Math.max(x2 - x1, (y2 - y1) / aspect) * 1.7;
  const cx = (x1 + x2) / 2;
  const cy = (y1 + y2) / 2;
  return {
    width: `${100 / side}%`,
    height: `${100 / (side * aspect)}%`,
    left: `${50 - (cx / side) * 100}%`,
    top: `${50 - (cy / (side * aspect)) * 100}%`,
  };
}

/** The faces asked for some frames, and the frames they are of. */
type Held = { ids: string; frames: CullFrame[]; faces: PhotoFace[] };

/**
 * Each face of the frames compared, cut out of every frame side by side to
 * compare expressions and eyes: from each photo's own preview (where its
 * faces' boxes are), then from its original when its disk is plugged in. A
 * click on a face looks at its frame. Until the faces of new frames are here,
 * the last ones stay, dimmed and not clickable.
 */
export const FaceRows = memo(function FaceRows({
  frames,
  current,
  onLook,
}: {
  frames: CullFrame[];
  /** The frame looked at, by id. */
  current: number | null;
  onLook: (id: number) => void;
}) {
  const ids = frames.map((f) => f.id).join(",");
  const [held, setHeld] = useState<Held | null>(null);
  // The frames whose originals are read for their faces: those compared a while.
  const [settled, setSettled] = useState("");
  const latest = useRef(frames);
  latest.current = frames;

  // Only the latest answer counts.
  const seq = useRef(0);
  useEffect(() => {
    const n = ++seq.current;
    const these = latest.current;
    const done = (faces: PhotoFace[]) => n === seq.current && setHeld({ ids, frames: these, faces });
    people
      .inPhotos(these.map((f) => f.id))
      .then(done)
      .catch(() => done([]));
    const t = window.setTimeout(() => setSettled(ids), SETTLE);
    return () => window.clearTimeout(t);
  }, [ids]);

  if (!held) return null;
  const rows = matchFaces(held.frames.map((f) => held.faces.filter((x) => x.photoId === f.id)));
  if (!rows.length) return null;
  const stale = held.ids !== ids;
  const sharp = !stale && settled === ids;
  return (
    <div className={`cull-faces ${stale ? "stale" : ""}`} style={{ "--columns": held.frames.length } as CSSProperties}>
      {rows.map((row, r) => {
        const name = row.find((f) => f?.name)?.name;
        return (
          <div className="cull-face-row" key={r}>
            {row.map((f, c) => {
              const frame = held.frames[c];
              const at = frame.id === current ? "current" : "";
              if (!f) return <span key={c} className={`cull-face none ${at}`} />;
              const style = cropStyle(f.bbox, frame.w / frame.h);
              return (
                <button
                  key={c}
                  className={`cull-face ${at}`}
                  data-face={f.id}
                  onClick={() => onLook(frame.id)}
                  title={`${name ? `${name} · ` : ""}${frame.name ?? `Photo ${frame.id}`}: click to look at this frame`}
                >
                  <span className="cull-face-crop">
                    <img className="cull-face-preview" src={originalUrl(frame.id, frame.rev)} alt="" draggable={false} style={style} />
                    {sharp && frame.state === 2 && (
                      <img
                        className="cull-face-full"
                        src={fullUrl(frame.id, 0, frame.rev)}
                        alt=""
                        draggable={false}
                        style={style}
                        onLoad={(e) => e.currentTarget.classList.add("in")}
                      />
                    )}
                  </span>
                </button>
              );
            })}
            {name && <span className="cull-face-name">{name}</span>}
          </div>
        );
      })}
    </div>
  );
});
