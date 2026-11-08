// How Cull's compare lines up the faces of a burst's frames: a row per face.
import { describe, expect, test } from "vitest";
import type { PhotoFace } from "../api";
import { matchFaces } from "../components/FaceRows";

const face = (id: number, x: number, y: number, w = 0.04, personId: number | null = null): PhotoFace => ({
  id,
  photoId: 0,
  personId,
  name: null,
  bbox: [x, y, x + w, y + w * 1.5],
});
const ids = (rows: (PhotoFace | null)[][]) => rows.map((r) => r.map((f) => f?.id ?? 0));

describe("faces lined up across frames", () => {
  test("someone running moves a few face widths between frames: still one row", () => {
    expect(ids(matchFaces([[face(1, 0.3, 0.3)], [face(2, 0.42, 0.31)], [face(3, 0.5, 0.3)]]))).toEqual([[1, 2, 3]]);
  });

  test("two people side by side keep their own rows, the nearest pairs first", () => {
    const rows = matchFaces([
      [face(1, 0.3, 0.3), face(2, 0.4, 0.3)],
      [face(3, 0.35, 0.3), face(4, 0.45, 0.3)],
    ]);
    expect(ids(rows).sort()).toEqual([
      [1, 3],
      [2, 4],
    ]);
  });

  test("a face far from any of the last frame's is someone else; a face much larger too", () => {
    expect(ids(matchFaces([[face(1, 0.1, 0.3)], [face(2, 0.8, 0.3)]])).sort()).toEqual([
      [0, 2],
      [1, 0],
    ]);
    expect(ids(matchFaces([[face(1, 0.3, 0.3)], [face(2, 0.3, 0.3, 0.12)]]))).toEqual([
      [0, 2],
      [1, 0],
    ]);
  });

  test("people the catalog knows go by who they are, wherever they are", () => {
    const rows = matchFaces([
      [face(1, 0.1, 0.3, 0.04, 7), face(2, 0.6, 0.3, 0.04, 8)],
      [face(3, 0.6, 0.3, 0.04, 7), face(4, 0.1, 0.3, 0.04, 8)],
    ]);
    expect(ids(rows).sort()).toEqual([
      [1, 3],
      [2, 4],
    ]);
  });

  test("a frame without the face leaves a gap; the next frame finds it again", () => {
    expect(ids(matchFaces([[face(1, 0.3, 0.3)], [], [face(2, 0.33, 0.3)]]))).toEqual([[1, 0, 2]]);
  });
});
