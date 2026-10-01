import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "tailwindcss";
import autoprefixer from "autoprefixer";
import path from "node:path";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";

const require = createRequire(import.meta.url);
const source = path.resolve(process.env.LOONGPORT_UI_SOURCE ?? ".");
const version = JSON.parse(
  readFileSync(path.join(source, "package.json"), "utf8"),
).version;
if (
  process.env.LOONGPORT_SOURCE_VERSION &&
  version !== process.env.LOONGPORT_SOURCE_VERSION
)
  throw new Error("Pinned screenshot version does not match source");
const config = require(path.join(source, "tailwind.config.cjs"));
export default defineConfig({
  root: import.meta.dirname,
  define: { "import.meta.env.DOCS_UI_VERSION": JSON.stringify(version) },
  base: "./",
  plugins: [react()],
  resolve: {
    alias: { "@": path.join(source, "src") },
    dedupe: ["react", "react-dom"],
  },
  css: {
    postcss: {
      plugins: [
        tailwind({
          ...config,
          content: [path.join(source, "src/**/*.{js,ts,jsx,tsx}")],
        }),
        autoprefixer(),
      ],
    },
  },
  build: {
    outDir: process.env.DOCS_UI_OUTPUT ?? ".output",
    emptyOutDir: true,
    assetsInlineLimit: 10000000,
  },
});
