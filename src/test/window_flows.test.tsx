// What a user does in the window, against the fake backend: each step lands
// on the right view, asks the backend for the right thing and draws the
// right photos (and keeps doing so as the window gets faster).
import { describe, expect, test } from "vitest";
import { ALL, fakeBackend, photosOf, SEARCH } from "./backend";
import {
  backButton,
  byText,
  choose,
  click,
  doubleClick,
  gridIds,
  gridStack,
  key,
  renderApp,
  scroller,
  searchBox,
  sidebarItem,
  sortSelect,
  subtitle,
  tile,
  title,
  type,
  viewer,
  wait,
} from "./ui";

const banner = (host: HTMLElement) =>
  [...host.querySelectorAll(".main > .banner")].map((b) => b.textContent?.trim() ?? "").find((t) => t.startsWith("Showing"));

describe("word search", () => {
  test("clear matches first, then the possible ones, and clearing it brings the photos back", async () => {
    const be = fakeBackend();
    const host = await renderApp();

    await type(searchBox(host), "beach");
    await wait(300); // still typing: nothing asked yet
    expect(be.calls.some((c) => c.cmd === "search_photos")).toBe(false);
    await wait(300);
    expect(be.calls.filter((c) => c.cmd === "search_photos").map((c) => c.args.query)).toEqual(["beach"]);
    expect(gridIds(host)).toEqual(SEARCH.ids.slice(0, SEARCH.clear));
    expect(subtitle(host)).toBe("2 matches");
    expect(banner(host)).toMatch(/^Showing the 2 clear matches\./);
    expect(sortSelect(host).disabled).toBe(true);

    await click(byText(host, ".banner button", "Show 3 more possible matches"));
    expect(gridIds(host)).toEqual(SEARCH.ids);
    expect(banner(host)).toMatch(/^Showing 2 clear and 3 possible matches, best first\./);
    await click(byText(host, ".banner button", "Clear matches only"));
    expect(gridIds(host)).toEqual(SEARCH.ids.slice(0, SEARCH.clear));

    await click(host.querySelector('[aria-label="Clear search"]')!);
    await wait(600);
    expect(gridIds(host)).toEqual(ALL);
    expect(subtitle(host)).toBe("12 photos");
    expect(banner(host)).toBeUndefined();
    expect(sortSelect(host).disabled).toBe(false);
  });
});

describe("sort order", () => {
  test("newest first asks the backend for that order and shows it", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await choose(sortSelect(host), "newest");
    await wait(100);
    expect(be.calls.filter((c) => c.cmd === "list_photos").at(-1)!.args.sort).toBe("newest");
    expect(gridIds(host)).toEqual([...ALL].reverse());
    await choose(sortSelect(host), "oldest");
    await wait(100);
    expect(gridIds(host)).toEqual(ALL);
  });
});

describe("marking for deletion", () => {
  test("Delete marks the selected photo at once, then the totals are reloaded; Delete again unmarks it", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    be.delay("mark_photos", 200);

    await click(tile(host, 201));
    expect(tile(host, 201).classList).toContain("is-selected");
    // (Selecting a photo also fetches its details for the status bar.)
    const changes = (from: number) =>
      be.calls.slice(from).filter((c) => c.cmd !== "get_photo").map((c) => [c.cmd, c.args]);
    let from = be.calls.length;
    await key(scroller(host), "Delete");
    expect(tile(host, 201).classList).toContain("is-marked"); // before the backend answers
    await wait(100);
    expect(changes(from).map(([c]) => c)).toEqual(["mark_photos"]); // totals wait for the change
    await wait(200);
    expect(changes(from)).toEqual([
      ["mark_photos", { ids: [201], marked: true }],
      ["get_overview", {}],
    ]);
    expect(be.marked).toEqual(new Set([201]));
    expect(sidebarItem(host, "Marked for deletion").textContent).toMatch(/1$/);

    from = be.calls.length;
    await key(scroller(host), "Delete");
    expect(tile(host, 201).classList).not.toContain("is-marked");
    await wait(300);
    expect(changes(from)).toEqual([
      ["mark_photos", { ids: [201], marked: false }],
      ["get_overview", {}],
    ]);
    expect(be.marked.size).toBe(0);
  });
});

describe("the viewer", () => {
  test("a double click opens a photo, Esc closes it and leaves it selected", async () => {
    fakeBackend();
    const host = await renderApp();
    await doubleClick(tile(host, 202));
    await wait(50);
    expect(viewer(host)).not.toBeNull();
    expect(viewer(host)!.querySelector("h2")!.textContent).toBe("IMG_202.jpg");

    await key(document.body, "Escape");
    expect(viewer(host)).toBeNull();
    expect(tile(host, 202).classList).toContain("is-selected");
    expect(title(host)).toBe("All photos"); // Esc closed the viewer, not the view
  });

  test("Folder shows the folder the photo is in, with it selected; Back returns to the photo", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await doubleClick(tile(host, 202));
    await wait(50);

    const from = be.calls.length;
    await click(byText(viewer(host)!, ".loc-actions button", "Folder"));
    await wait(100);
    expect(viewer(host)).toBeNull();
    expect(title(host)).toBe("Fast");
    expect(gridIds(host)).toEqual(photosOf(20));
    expect(tile(host, 202).classList).toContain("is-selected");
    expect(be.cmdsSince(from).filter((c) => c !== "get_photo").sort()).toEqual(["facets", "list_photos"]);
    expect(backButton(host)).not.toBeNull();

    await click(backButton(host)!);
    await wait(100);
    expect(title(host)).toBe("All photos");
    expect(gridIds(host)).toEqual(ALL);
    expect(viewer(host)!.querySelector("h2")!.textContent).toBe("IMG_202.jpg");
    expect(backButton(host)).toBeNull();
  });
});

describe("People", () => {
  test("a person's photos, then Back to People", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await click(sidebarItem(host, "People"));
    expect(title(host)).toBe("People");
    expect([...host.querySelectorAll(".person-name")].map((e) => e.textContent?.trim())).toEqual(["Ana", "Add a name"]);

    await click(host.querySelector(".person-face")!);
    expect(title(host)).toBe("Ana");
    await wait(100);
    expect(be.calls.filter((c) => c.cmd === "list_photos").at(-1)!.args.scope).toEqual({ kind: "person", id: 7 });
    expect(gridIds(host)).toEqual([101, 201, 301]);
    expect(gridStack(host).classList).not.toContain("stale");

    await click(backButton(host)!);
    expect(title(host)).toBe("People");
    expect(host.querySelector(".people-wrap")).not.toBeNull();

    // Esc is Back too.
    await click(host.querySelector(".person-face")!);
    await wait(100);
    await key(document.body, "Escape");
    expect(title(host)).toBe("People");
  });
});
