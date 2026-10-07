import path from "node:path";
import { defineConfig } from "vite";
import tailwind from "tailwindcss";
import autoprefixer from "autoprefixer";
import theme from "../../tailwind.config.cjs";
import base from "./vite.config.mjs";

export default defineConfig({
  ...base,
  css: {
    postcss: {
      plugins: [
        tailwind({
          ...theme,
          content: [...theme.content, "./tests/browser/ui-foundation.tsx"],
        }),
        autoprefixer(),
      ],
    },
  },
  build: {
    ...base.build,
    outDir: process.env.UI_FOUNDATION_OUTPUT ?? "dist/ui-foundation",
    rollupOptions: {
      input: path.resolve(import.meta.dirname, "ui-foundation.html"),
    },
  },
});
