import { createLucideIcon, SlidersHorizontal, type LucideProps } from "lucide-react";
import type { EditRendering, FileEdit } from "../api";

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

/** An edited version of a photo the viewer can show: its own edit's, or a virtual copy's. */
export interface EditedVersion {
  version: string;
  label: string;
}

/** The versions Argentum rendered of a photo, its own edit first, then each virtual copy's. */
export function editedVersions(files: { edits?: FileEdit[] }[]): EditedVersion[] {
  const edits = files.flatMap((f) => f.edits ?? []).filter((e) => e.version != null);
  const own = edits.filter((e) => e.copy == null);
  const copies = [...new Set(edits.filter((e) => e.copy != null).map((e) => e.copy!))].sort();
  const out: EditedVersion[] = [];
  const add = (version: string, label: string) => {
    if (!out.some((v) => v.version === version)) out.push({ version, label });
  };
  own.forEach((e) => add(e.version!, "Edited"));
  copies.forEach((copy, i) => add(edits.find((e) => e.copy === copy)!.version!, copies.length > 1 ? `Copy ${i + 1}` : "Copy"));
  return out;
}

/** What is left to show of a copy's edits, in words; null when every one shows. */
export function renderLine(edits: FileEdit[], rendering: EditRendering | null, plugged: boolean) {
  const failed = edits.find((e) => e.error != null);
  if (failed) return `Argentum couldn't render ${failed.copy ? "a virtual copy" : "it"}: ${failed.error}`;
  if (!edits.some((e) => e.waiting)) return null;
  if (rendering === "outdated") return "Update Argentum to show edits";
  if (rendering === "missing") return "Argentum isn't on this computer: its edits show once it is";
  if (!plugged) return "The edit shows once its disk is plugged in";
  return rendering === "ready" ? "Argentum is rendering the edit…" : null;
}
