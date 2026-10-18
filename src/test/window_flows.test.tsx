// What a user does in the window, against the fake backend: each step lands
// on the right view, asks the backend for the right thing and draws the
// right photos (and keeps doing so as the window gets faster).
import { emit } from "@tauri-apps/api/event";
import { act } from "react";
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

/** Someone in People, and a face in a check finding. */
const checkPerson = (id: number, name: string, cover: number) => ({
  id,
  name,
  faces: 1,
  photos: 1,
  cover,
  ignored: false,
  isNew: false,
  newPhotos: 0,
});
const checkFace = (faceId: number, photoId: number, similarity: number) => ({ faceId, photoId, similarity, byHand: false });

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

describe("turning photos", () => {
  const turnsAsked = (be: ReturnType<typeof fakeBackend>, cmd = "turn_photos") =>
    be.calls.filter((c) => c.cmd === cmd).map((c) => c.args);

  test("R turns the selected photos right, Shift+R left, and they're drawn again turned", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await click(tile(host, 201));
    await click(tile(host, 202), { ctrlKey: true });
    await key(scroller(host), "r");
    await wait(100);
    expect(turnsAsked(be)).toEqual([{ ids: [201, 202], by: 1 }]);
    // A new revision: the grid asks for the new thumbnails.
    expect(tile(host, 201).querySelector("img")!.src).toContain("/thumb/201?r=2");
    expect(tile(host, 203).querySelector("img")!.src).toContain("/thumb/203?r=1");
    expect(host.querySelector(".statusbar")?.textContent ?? "").toMatch(/Turned 2 photos a quarter turn right/);
    await key(scroller(host), "R", { shiftKey: true });
    await wait(100);
    expect(turnsAsked(be).at(-1)).toEqual({ ids: [201, 202], by: -1 });
    expect(be.turns.get(201)).toBe(0);
  });

  test("Turn in the toolbar turns the selected photos; on the photo in the viewer, that photo", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await click(tile(host, 201));
    await click(host.querySelector('.toolbar .turn-control [aria-label="Turn left"]')!);
    await wait(100);
    expect(turnsAsked(be)).toEqual([{ ids: [201], by: -1 }]);

    await doubleClick(tile(host, 202));
    await wait(50);
    // On the photo, not among the photo's details.
    expect(host.querySelector(".viewer-info .turn-control, .viewer-info [aria-label='Turn right']")).toBeNull();
    await click(host.querySelector('.viewer-stage .turn-control [aria-label="Turn right"]')!);
    await wait(100);
    expect(turnsAsked(be).at(-1)).toEqual({ ids: [202], by: 1 });
    expect(viewer(host)).not.toBeNull(); // a click on the control doesn't close the viewer
  });

  test("resting on Turn shows the selected photos turned that way before the click; Undo turns them back", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await click(tile(host, 201));
    const right = host.querySelector('.toolbar .turn-control [aria-label="Turn right"]')!;
    const img = (id: number) => tile(host, id).querySelector("img")!;
    // Landing on it (or a window opening under the pointer) shows nothing...
    await act(async () => {
      right.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: document.body, clientX: 5, clientY: 5 }));
    });
    await wait(400);
    expect(img(201).style.transform).toBe("");
    // ...moving onto it and resting there does.
    await act(async () => {
      right.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 7, clientY: 6 }));
    });
    await wait(100);
    expect(img(201).style.transform).toBe("");
    await wait(300);
    expect(img(201).style.transform).toContain("rotate(90deg)");
    expect(img(202).style.transform).toBe(""); // not selected
    await act(async () => {
      right.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body }));
    });
    expect(img(201).style.transform).toBe("");

    await click(right);
    await wait(100);
    expect(turnsAsked(be)).toEqual([{ ids: [201], by: 1 }]);
    await click(byText(host, ".statusbar button", "Undo"));
    await wait(100);
    expect(turnsAsked(be).at(-1)).toEqual({ ids: [201], by: -1 });
    expect(be.turns.get(201)).toBe(0);
  });

  test("Sideways in the viewer: each photo as Shoebox would turn it, Turn it or Leave it, then the next", async () => {
    const be = fakeBackend();
    be.sideways.set(102, 1);
    be.sideways.set(203, 3);
    const host = await renderApp();
    await click(sidebarItem(host, "Sideways"));
    await wait(100);
    await doubleClick(tile(host, 102));
    await wait(50);
    const label = () => host.querySelector(".viewer-preview")?.textContent ?? "";
    const answer = (text: string) => byText(host, ".turn-decision button", text);
    const film = () => [...host.querySelectorAll<HTMLImageElement>(".filmstrip .film img")].map((i) => i.style.transform);
    expect(label()).toBe("Shown as Shoebox would turn it: a quarter turn right. Not turned yet.");
    expect(film()).toEqual(["translate(-50%, -50%) rotate(90deg)", "translate(-50%, -50%) rotate(270deg)"]);
    await click(answer("Turn it right"));
    await wait(100);
    expect(turnsAsked(be, "sideways_accept")).toEqual([{ ids: [102] }]);
    // On to the next one.
    expect(host.querySelector(".viewer-info h2")!.textContent).toBe("IMG_203.jpg");
    expect(label()).toBe("Shown as Shoebox would turn it: a quarter turn left. Not turned yet.");
    await click(answer("Leave it as it is"));
    await wait(100);
    expect(turnsAsked(be, "sideways_leave")).toEqual([{ ids: [203] }]);
    expect(viewer(host)).toBeNull(); // that was the last one
  });

  test("everywhere else the viewer shows the photo as it is", async () => {
    const be = fakeBackend();
    be.sideways.set(102, 1);
    const host = await renderApp();
    await doubleClick(tile(host, 102));
    await wait(50);
    expect(host.querySelector(".viewer-preview")).toBeNull();
    expect(host.querySelector(".turn-decision")).toBeNull();
    expect(host.querySelector<HTMLElement>(".viewer-turn")!.style.transform).toBe("");
    // Its suggestion is offered in words, with the direction.
    expect(byText(host, ".viewer-info .turn-note .link", "Turn it right")).toBeTruthy();
  });

  test("Sideways shows each photo turned the way Shoebox suggests: leave the wrong ones, turn the rest", async () => {
    const be = fakeBackend();
    be.sideways.set(102, 1);
    be.sideways.set(203, 3);
    const host = await renderApp();
    await click(sidebarItem(host, "Sideways"));
    await wait(100);
    expect(title(host)).toBe("Sideways");
    expect(gridIds(host)).toEqual([102, 203]);
    const shown = (id: number) => tile(host, id).querySelector("img")!;
    expect(shown(102).classList).toContain("turned");
    expect(shown(102).style.transform).toContain("rotate(90deg)");
    expect(shown(203).style.transform).toContain("rotate(270deg)");
    // R doesn't turn here: the thumbnails show the suggestion, not the photo.
    await click(tile(host, 203));
    await key(scroller(host), "r");
    await wait(50);
    expect(turnsAsked(be)).toEqual([]);

    await click(byText(host, ".banner button", "Leave it as it is"));
    await wait(100);
    expect(turnsAsked(be, "sideways_leave")).toEqual([{ ids: [203] }]);
    expect(gridIds(host)).toEqual([102]);
    await click(byText(host, ".banner button", "Turn it as shown"));
    await wait(100);
    expect(turnsAsked(be, "sideways_accept")).toEqual([{ ids: [102] }]);
    expect(be.turns.get(102)).toBe(1);
    expect(gridIds(host)).toEqual([]);
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

describe("Cull", () => {
  /** The frame shown big in Cull, by its preview. */
  const cullPhoto = (host: HTMLElement) => Number(/preview\/(\d+)/.exec(host.querySelector<HTMLImageElement>(".cull-photo img")!.src)![1]);
  const marks = (be: ReturnType<typeof fakeBackend>, from: number) =>
    be.calls.slice(from).filter((c) => c.cmd === "mark_photos").map((c) => c.args);

  /** Opens Cull on a source. */
  const openCull = async (host: HTMLElement, source: string) => {
    await click(sidebarItem(host, source));
    await wait(100);
    await click(byText(host, ".toolbar button", "Cull"));
    await wait(100);
  };

  test("a folder as shots: Enter rejects what Cull suggests in a shot, X rejects a frame, P keeps it, Esc goes back", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await openCull(host, "Trip");
    expect(title(host)).toBe("Cull · Trip");
    // The burst (301, 302), then 303 and the blurry 304: three shots, from the burst's keeper.
    expect(host.querySelectorAll(".cull-shot")).toHaveLength(3);
    expect(cullPhoto(host)).toBe(302);
    expect(host.querySelector(".cull-suggest")!.textContent).toBe("Cull suggests: keep");

    let from = be.calls.length;
    await key(document.body, "Enter");
    await wait(50);
    expect(marks(be, from)).toEqual([{ ids: [301], marked: true }]);
    // On to the next shot. X rejects it (and moves on); P keeps the blurry one anyway.
    expect(cullPhoto(host)).toBe(303);
    from = be.calls.length;
    await key(document.body, "x");
    await wait(30);
    expect(cullPhoto(host)).toBe(304);
    expect(host.querySelector(".cull-says")!.textContent).toMatch(/Cull suggests: reject.*Blurry everywhere/);
    await key(document.body, "p");
    await wait(50);
    expect(marks(be, from)).toEqual([
      { ids: [303], marked: true },
      { ids: [304], marked: false },
    ]);
    expect(be.marked).toEqual(new Set([301, 303]));

    await key(document.body, "Escape");
    await wait(100);
    expect(title(host)).toBe("Trip");
    expect(tile(host, 301).classList).toContain("is-marked");
    // Kept is nothing: no mark, no word.
    expect(tile(host, 302).querySelector(".badge")).toBeNull();
    expect(tile(host, 302).title).toBe("");
  });

  test("Accept all suggestions rejects what Cull suggests rejecting, where not marked yet", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await openCull(host, "Fast");
    await key(document.body, "x"); // 202, the keeper, rejected by hand
    await wait(30);
    const from = be.calls.length;
    await click(byText(host, ".cull-head button", /Accept all suggestions/));
    await wait(50);
    expect(marks(be, from)).toEqual([{ ids: [201, 204], marked: true }]);
    expect(be.marked).toEqual(new Set([201, 202, 204]));
    expect(byText(host, ".cull-head button", /Accept all suggestions/).hasAttribute("disabled")).toBe(true);
  });

  test("the Cull button says what it will cull", async () => {
    fakeBackend();
    const host = await renderApp();
    await click(sidebarItem(host, "Trip"));
    await wait(100);
    expect(byText(host, ".toolbar button", "Cull").title).toMatch(/^Cull the 4 photos of this folder and its subfolders:/);
  });

  test("Z shows the frame at 100% from its original; the next frame keeps it; Esc goes back, then leaves", async () => {
    fakeBackend();
    const host = await renderApp();
    await openCull(host, "Trip");
    const full = () => host.querySelector<HTMLImageElement>(".cull-zoom-full");
    expect(full()).toBeNull();
    await key(document.body, "z");
    await wait(20);
    expect(full()!.src).toContain("/full/302?long=0");
    await key(document.body, "ArrowLeft"); // the burst's other frame, still at 100%
    await wait(20);
    expect(full()!.src).toContain("/full/301?long=0");
    await key(document.body, "Escape");
    await wait(20);
    expect(full()).toBeNull();
    expect(title(host)).toBe("Cull · Trip");
    await key(document.body, "Escape");
    await wait(100);
    expect(title(host)).toBe("Trip");
  });

  test("Ctrl or Shift click selects several frames or shots: X and P act on them all", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await openCull(host, "Trip");
    // The burst's other frame, with the one shown (302).
    await click(host.querySelectorAll(".cull-frame")[0], { ctrlKey: true });
    expect(host.querySelector(".cull-selected")!.textContent).toMatch(/^2 selected/);
    let from = be.calls.length;
    await key(document.body, "x");
    await wait(30);
    expect(marks(be, from)).toEqual([{ ids: [301, 302], marked: true }]);
    expect(host.querySelector(".cull-selected")).toBeNull();
    // The second and third shots.
    await click(host.querySelectorAll(".cull-shot")[1]);
    await click(host.querySelectorAll(".cull-shot")[2], { shiftKey: true });
    from = be.calls.length;
    await key(document.body, "x");
    await wait(30);
    expect(marks(be, from)).toEqual([{ ids: [303, 304], marked: true }]);
    // Then K takes them all back out.
    await click(host.querySelectorAll(".cull-shot")[1]);
    await click(host.querySelectorAll(".cull-shot")[2], { shiftKey: true });
    from = be.calls.length;
    await key(document.body, "k");
    await wait(30);
    expect(marks(be, from)).toEqual([{ ids: [303, 304], marked: false }]);
    expect(be.marked).toEqual(new Set([301, 302]));
  });

  test("originals Look closer couldn't read are named, with why", async () => {
    const be = fakeBackend();
    be.unreadableOriginals([{ id: 303, name: "IMG_303.CR3", reason: "IMG_303.CR3: the file couldn't be read" }]);
    const host = await renderApp();
    await openCull(host, "Trip");
    const warn = host.querySelector<HTMLElement>(".cull-unreadable")!;
    expect(warn.textContent!.trim()).toBe("1 original couldn't be read");
    expect(warn.title).toBe("IMG_303.CR3: IMG_303.CR3: the file couldn't be read");
  });

  test("photos measured in the background don't reload a shoot already measured", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await openCull(host, "Trip");
    const shoots = () => be.calls.filter((c) => c.cmd === "cull_shoot").length;
    const before = shoots();
    for (let i = 0; i < 5; i++) {
      await act(() => emit("ai-progress", { task: "quality", done: i, total: 10, gpu: false, finished: false, error: null }));
      await wait(500);
    }
    expect(shoots()).toBe(before);
  });

  test("what Cull found shows on thumbnails only when asked", async () => {
    fakeBackend();
    const host = await renderApp();
    const toggle = () => host.querySelector<HTMLElement>('.toolbar [aria-pressed][title*="what Cull found"]')!;
    expect(tile(host, 204).querySelector(".badge.problem")).toBeNull();
    await click(toggle());
    expect(toggle().getAttribute("aria-pressed")).toBe("true");
    expect(tile(host, 204).querySelector(".badge.problem")!.getAttribute("title")).toBe("Blurry");
    expect(tile(host, 203).querySelector(".badge.problem")).toBeNull();
    expect(localStorage.getItem("shoebox.showProblems")).toBe("1");
    localStorage.removeItem("shoebox.showProblems");
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
    be.addPerson(checkPerson(9, "Bea", 90), [103]);
    be.addPerson(checkPerson(10, "Bee", 91), [203]);
    be.peopleCheck({
      people: 3,
      faces: 40,
      left: 0,
      // Ana twice in photo 101: face 2 is someone else.
      twice: [{ personId: 7, photoId: 101, faces: [checkFace(1, 101, 0.9), checkFace(2, 101, 0.1)], elsewhere: [2], left: false }],
      ageJumps: [],
      likeOthers: [
        { personId: 7, face: checkFace(3, 201, 0.2), otherId: 9, other: 0.8, near: true, left: false },
        { personId: 7, face: checkFace(4, 301, 0.3), otherId: 10, other: 0.7, near: false, left: false },
      ],
      samePeople: [{ personId: 9, otherId: 10, similarity: 0.7, sameName: false, together: 0, left: false }],
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
    const action = (c: HTMLElement) => c.querySelector<HTMLButtonElement>(".check-card-foot button.primary")!;
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

  describe("merges, each with Undo in the status bar", () => {
    const someone = (id: number, name: string, cover: number) => ({
      id,
      name,
      faces: 1,
      photos: 1,
      cover,
      ignored: false,
      isNew: false,
      newPhotos: 0,
    });
    const names = (host: HTMLElement) => [...host.querySelectorAll(".person-name")].map((e) => e.textContent?.trim());
    const status = (host: HTMLElement) => host.querySelector(".statusbar .status-left")?.textContent;
    const undo = (host: HTMLElement) => byText(host, ".statusbar button", "Undo");
    const lastArgs = (be: ReturnType<typeof fakeBackend>, cmd: string) => be.calls.filter((c) => c.cmd === cmd).at(-1)!.args;

    test("Merge into one person: the named one stays; Undo makes them two again", async () => {
      const be = fakeBackend();
      const host = await renderApp();
      await click(sidebarItem(host, "People"));
      for (const tick of [...host.querySelectorAll(".person-face .tick")]) await click(tick);
      await click(byText(host, ".people-bar button", "Merge into one person"));
      await wait(100);
      expect(lastArgs(be, "merge_people")).toEqual({ ids: [7, 8], into: 7 });
      expect(names(host)).toEqual(["Ana"]);
      expect(status(host)).toBe("An unnamed person is now part of Ana.");

      await click(undo(host));
      await wait(100);
      expect(lastArgs(be, "unmerge_people")).toEqual({
        merged: { into: 7, name: "Ana", joined: [{ id: 8, name: null, ignored: false, createdAt: 0, faces: [2] }] },
      });
      expect(names(host)).toEqual(["Ana", "Add a name"]);
      expect(status(host)).toBe("Ana and an unnamed person are apart again.");
      expect(host.querySelector(".statusbar button")).toBeNull();
    });

    test("Same person while naming someone: Undo brings them back under their name", async () => {
      const be = fakeBackend();
      be.addPerson(someone(9, "Bea", 90), [103]);
      be.addPerson(someone(10, "Bee", 91), [203]);
      const host = await renderApp();
      await click(sidebarItem(host, "People"));
      await click(byText(host, ".person-name", "Bee"));
      await type(host.querySelector<HTMLInputElement>(".person-name-input")!, "Bea");
      await click(byText(host, ".name-suggest .same-person", "Same person"));
      await wait(100);
      expect(lastArgs(be, "merge_people")).toEqual({ ids: [10, 9], into: 9 });
      expect(names(host)).toEqual(["Ana", "Bea", "Add a name"]);
      expect(status(host)).toBe("Bee is now part of Bea.");

      await click(undo(host));
      await wait(100);
      expect(lastArgs(be, "unmerge_people")).toEqual({
        merged: { into: 9, name: "Bea", joined: [{ id: 10, name: "Bee", ignored: false, createdAt: 0, faces: [91] }] },
      });
      expect(names(host)).toEqual(["Ana", "Bea", "Bee", "Add a name"]);
      expect(status(host)).toBe("Bea and Bee are apart again.");
    });

    test("a name ending in a full stop ends the sentence once", async () => {
      fakeBackend().addPerson(someone(9, "Bea Jr.", 90), [103]);
      const host = await renderApp();
      await click(sidebarItem(host, "People"));
      await click(byText(host, ".person-name", "Add a name"));
      await type(host.querySelector<HTMLInputElement>(".person-name-input")!, "Bea");
      await click(byText(host, ".name-suggest .same-person", "Same person"));
      await wait(100);
      expect(status(host)).toBe("An unnamed person is now part of Bea Jr.");
    });

    /** People > Bee's page > her name box: "Same person" as Bea. */
    const mergeOnPage = async (be: ReturnType<typeof fakeBackend>, host: HTMLElement) => {
      await click(sidebarItem(host, "People"));
      await click(card(host, "Bee").querySelector(".person-face")!);
      await wait(100);
      expect(title(host)).toBe("Bee");
      expect(gridIds(host)).toEqual([203]);

      await click(host.querySelector(".title-name")!);
      await type(host.querySelector<HTMLInputElement>(".title-name-input")!, "Bea");
      await click(byText(host, ".name-suggest .same-person", "Same person"));
      await wait(100);
      expect(lastArgs(be, "merge_people")).toEqual({ ids: [10, 9], into: 9 });
      expect(title(host)).toBe("Bea");
      expect(gridIds(host)).toEqual([103, 203]);
      expect(status(host)).toBe("Bee is now part of Bea.");
    };
    const card = (host: HTMLElement, name: string) =>
      [...host.querySelectorAll(".person")].find((c) => c.querySelector(".person-name")?.textContent?.trim() === name)!;

    test("from a person's page: their photos join the other's page; Undo goes back to theirs", async () => {
      const be = fakeBackend();
      be.addPerson(someone(9, "Bea", 90), [103]);
      be.addPerson(someone(10, "Bee", 91), [203]);
      const host = await renderApp();
      await mergeOnPage(be, host);

      await click(undo(host));
      await wait(100);
      expect(lastArgs(be, "unmerge_people")).toEqual({
        merged: { into: 9, name: "Bea", joined: [{ id: 10, name: "Bee", ignored: false, createdAt: 0, faces: [91] }] },
      });
      expect(title(host)).toBe("Bee");
      expect(gridIds(host)).toEqual([203]);
      expect(status(host)).toBe("Bea and Bee are apart again.");

      // Back still goes to People, where they are two again.
      await click(backButton(host)!);
      expect(title(host)).toBe("People");
      expect(names(host)).toEqual(["Ana", "Bea", "Bee", "Add a name"]);
    });

    test("from a person's page, when someone new got their id meanwhile: Undo goes to People, where they are back", async () => {
      const be = fakeBackend();
      be.addPerson(someone(9, "Bea", 90), [103]);
      be.addPerson(someone(10, "Bee", 91), [203]);
      const host = await renderApp();
      await mergeOnPage(be, host);
      // Faces found in the background: someone new under Bee's old id.
      be.addPerson(someone(10, "Cy", 92), [303]);
      await act(async () => {
        await emit("catalog-changed", null);
      });
      await wait(600);

      await click(undo(host));
      await wait(100);
      expect(be.calls.filter((c) => c.cmd === "unmerge_people")).toHaveLength(1);
      expect(title(host)).toBe("People");
      expect(names(host)).toEqual(["Ana", "Bea", "Cy", "Bee", "Add a name"]);
      expect(status(host)).toBe("Bea and Bee are apart again.");

      // Under another id, with her own photos.
      await click(card(host, "Bee").querySelector(".person-face")!);
      await wait(100);
      expect(title(host)).toBe("Bee");
      expect(gridIds(host)).toEqual([203]);
    });
  });

  test("Check: Leave as is keeps a finding out of the next check; those left are shown on demand and brought back", async () => {
    const be = fakeBackend();
    be.addPerson(checkPerson(9, "Bea", 90), [103]);
    be.addPerson(checkPerson(10, "Bea", 91), [203]);
    be.peopleCheck({
      people: 3,
      faces: 40,
      left: 0,
      // Ana twice in photo 101: a reflection.
      twice: [{ personId: 7, photoId: 101, faces: [checkFace(1, 101, 0.9), checkFace(2, 101, 0.8)], elsewhere: [2], left: false }],
      ageJumps: [],
      // A face of Ana much like Bea, left as is before.
      likeOthers: [{ personId: 7, face: checkFace(3, 201, 0.2), otherId: 9, other: 0.8, near: true, left: true }],
      // Two people called Bea, in a photo together.
      samePeople: [{ personId: 9, otherId: 10, similarity: 0.6, sameName: true, together: 1, left: false }],
    });
    const host = await renderApp();
    await click(sidebarItem(host, "People"));
    await click(byText(host, ".people-bar button", "Check"));
    await wait(100);
    // One check, those left as is included: shown only when asked.
    expect(be.calls.filter((c) => c.cmd === "check_people").map((c) => c.args)).toEqual([{ all: true }]);
    const dialog = host.querySelector(".check-modal")!;
    const sections = () => [...dialog.querySelectorAll(".check-section h3")].map((h) => h.textContent);
    expect(sections()).toEqual(["Twice in one photo 1", "One person under two names? 1"]);
    const cards = () => [...dialog.querySelectorAll<HTMLElement>(".check-card")];
    const buttons = (c: HTMLElement) => [...c.querySelectorAll(".check-card-foot button")].map((b) => b.textContent);
    const foot = (c: HTMLElement) => c.querySelector(".check-card-foot")!.textContent;
    // Together in a photo, the two are not one: nothing to merge, only to leave as is.
    expect(cards().map(buttons)).toEqual([["Leave as is", "Move 1 face out"], ["Leave as is"]]);

    // Leave as is: kept by the catalog, People untouched, Undo on the card.
    const twice = [{ kind: "twice", personId: 7, photoId: 101, faces: [1, 2] }];
    let from = be.calls.length;
    await click(byText(cards()[0], "button", "Leave as is"));
    await wait(100);
    expect(be.calls.slice(from).find((c) => c.cmd === "leave_as_is")!.args).toEqual({ findings: twice, left: true });
    expect(be.cmdsSince(from)).not.toContain("people");
    expect(foot(cards()[0])).toBe("Left as is: Check leaves it out until it changes.Undo");
    from = be.calls.length;
    await click(byText(cards()[0], "button", "Undo"));
    await wait(100);
    expect(be.calls.slice(from).find((c) => c.cmd === "leave_as_is")!.args).toEqual({ findings: twice, left: false });
    expect(buttons(cards()[0])).toEqual(["Leave as is", "Move 1 face out"]);
    from = be.calls.length;
    await click(byText(cards()[1], "button", "Leave as is"));
    await wait(100);
    expect(be.calls.slice(from).find((c) => c.cmd === "leave_as_is")!.args).toEqual({
      findings: [{ kind: "samePerson", personId: 9, otherId: 10 }],
      left: true,
    });

    // Left before: listed on demand with nothing ticked; Bring back puts it in the check again.
    await click(byText(dialog, ".check-summary button", "Show 1 left as is"));
    expect(sections()).toEqual(["Twice in one photo 1", "One person under two names? 1", "Left as is 1"]);
    const left = cards()[2];
    expect([...left.querySelectorAll(".stray")].map((s) => s.classList.contains("on"))).toEqual([false]);
    expect(buttons(left)).toEqual(["Bring back", "Move 0 faces to Bea"]);
    from = be.calls.length;
    await click(byText(left, "button", "Bring back"));
    await wait(100);
    expect(be.calls.slice(from).find((c) => c.cmd === "leave_as_is")!.args).toEqual({
      findings: [{ kind: "likeOther", personId: 7, faceId: 3, otherId: 9 }],
      left: false,
    });
    expect(foot(cards()[2])).toBe("Back in the check.Undo");
    await click(byText(dialog, ".check-summary button", "Hide those left as is"));
    expect(sections()).toEqual(["Twice in one photo 1", "One person under two names? 1"]);

    // Checked again: the face is among the findings, the two people left as is.
    await click(byText(dialog, ".modal-head button", "Check again"));
    await wait(100);
    expect(sections()).toEqual(["Twice in one photo 1", "More like someone else 1"]);
    await click(byText(dialog, ".check-summary button", "Show 1 left as is"));
    expect(cards().map(buttons).at(-1)).toEqual(["Bring back"]);
  });

  test("Find more: the faces near their photos come ticked, and Move puts them in the person", async () => {
    const be = fakeBackend();
    const host = await renderApp();
    await click(sidebarItem(host, "People"));
    await click(host.querySelector(".person-face")!);
    await wait(100);
    expect(title(host)).toBe("Ana");

    await click(byText(host, ".tools button", "Find more…"));
    await wait(100);
    expect(be.calls.filter((c) => c.cmd === "more_faces").map((c) => c.args.personId)).toEqual([7]);
    const dialog = host.querySelector(".strays-modal")!;
    const days = [...dialog.querySelectorAll(".more-day-head")];
    expect(days.map((d) => d.getAttribute("aria-pressed"))).toEqual(["true", "false"]);
    expect(days[0].textContent).toMatch(/near Ana's photos/);
    expect([...dialog.querySelectorAll(".stray")].map((s) => s.classList.contains("on"))).toEqual([true, false]);
    const move = () => byText(dialog, ".modal-foot button", /^Move /);
    expect(move().textContent).toBe("Move 1 face to Ana");

    // A day's header ticks the whole day, and unticks it.
    await click(days[1]);
    expect(move().textContent).toBe("Move 2 faces to Ana");
    await click(days[1]);
    expect(move().textContent).toBe("Move 1 face to Ana");

    await click(move());
    await wait(150);
    expect(be.calls.filter((c) => c.cmd === "reassign_faces").map((c) => c.args)).toEqual([
      { faceIds: [1102], to: { kind: "person", id: 7 } },
    ]);
    expect(host.querySelector(".strays-modal")).toBeNull();
    expect(host.querySelector(".status-left")?.textContent).toBe("1 face moved to Ana.");
    expect(gridIds(host)).toEqual([101, 102, 201, 301]);
  });
});

describe("missing photos", () => {
  test("the Missing view says which are in the bin, and Remove takes them out of the catalog", async () => {
    const be = fakeBackend();
    for (const id of [101, 102, 103]) be.missing.add(id);
    be.inBin.add(101);
    be.inBin.add(102);
    const host = await renderApp();

    await click(sidebarItem(host, "Missing"));
    await wait(100);
    expect(gridIds(host)).toEqual([101, 102, 103]);
    const missingBanner = () => byText(host, ".main > .banner", /weren't where/);
    expect(missingBanner().textContent).toContain("2 of them are in the Trash: deleted, not moved");
    expect(tile(host, 101).querySelector(".badge.in-bin")).not.toBeNull();
    expect(tile(host, 103).querySelector(".badge.in-bin")).toBeNull();
    expect(tile(host, 103).querySelector(".badge.missing")).not.toBeNull();

    // Only those in the bin: asked first, then they leave the grid.
    await click(byText(missingBanner(), "button", /Remove the 2 in the Trash/));
    await wait(100);
    expect(be.calls.filter((c) => c.cmd === "plugin:dialog|message")).toHaveLength(1);
    expect(be.calls.filter((c) => c.cmd === "forget_missing").map((c) => c.args.ids)).toEqual([[101, 102]]);
    expect(gridIds(host)).toEqual([103]);
    expect(missingBanner().textContent).not.toContain("Trash");

    await click(byText(missingBanner(), "button", /Remove it from catalog/));
    await wait(100);
    expect(be.calls.filter((c) => c.cmd === "forget_missing").map((c) => c.args.ids)).toEqual([[101, 102], [103]]);
    expect(gridIds(host)).toEqual([]);
  });

  test("selected missing photos can be removed from the toolbar; photos still on a disk can't", async () => {
    const be = fakeBackend();
    be.missing.add(102);
    const host = await renderApp();
    const remove = () => [...host.querySelectorAll(".toolbar button")].find((b) => /from\s*catalog/.test(b.textContent ?? ""));

    await click(tile(host, 101));
    expect(remove()).toBeUndefined();
    await click(tile(host, 102), { ctrlKey: true });
    expect(remove()?.textContent?.trim()).toMatch(/^Remove from\s*catalog…$/);
    await click(remove()!);
    await wait(100);
    expect(be.calls.filter((c) => c.cmd === "forget_missing").map((c) => c.args.ids)).toEqual([[102]]);
    expect(gridIds(host)).not.toContain(102);
  });
});
