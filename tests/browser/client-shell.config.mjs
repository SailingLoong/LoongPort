import path from "node:path";
import { defineConfig } from "vite";
import base from "./vite.config.mjs";

export default defineConfig({
  ...base,
  build: {
    ...base.build,
    outDir: process.env.SHELL_LAYOUT_OUTPUT ?? "dist/client-shell",
    rollupOptions: {
      input: path.resolve(import.meta.dirname, "client-shell.html"),
    },
  },
});
