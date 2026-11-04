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

/** An edited version of a photo the viewer can show: its own edit's, or a virtual copy's.
 *  `key` stays the same when the edit is rendered again (a new `version`). */
export interface EditedVersion {
  key: string;
  version: string;
  label: string;
}

/** The versions Argentum rendered of a photo: its own edit first, then each virtual copy's.
 *  The same edit beside two copies of the photo is one version; two different ones are told
 *  apart by their disk. */
export function editedVersions(files: { id: number; volumeLabel: string; edits?: FileEdit[] }[]): EditedVersion[] {
  const found = files.flatMap((f) => (f.edits ?? []).filter((e) => e.version != null).map((e) => ({ e, f })));
  const unique = found.filter((x, i) => found.findIndex((y) => y.e.version === x.e.version) === i);
  const copies = [...new Set(unique.flatMap((x) => (x.e.copy == null ? [] : [x.e.copy])))].sort();
  const named = unique.map(({ e, f }) => {
    const base = e.copy == null ? "Edited" : copies.length > 1 ? `Copy ${copies.indexOf(e.copy) + 1}` : "Copy";
    const twins = unique.filter((x) => x.e.copy === e.copy).length > 1;
    return { key: `${e.copy ?? ""}@${f.id}`, version: e.version!, label: twins ? `${base} · ${f.volumeLabel}` : base, copy: e.copy };
  });
  const rank = (copy: string | null) => (copy == null ? -1 : copies.indexOf(copy));
  named.sort((a, b) => rank(a.copy) - rank(b.copy));
  // Still alike (two copies on disks of the same name): numbered.
  return named.map(({ key, version, label }, i) => ({
    key,
    version,
    label: named.filter((x) => x.label === label).length > 1 ? `${label} (${named.slice(0, i + 1).filter((x) => x.label === label).length})` : label,
  }));
}

/** What is left to show of a copy's edits, in words; null when every one shows. */
export function renderLine(edits: FileEdit[], rendering: EditRendering | null, plugged: boolean) {
  const failed = edits.find((e) => e.error != null);
  if (failed) return `Argentum couldn't render ${failed.copy ? "a virtual copy" : "it"}: ${failed.error}`;
  if (!edits.some((e) => e.waiting)) return null;
  if (rendering === "outdated") return "Update Argentum to show edits";
  if (rendering === "missing") return "Argentum isn't on this computer: its edits show once it is";
  if (rendering === "cantStart") return "Argentum couldn't start: its edits show once it does";
  if (rendering === "stopped") return "Rendering edits is stopped";
  if (!plugged) return "The edit shows once its disk is plugged in";
  return rendering === "ready" ? "Argentum is rendering the edit…" : null;
}
