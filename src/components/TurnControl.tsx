import type { MouseEvent as ReactMouseEvent } from "react";

/** A photo with an arrow going round its corner, turning it: the way photo
 *  apps draw a turn. Never a closed circle of arrows (reads "reload"), a bare
 *  hook (reads "undo"), or an arc rising from the frame (reads "unlocked"). */
export function TurnIcon({ dir = "right", size = 17, className }: { dir?: "left" | "right"; size?: number; className?: string }) {
  const parts =
    dir === "right"
      ? [<rect key="r" x="2" y="10" width="11" height="11" rx="2" />, "M5 6h7a7 7 0 0 1 7 7v1.5", "m16 12.5 3 3 3-3"]
      : [<rect key="r" x="11" y="10" width="11" height="11" rx="2" />, "M19 6h-7a7 7 0 0 0-7 7v1.5", "m2 12.5 3 3 3-3"];
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden="true"
    >
      {parts.map((p) => (typeof p === "string" ? <path key={p} d={p} /> : p))}
    </svg>
  );
}

/** Sideways (sidebar, AI models): the turn, as a lucide-sized icon. */
export const TurnPhotoIcon = ({ size = 16, className }: { size?: number; className?: string }) => (
  <TurnIcon dir="right" size={size} className={className} />
);

/**
 * "Turn" between a left and a right arrow: one control, so what it does is
 * read, not guessed. `on="photo"`: floating on the photo in the viewer;
 * `on="toolbar"`: with the grid's other buttons. `what` names what turns,
 * for the hover text ("this photo", "the 3 selected").
 */
export function TurnControl({ onTurn, what, on }: { onTurn: (by: number) => void; what: string; on: "photo" | "toolbar" }) {
  const turn = (by: number) => (e: ReactMouseEvent) => {
    e.stopPropagation(); // on the photo, a click elsewhere closes the viewer
    onTurn(by);
  };
  return (
    <div className={`turn-control on-${on}`} role="group" aria-label="Turn" onClick={(e) => e.stopPropagation()}>
      <button onClick={turn(-1)} aria-label="Turn left" title={`Turn ${what} left (Shift+R)`}>
        <TurnIcon dir="left" />
      </button>
      <span className="turn-label">Turn</span>
      <button onClick={turn(1)} aria-label="Turn right" title={`Turn ${what} right (R)`}>
        <TurnIcon dir="right" />
      </button>
    </div>
  );
}
