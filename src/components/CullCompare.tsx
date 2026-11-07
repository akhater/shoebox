import { memo, useMemo } from "react";
import type { CullFrame, CullStack } from "../api";
import { ComparePane } from "./ComparePane";
import type { Spot } from "./CullPicture";
import { FaceRows } from "./FaceRows";

/**
 * The frames compared: `n` of `order` (indices into a burst's frames) with
 * `current` among them, from `from` on while `current` stays in view (null:
 * from `current` on).
 */
export function compareSet(order: number[], current: number, from: number | null, n: number) {
  const k = Math.min(n, order.length);
  const p = Math.max(0, order.indexOf(current));
  const start = Math.max(0, Math.min(Math.max(Math.min(from ?? p, p), p - k + 1), order.length - k));
  return { from: start, frames: order.slice(start, start + k) };
}

/**
 * Cull's compare: frames of a burst side by side, each with what Cull says,
 * at 100% together on the same spot; below, each face cut out of every frame.
 * Marks, the frame looked at and 100% are Cull's own (CullView).
 */
export const CullCompare = memo(function CullCompare({
  stack,
  shown,
  current,
  isMarked,
  zoom,
  onLook,
  onZoom,
}: {
  stack: CullStack;
  /** The frames compared, by index into the stack's. */
  shown: number[];
  /** The frame looked at (an index). */
  current: number;
  isMarked: (f: CullFrame) => boolean;
  /** At 100%: where, for every frame. */
  zoom: Spot | null;
  /** Looks at a frame, by id. */
  onLook: (id: number) => void;
  onZoom: (s: Spot | null) => void;
}) {
  const frames = useMemo(() => shown.map((i) => stack.frames[i]), [shown, stack]);
  const starred = stack.frames.length > 1 && stack.kind !== "focus";
  return (
    <div className="cull-compare">
      <div className="cull-panes">
        {shown.map((i) => {
          const f = stack.frames[i];
          return (
            <ComparePane
              key={f.id}
              frame={f}
              number={i + 1}
              total={stack.frames.length}
              keeper={starred && i === stack.keeper}
              marked={isMarked(f)}
              current={i === current}
              spot={zoom}
              onLook={onLook}
              onZoom={onZoom}
            />
          );
        })}
      </div>
      <FaceRows frames={frames} current={stack.frames[current]?.id ?? null} onLook={onLook} />
    </div>
  );
});
