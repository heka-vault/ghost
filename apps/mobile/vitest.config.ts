import { defineConfig } from "vitest/config";

// Unit tests for the app's plain TypeScript (the stream's views, signing). Screens and native
// code are not loaded here; the Kotlin side has its own tests (modules/ghost-call/android).
export default defineConfig({
  // esbuild can't follow `extends: "expo/tsconfig.base"`; nothing here needs it.
  esbuild: { tsconfigRaw: "{}" },
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
