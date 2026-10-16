import { defineConfig, mergeConfig } from "vitest/config";
import viteConfig from "./vite.config.ts";

// The window's tests: React in jsdom, the backend replaced by a fake one
// (src/test/backend.ts). `npm test`.
export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      environment: "jsdom",
      setupFiles: ["src/test/setup.ts"],
      include: ["src/**/*.test.{ts,tsx}"],
      // The stylesheet is read for real: the grid's "not clickable" and "dimmed" are CSS.
      css: { include: [/styles\.css/] },
    },
  }),
);
