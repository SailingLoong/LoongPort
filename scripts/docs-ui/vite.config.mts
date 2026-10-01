import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "tailwindcss";
import autoprefixer from "autoprefixer";
import path from "node:path";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const source = path.resolve(process.env.LOONGPORT_UI_SOURCE ?? ".");
const config = require(path.join(source, "tailwind.config.cjs"));
export default defineConfig({
  root: import.meta.dirname,
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
