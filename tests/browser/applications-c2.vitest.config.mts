import path from "node:path";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  root: path.resolve(import.meta.dirname, "../.."),
  plugins: [react()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "../../src") } },
  test: {
    environment: "jsdom",
    include: ["tests/browser/applications-c2.dom.tsx"],
    setupFiles: ["./tests/setupGlobals.ts"],
    testTimeout: 45000,
    maxWorkers: 1,
  },
});
