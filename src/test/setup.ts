import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, vi } from "vitest";

// React's act() is how the tests wait for the window to draw.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// jsdom has no layout: every box is 1240 x 800, so the grid (20 px padding on
// each side) lays its photos out on 1200 px and draws the first screens of them.
Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 1240 });
Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 800 });
// Nor ResizeObserver or scrolling into view; the sizes above never change.
class StillObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver = StillObserver as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView = () => {};

beforeEach(() => {
  // Answers are delayed on a fake clock: a test says how much time passes.
  vi.useFakeTimers();
});

// (The window drawn by a test is taken down by ui.ts, which tests import:
// importing the window here would load it before a test can mock a part of it.)
afterEach(() => {
  clearMocks();
  vi.useRealTimers();
  try {
    localStorage.clear();
  } catch {
    /* nothing remembered */
  }
});
