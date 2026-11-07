import { Star, Trash2 } from "lucide-react";
import { memo, useRef, type MouseEvent as ReactMouseEvent } from "react";
import { previewUrl, type CullFrame } from "../api";
import { clickedSpot, firstSpot, missedPoints, OnPicture, Zoomed, type Spot } from "./CullPicture";
import { FocusPoints } from "./FocusPoints";

const says: Record<CullFrame["suggest"], string> = { keep: "Keep", reject: "Reject", none: "Your call" };

/**
 * One frame compared: fitted, or at 100% on `spot` as the others are; what
 * Cull says under it. A click looks at it; on the frame looked at, 100% there.
 */
export const ComparePane = memo(function ComparePane({
  frame,
  number,
  total,
  keeper,
  marked,
  current,
  spot,
  onLook,
  onZoom,
}: {
  frame: CullFrame;
  /** Its place in the burst, from 1. */
  number: number;
  total: number;
  /** The frame Cull would keep. */
  keeper: boolean;
  marked: boolean;
  /** The frame looked at: X and P act on it. */
  current: boolean;
  spot: Spot | null;
  onLook: (id: number) => void;
  onZoom: (s: Spot | null) => void;
}) {
  const picture = useRef<HTMLImageElement>(null);
  const missed = missedPoints(frame);
  const click = (e: ReactMouseEvent<HTMLDivElement>) => {
    if (current) onZoom(clickedSpot(e) ?? firstSpot(frame));
    else onLook(frame.id);
  };
  return (
    <div className={`cull-pane ${current ? "current" : ""} ${marked ? "rejected" : ""}`} data-frame={frame.id}>
      <div className="cull-pane-pic" onMouseDownCapture={spot && !current ? () => onLook(frame.id) : undefined}>
        {spot ? (
          <Zoomed frame={frame} points={missed} spot={spot} onMove={onZoom} onExit={() => onZoom(null)} />
        ) : (
          <div
            className="cull-pane-photo"
            onClick={click}
            title={current ? "Click: every frame at 100% here" : "Click: look at this frame (X, P act on it)"}
          >
            <img key={frame.id} ref={picture} src={previewUrl(frame.id, frame.rev)} alt="" draggable={false} />
            {missed && (
              <OnPicture key={frame.id} picture={picture}>
                <FocusPoints focus={missed} />
              </OnPicture>
            )}
          </div>
        )}
        {keeper && (
          <span className="cull-keeper" title="The frame Cull would keep">
            <Star size={14} />
          </span>
        )}
        {marked && (
          <span className="cull-stamp rejected">
            <Trash2 size={14} /> Rejected
          </span>
        )}
      </div>
      <div className="cull-pane-says" title={frame.reasons.join(" · ")}>
        <span className={`cull-suggest ${frame.suggest}`}>{says[frame.suggest]}</span>
        <span className="muted ellipsis grow">{frame.reasons[0] ?? ""}</span>
        <span className="muted small">
          {number} of {total}
        </span>
      </div>
    </div>
  );
});
