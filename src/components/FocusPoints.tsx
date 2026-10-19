import { memo, type ReactNode } from "react";
import type { Focus, FocusPoint, FocusState } from "../api";

/** A viewfinder's corners around a small square (the focus point), filled
 *  when the points are shown. Not a bare corner frame: that reads "full screen". */
export function FocusIcon({ on, size = 17 }: { on: boolean; size?: number }) {
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
      aria-hidden="true"
    >
      <path d="M3 8V5a2 2 0 0 1 2-2h3" />
      <path d="M16 3h3a2 2 0 0 1 2 2v3" />
      <path d="M21 16v3a2 2 0 0 1-2 2h-3" />
      <path d="M8 21H5a2 2 0 0 1-2-2v-3" />
      <rect x="9" y="9" width="6" height="6" rx="1" fill={on ? "currentColor" : "none"} />
    </svg>
  );
}

/** Something to draw: a point the photo used, or its zone (a camera's other points alone say nothing). */
export const hasFocusPoints = (f: Focus | null | undefined) => !!f && f.points.some((p) => p.state !== "other");

/** Drawn last on top: in focus over chosen over the zone over the camera's other points. */
const ORDER: Record<FocusState, number> = { other: 0, area: 1, chosen: 2, inFocus: 3 };

/**
 * The camera's focus points over the photo, in a box the size of the photo:
 * the other points faint, the chosen ones white, those in focus red, the zone
 * given dashed. Each drawn over a dark halo, to show on bright and dark photos.
 */
export const FocusPoints = memo(function FocusPoints({ focus, faded }: { focus: Focus; faded?: boolean }) {
  const points = [...focus.points].sort((a, b) => ORDER[a.state] - ORDER[b.state]);
  const box = (p: FocusPoint) => ({
    x: `${p.x * 100}%`,
    y: `${p.y * 100}%`,
    width: `${p.w * 100}%`,
    height: `${p.h * 100}%`,
    rx: 2,
  });
  return (
    <svg className={`focus-points ${faded ? "faded" : ""}`} aria-hidden="true">
      {points.map((p, i) => (
        <g key={i} className={`af ${p.state}`}>
          <rect {...box(p)} className="halo" />
          <rect {...box(p)} className="line" />
        </g>
      ))}
    </svg>
  );
});

/** The info panel's line about focus: where the camera confirmed focus or
 *  what was chosen, then the mode. With the points shown, each part has its
 *  colour (the legend). Confirmed is the camera's word when it focused, not
 *  that the photo is sharp: shake and motion come after (Sharpness says). */
export function FocusSummary({ focus, legend }: { focus: Focus; legend: boolean }) {
  const n = focus.points.length;
  const layout = focus.points.some((p) => p.state === "other");
  const count = (s: FocusState) => focus.points.filter((p) => p.state === s).length;
  const [inFocus, chosen, area] = [count("inFocus"), count("chosen"), count("area")];
  const part = (key: FocusState, text: string) => (
    <span key={key} className="nowrap">
      {legend && <i className={`af-swatch ${key}`} />}
      {text}
    </span>
  );
  const parts: ReactNode[] = [];
  if (inFocus) {
    const text = layout
      ? `Focus confirmed on ${inFocus} of ${n}`
      : inFocus === 1
        ? "Focus confirmed"
        : `Focus confirmed on ${inFocus} points`;
    parts.push(part("inFocus", text));
  }
  if (chosen) {
    const text = inFocus
      ? `${chosen} chosen`
      : layout
        ? `${chosen} of ${n} chosen, focus not confirmed`
        : "Focus not confirmed";
    parts.push(part("chosen", text));
  }
  if (area && legend) parts.push(part("area", "the zone given"));
  const manual = focus.mode === "Manual focus";
  return (
    <span className="focus-summary">
      {parts.length > 0 ? (
        parts.flatMap((p, i) => (i ? [", ", p] : [p]))
      ) : manual ? (
        "Manual focus"
      ) : (
        <span className="muted">No point recorded</span>
      )}
      {focus.mode && !(manual && parts.length === 0) && <span className="muted nowrap"> · {focus.mode}</span>}
    </span>
  );
}
