import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, vi } from "vitest";
import App from "../App";
import css from "../styles.css?raw";

const mounted: { root: Root; host: HTMLElement }[] = [];

/** Draws the whole window, with its stylesheet, and lets its first answers in. */
export async function renderApp() {
  const style = document.createElement("style");
  style.textContent = css;
  document.head.append(style);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  mounted.push({ root, host });
  await act(async () => root.render(createElement(App)));
  await wait(100);
  return host;
}

// Each test starts with no window. (Runs before setup.ts's afterEach: the
// clock is still fake and the backend still there while it is taken down.)
afterEach(async () => {
  for (const { root, host } of mounted.splice(0)) {
    await act(async () => root.unmount());
    host.remove();
  }
  document.head.querySelectorAll("style").forEach((s) => s.remove());
});

/**
 * Lets `ms` pass on the fake clock: answers arrive, timers fire, the window
 * draws. In steps of 10 ms at most, each its own act(), since React draws
 * (and starts what its effects start) only when an act() ends.
 */
export async function wait(ms: number) {
  await walk(ms, () => {}, 10);
}

/**
 * Lets `ms` pass in small steps, checking `check` after each one: for rules
 * about what the window shows on the way, not only at the end.
 */
export async function walk(ms: number, check: () => void, step = 5) {
  for (let t = 0; t < ms; t += step) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(Math.min(step, ms - t));
    });
    check();
  }
}

const one = (host: ParentNode, selector: string) => {
  const el = host.querySelector<HTMLElement>(selector);
  if (!el) throw new Error(`nothing matches ${selector}`);
  return el;
};

/** The first `selector` whose text is `text` (trimmed), or matches it. */
export function byText(host: ParentNode, selector: string, text: string | RegExp) {
  const all = [...host.querySelectorAll<HTMLElement>(selector)];
  const el = all.find((e) => (typeof text === "string" ? e.textContent?.trim() === text : text.test(e.textContent ?? "")));
  if (!el) throw new Error(`no ${selector} with text ${text}`);
  return el;
}

export async function click(el: Element, init: MouseEventInit = {}) {
  await act(async () => {
    el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, ...init }));
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, cancelable: true, ...init }));
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, ...init }));
  });
}

export async function doubleClick(el: Element) {
  await click(el);
  await act(async () => {
    el.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
  });
}

/** Drags `from` and drops it on `to`, as the browser does (jsdom has no DataTransfer: a plain one stands in). */
export async function dragAndDrop(from: Element, to: Element) {
  const data = new Map<string, string>();
  const dataTransfer = {
    get types() {
      return [...data.keys()];
    },
    setData: (k: string, v: string) => data.set(k, v),
    getData: (k: string) => data.get(k) ?? "",
    setDragImage: () => {},
    effectAllowed: "all",
    dropEffect: "none",
  };
  const fire = (el: Element, type: string) => {
    const e = new Event(type, { bubbles: true, cancelable: true });
    Object.defineProperty(e, "dataTransfer", { value: dataTransfer });
    el.dispatchEvent(e);
  };
  await act(async () => {
    fire(from, "dragstart");
    fire(to, "dragenter");
    fire(to, "dragover");
    fire(to, "drop");
    fire(from, "dragend");
  });
}

export async function key(el: Element, k: string, init: KeyboardEventInit = {}) {
  await act(async () => {
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));
  });
}

/** Types into a text box (React only sees a value set through the native setter). */
export async function type(input: HTMLInputElement, text: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

export async function choose(select: HTMLSelectElement, value: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!.call(select, value);
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

// What the window shows.

/** The photos in the grid, in order (by their thumbnails). */
export const gridIds = (host: ParentNode) =>
  [...host.querySelectorAll<HTMLImageElement>(".grid-scroller .tile img")].map((img) => Number(/thumb\/(\d+)/.exec(img.src)![1]));
export const tile = (host: ParentNode, id: number) =>
  [...host.querySelectorAll<HTMLElement>(".grid-scroller .tile")].find((t) => t.querySelector("img")?.src.includes(`/thumb/${id}?`))!;
export const gridStack = (host: ParentNode) => one(host, ".grid-stack");
export const title = (host: ParentNode) => one(host, ".toolbar .title h1").textContent!.trim();
/** The count next to the title ("12 photos", "Loading…"). */
export const subtitle = (host: ParentNode) => one(host, ".toolbar .title > .muted").textContent!.trim();
export const searchBox = (host: ParentNode) => one(host, ".toolbar .search input") as HTMLInputElement;
export const sortSelect = (host: ParentNode) => one(host, ".viewbar .sort-select") as HTMLSelectElement;
export const scroller = (host: ParentNode) => one(host, ".grid-scroller");
export const viewer = (host: ParentNode) => host.querySelector<HTMLElement>(".viewer");
export const backButton = (host: ParentNode) => host.querySelector<HTMLElement>(".toolbar .back-btn");
/** A row in the sidebar (All photos, People, a source), by the start of its text. */
export const sidebarItem = (host: ParentNode, name: string) => byText(host, ".sidebar-scroll .nav-item", new RegExp(`^${name}`));
