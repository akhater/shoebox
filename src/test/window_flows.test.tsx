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

  test("Check: findings by kind, each fixed in one click through the usual moves and merges, each with Undo", async () => {
    const be = fakeBackend();
    const person = (id: number, name: string, cover: number) => ({
      id,
      name,
      faces: 1,
      photos: 1,
      cover,
      ignored: false,
      isNew: false,
      newPhotos: 0,
    });
    be.addPerson(person(9, "Bea", 90), [103]);
    be.addPerson(person(10, "Bee", 91), [203]);
    const face = (faceId: number, photoId: number, similarity: number) => ({ faceId, photoId, similarity, byHand: false });
    be.peopleCheck({
      people: 3,
      faces: 40,
      // Ana twice in photo 101: face 2 is someone else.
      twice: [{ personId: 7, photoId: 101, faces: [face(1, 101, 0.9), face(2, 101, 0.1)], elsewhere: [2] }],
      ageJumps: [],
      likeOthers: [
        { personId: 7, face: face(3, 201, 0.2), otherId: 9, other: 0.8, near: true },
        { personId: 7, face: face(4, 301, 0.3), otherId: 10, other: 0.7, near: false },
      ],
      samePeople: [{ personId: 9, otherId: 10, similarity: 0.7, sameName: false, together: 0 }],
    });
    const host = await renderApp();
    await click(sidebarItem(host, "People"));
    await click(byText(host, ".people-bar button", "Check"));
    await wait(100);
    expect(be.calls.filter((c) => c.cmd === "check_people")).toHaveLength(1);
    const dialog = host.querySelector(".check-modal")!;
    expect([...dialog.querySelectorAll(".check-section h3")].map((h) => h.textContent)).toEqual([
      "Twice in one photo 1",
      "More like someone else 1",
      "Unsure: more like someone else, far apart in time 1",
      "One person under two names? 1",
    ]);
    const cards = () => [...dialog.querySelectorAll<HTMLElement>(".check-card")];
    const ticks = (c: HTMLElement) => [...c.querySelectorAll(".stray")].map((s) => s.classList.contains("on"));
    const action = (c: HTMLElement) => c.querySelector<HTMLButtonElement>(".check-card-foot button")!;
    // Suggested faces come ticked; the unsure ones don't.
    expect(cards().map(ticks)).toEqual([[false, true], [true], [false], []]);
    expect(cards().map((c) => action(c).textContent)).toEqual([
      "Move 1 face out",
      "Move 1 face to Bea",
      "Move 0 faces to Bee",
      "Same person",
    ]);
    expect(action(cards()[2]).disabled).toBe(true);

    // Twice: the unlike face leaves for whoever it resembles; Undo puts it back.
    let from = be.calls.length;
    await click(action(cards()[0]));
    await wait(100);
    expect(be.calls.slice(from).find((c) => c.cmd === "reassign_faces")!.args).toEqual({
      faceIds: [2],
      to: { kind: "elsewhere" },
    });
    expect(be.cmdsSince(from)).toContain("people"); // People reloaded
    expect(cards()[0].querySelector(".check-card-foot")!.textContent).toBe("1 face moved.Undo");
    from = be.calls.length;
    await click(byText(cards()[0], "button", "Undo"));
    await wait(100);
    expect(be.calls.slice(from).find((c) => c.cmd === "restore_faces")!.args).toEqual({
      placements: [{ faceId: 2, personId: 7, byHand: false, seen: true }],
    });
    expect(action(cards()[0]).textContent).toBe("Move 1 face out");

    // Unticked, a face stays where it is; ticked, it moves to the one it looks like.
    await click(cards()[1].querySelector(".stray")!);
    expect(action(cards()[1]).textContent).toBe("Move 0 faces to Bea");
    await click(cards()[1].querySelector(".stray")!);
    from = be.calls.length;
    await click(action(cards()[1]));
    await wait(100);
    expect(be.calls.slice(from).find((c) => c.cmd === "reassign_faces")!.args).toEqual({
      faceIds: [3],
      to: { kind: "person", id: 9 },
    });

    // Same person: Bee joins Bea. What was about Bee waits for "Check again"; Undo brings her back.
    from = be.calls.length;
    await click(action(cards()[3]));
    await wait(100);
    expect(be.calls.slice(from).find((c) => c.cmd === "merge_people")!.args).toEqual({ ids: [9, 10], into: 9 });
    expect(cards()[3].querySelector(".check-card-foot")!.textContent).toBe("Now one person: Bea.Undo");
    expect(cards()[2].querySelector(".check-card-foot")!.textContent).toBe("Changed since the check: check again.");
    from = be.calls.length;
    await click(byText(cards()[3], "button", "Undo"));
    await wait(100);
    expect(be.calls.slice(from).find((c) => c.cmd === "unmerge_people")!.args).toEqual({
      merged: { into: 9, name: "Bea", joined: [{ id: 10, name: "Bee", ignored: false, createdAt: 0, faces: [91] }] },
    });
    expect(action(cards()[2]).textContent).toBe("Move 0 faces to Bee");
    expect(action(cards()[3]).textContent).toBe("Same person");

    // Check again asks once more; Esc closes.
    await click(byText(dialog, ".modal-head button", "Check again"));
    await wait(100);
    expect(be.calls.filter((c) => c.cmd === "check_people")).toHaveLength(2);
    await key(document.body, "Escape");
    expect(host.querySelector(".check-modal")).toBeNull();
  });
});
