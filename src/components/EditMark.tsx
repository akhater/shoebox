import { createLucideIcon, SlidersHorizontal, type LucideProps } from "lucide-react";

/** A 35 mm film can with its leader out, drawn on Lucide's 24 px grid to sit beside its icons. */
export const FilmRoll = createLucideIcon("film-roll", [
  ["path", { d: "M5 3h3v2", key: "spool" }],
  ["rect", { x: "2", y: "5", width: "9", height: "16", rx: "2", key: "can" }],
  ["path", { d: "M11 8h9a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-9", key: "leader" }],
  ["path", { d: "M14.5 10.5h.01M18 10.5h.01M14.5 13.5h.01M18 13.5h.01", key: "holes" }],
]);

/** Argentum's mark on a photo it edited: sliders, or a film can when a film look is on (as Argentum shows it). */
export function EditMark({ film, ...props }: { film: boolean } & LucideProps) {
  const Mark = film ? FilmRoll : SlidersHorizontal;
  return <Mark {...props} />;
}

/** What the mark says, for a tooltip: `versions` = how many different edits Argentum keeps. */
export function editTitle(film: boolean, versions: number) {
  const what = film ? "Edited in Argentum, with a film look" : "Edited in Argentum";
  return versions > 1 ? `${what}: ${versions} versions` : what;
}

/** The edits Argentum keeps beside one copy of a photo, in words: its own, its virtual copies'. */
export function editLine(edits: { copy: string | null; film: boolean }[]) {
  const copies = edits.filter((x) => x.copy != null).length;
  const copyWords = `${copies} virtual ${copies === 1 ? "copy" : "copies"}`;
  const what = edits.some((x) => x.copy == null)
    ? copies
      ? `Edited in Argentum, and ${copyWords}`
      : "Edited in Argentum"
    : `${copyWords} edited in Argentum`;
  return edits.some((x) => x.film) ? `${what} · film look` : what;
}
