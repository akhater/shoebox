// How the window stays quick, checked against a fake backend whose answers
// take time: a view change asks only for that view's photos and counts, and
// only the latest answer counts; the grid keeps the last view's photos (not
// clickable, then dimmed) until the next one's arrive, never showing them as
// the new view's; background work reloads only what its event changed; the
// grid and People aren't drawn again for nothing.
import { emit } from "@tauri-apps/api/event";
import { act } from "react";
import { describe, expect, test, vi } from "vitest";
import type { AiProgress, Progress } from "../api";
import { ALL, fakeBackend, photosOf, ROOTS } from "./backend";
import { choose, click, gridIds, gridStack, renderApp, searchBox, sidebarItem, sortSelect, subtitle, title, type, wait, walk } from "./ui";

// How often the grid and People are drawn.
const renders = vi.hoisted(() => ({ grid: 0, people: 0 }));
vi.mock("../components/PhotoGrid", async (importOriginal) => {
  const { counted } = await import("./counted");
  const m = await importOriginal<typeof import("../components/PhotoGrid")>();
  return { ...m, PhotoGrid: counted(m.PhotoGrid, () => renders.grid++) };
});
vi.mock("../components/PeopleView", async (importOriginal) => {
  const { counted } = await import("./counted");
  const m = await importOriginal<typeof import("../components/PeopleView")>();
  return { ...m, PeopleView: counted(m.PeopleView, () => renders.people++) };
});

const FAST = { kind: "root", id: 20 } as const;
const SLOW = { kind: "root", id: 10 } as const;

/** The photos each title stands for. */
const photosTitled = (t: string) => (t === "All photos" ? ALL : photosOf(ROOTS.find((r) => r.name === t)?.id ?? -1));

/** Photos shown as current are always the titled view's; anything else is
 *  the last view's, kept (not clickable) until the new one is here. */
function honestGrid(host: HTMLElement) {
  if (!gridStack(host).classList.contains("stale")) expect(gridIds(host)).toEqual(photosTitled(title(host)));
}

const send = (event: string, payload: unknown) =>
  act(async () => {
    await emit(event, payload);
  });

describe("the window asks only for what changed", () => {
  test("a view change asks for that view's photos and counts, nothing else", async () => {
    const be = fakeBackend();
    const host = await renderApp();

    let from = be.calls.length;
    await click(sidebarItem(host, "Fast"));
    await wait(100);
    expect(gridIds(host)).toEqual(photosOf(20));
    expect(be.cmdsSince(from).sort()).toEqual(["facets", "list_photos"]);
    expect(be.calls.slice(from).every((c) => JSON.stringify(c.args.scope) === JSON.stringify(FAST))).toBe(true);

    from = be.calls.length;
    await choose(sortSelect(host), "newest");
    await wait(100);
    expect(be.cmdsSince(from).sort()).toEqual(["facets", "list_photos"]);

    // People, then a person: only that person's photos and counts.
    from = be.calls.length;
    await click(sidebarItem(host, "People"));
    await wait(100);
    await click(host.querySelector(".person-face")!);
    await wait(100);
    expect(title(host)).toBe("Ana");
    expect(be.cmdsSince(from).sort()).toEqual(["facets", "list_photos"]);

    from = be.calls.length;
    await type(searchBox(host), "beach");
    await wait(600);
    expect(be.cmdsSince(from).sort()).toEqual(["facets", "search_photos"]);
  });

  test("the photos show without waiting for their counts", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    be.slow(FAST, 2000, ["facets"]);
    await click(sidebarItem(host, "Fast"));
    await wait(100);
    expect(gridIds(host)).toEqual(photosOf(20));
    expect(gridStack(host).classList).not.toContain("stale");
    expect(subtitle(host)).toBe("4 photos");
  });

  test("a slow sidebar or People never holds the photos back", async () => {
    const be = fakeBackend();
    be.delay("browse", 5000);
    be.delay("people", 5000);
    be.delay("get_overview", 5000);
    const host = await renderApp();
    expect(gridIds(host)).toEqual(ALL);
    expect(gridStack(host).classList).not.toContain("stale");
  });

  test("only the latest answer counts: a slow view already left is dropped", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    be.slow(SLOW, 300);

    await click(sidebarItem(host, "Slow"));
    await wait(20);
    await click(sidebarItem(host, "Fast"));
    // The slow answer arrives at ~310 ms, long after the fast one.
    await walk(800, () => {
      honestGrid(host);
      expect(gridIds(host)).not.toEqual(photosOf(10));
    });
    expect(title(host)).toBe("Fast");
    expect(gridIds(host)).toEqual(photosOf(20));
    expect(gridStack(host).classList).not.toContain("stale");
  });
});

describe("the grid never shows one view's photos under another's title", () => {
  test("the last photos stay, not clickable, then dimmed with Loading… after 150 ms", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    be.slow(SLOW, 1000);
    const area = () => host.querySelector(".grid-stack .grid-area")!;

    await click(sidebarItem(host, "Slow"));
    expect(title(host)).toBe("Slow");
    expect(gridIds(host)).toEqual(ALL);
    expect(gridStack(host).classList).toContain("stale");
    expect(getComputedStyle(area()).pointerEvents).toBe("none");

    // A quick view just takes its place: no dimming, no "Loading…" before 150 ms.
    await walk(140, () => {
      honestGrid(host);
      expect(gridStack(host).classList).toContain("stale");
      expect(gridStack(host).classList).not.toContain("slow");
      expect(subtitle(host)).not.toMatch(/Loading/);
    });
    await wait(20);
    expect(gridStack(host).classList).toContain("slow");
    expect(getComputedStyle(area()).opacity).toBe("0.35");
    expect(subtitle(host)).toMatch(/^Loading…/);
    expect(gridIds(host)).toEqual(ALL);

    await walk(1000, () => honestGrid(host), 10);
    expect(gridIds(host)).toEqual(photosOf(10));
    expect(gridStack(host).classList).not.toContain("stale");
    expect(gridStack(host).classList).not.toContain("slow");
    expect(getComputedStyle(area()).pointerEvents).not.toBe("none");
    expect(subtitle(host)).toBe("4 photos");
  });

  test("going back and forth between views never shows the wrong photos", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    be.slow(SLOW, 200);
    be.slow({ kind: "root", id: 30 }, 90);
    for (const name of ["Slow", "Fast", "Trip", "All photos", "Slow", "Trip", "Fast"]) {
      await click(sidebarItem(host, name));
      await walk(60, () => honestGrid(host));
    }
    await walk(400, () => honestGrid(host));
    expect(title(host)).toBe("Fast");
    expect(gridIds(host)).toEqual(photosOf(20));
  });
});

describe("background work refreshes only what its event changed", () => {
  const faces = (p: Partial<AiProgress> = {}): AiProgress => ({
    task: "faces",
    done: 5,
    total: 10,
    gpu: false,
    finished: false,
    error: null,
    ...p,
  });

  test("face finding reloads People and the shown view, not the sidebar, at most every 3 s", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    // On Ana's page, while faces are found: she turns up in another photo,
    // and someone new appears in People.
    await click(sidebarItem(host, "People"));
    await click(host.querySelector(".person-face")!);
    await wait(100);
    expect(gridIds(host)).toEqual([101, 201, 301]);
    be.addPhotosOf(7, [104]);
    be.addPerson({ id: 9, name: "Ben", faces: 2, photos: 2, cover: 3, ignored: false, isNew: false, newPhotos: 0 }, [103, 203]);

    let from = be.calls.length;
    await send("ai-progress", faces());
    await wait(100);
    expect(be.cmdsSince(from).sort()).toEqual(["facets", "list_photos", "people"]);
    expect(gridIds(host)).toEqual([101, 104, 201, 301]);

    // Throttled: more progress within 3 s asks for nothing.
    from = be.calls.length;
    await send("ai-progress", faces({ done: 6 }));
    await wait(1000);
    expect(be.cmdsSince(from)).toEqual([]);
    await wait(2500);
    await send("ai-progress", faces({ done: 7 }));
    await wait(100);
    expect(be.cmdsSince(from).sort()).toEqual(["facets", "list_photos", "people"]);

    // Back on People, Ben is there.
    await click(host.querySelector(".toolbar .back-btn")!);
    await wait(100);
    expect([...host.querySelectorAll(".person-name")].map((e) => e.textContent?.trim())).toContain("Ben");

    // Once faces are all found, everything is reloaded (once, after a pause).
    from = be.calls.length;
    await send("ai-progress", faces({ done: 10, finished: true }));
    await send("catalog-changed", null);
    await wait(600);
    expect(be.cmdsSince(from).sort()).toEqual(["browse", "facets", "get_overview", "list_photos", "people"]);
  });

  test("a scan's refreshes never pile up: the next one waits for the last to answer", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await wait(100);
    // A long scan on a big catalog: it changes every half second, and the
    // photos take 2 s to come back.
    be.delay("list_photos", 2000);
    const from = be.calls.length;
    for (let i = 0; i < 5; i++) {
      await send("catalog-changed", null);
      await wait(500);
    }
    await wait(5000);
    // One at once, and one after it for everything that changed meanwhile.
    expect(be.cmdsSince(from).filter((c) => c === "list_photos")).toHaveLength(2);
    expect(gridIds(host)).toEqual(ALL);
  });

  test("the grid and People aren't drawn again for a scan's progress or a letter typed", async () => {
    fakeBackend();
    const host = await renderApp();
    const progress: Progress = { rootId: 10, rootName: "Slow", phase: "reading", found: 10, toRead: 10, read: 3, current: "x" };

    // The count works: another view draws the grid again.
    let before = renders.grid;
    await click(sidebarItem(host, "Fast"));
    await wait(100);
    expect(renders.grid).toBeGreaterThan(before);

    before = renders.grid;
    await send("scan-progress", progress);
    await wait(50);
    await type(searchBox(host), "b"); // the search waits for a pause in typing
    await wait(100);
    expect(subtitle(host)).toBe("4 photos");
    expect(renders.grid - before).toBe(0);

    await type(searchBox(host), "");
    await wait(600);
    before = renders.people;
    await click(sidebarItem(host, "People"));
    await wait(100);
    expect(renders.people).toBeGreaterThan(before);
    before = renders.people;
    await send("scan-progress", { ...progress, read: 4 });
    await wait(50);
    expect(renders.people - before).toBe(0);
  });
});

describe("big catalogs stay smooth", () => {
  test("People draws its faces a few hundred at a time, more as they're scrolled to", async () => {
    // What the browser says is near the screen, told by the test.
    const watched: { see: () => void; done: boolean }[] = [];
    class Near {
      private entry = { done: false, see: () => {} };
      constructor(callback: IntersectionObserverCallback) {
        this.entry.see = () => callback([{ isIntersecting: true } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
        watched.push(this.entry);
      }
      observe() {}
      unobserve() {}
      takeRecords() {
        return [];
      }
      disconnect() {
        this.entry.done = true;
      }
    }
    vi.stubGlobal("IntersectionObserver", Near);
    try {
      const be = fakeBackend();
      for (let i = 0; i < 600; i++) {
        be.addPerson({ id: 1000 + i, name: `Person ${i}`, faces: 2, photos: 2, cover: 1, ignored: false, isNew: false, newPhotos: 0 }, [101, 102]);
      }
      const host = await renderApp();
      await click(sidebarItem(host, "People"));
      await wait(100);
      // 601 named people and 1 to name: the first 240 of each group are drawn.
      expect(host.querySelectorAll(".person")).toHaveLength(241);
      // Scrolled near the end of what is drawn: 240 more.
      await act(async () => watched.filter((w) => !w.done).forEach((w) => w.see()));
      expect(host.querySelectorAll(".person")).toHaveLength(481);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  test("the thumbnail size slider still sizes the photos", async () => {
    fakeBackend();
    const host = await renderApp();
    await wait(100);
    const height = () => parseFloat((host.querySelector(".tile") as HTMLElement).style.height);
    const slider = host.querySelector('.toolbar input[type="range"]') as HTMLInputElement;
    const before = height();
    await type(slider, "320");
    await wait(50);
    expect(height()).toBeGreaterThan(before);
  });
});
