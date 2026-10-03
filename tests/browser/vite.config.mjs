import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  root: path.resolve(import.meta.dirname, "../.."),
  plugins: [react()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "../../src") } },
  build: {
    outDir: process.env.ZCODE_LAYOUT_OUTPUT ?? "dist/zcode-layout",
    emptyOutDir: true,
    target: "esnext",
    rollupOptions: {
      input: path.resolve(import.meta.dirname, "zcode-provider.html"),
    },
  },
  server: { host: "127.0.0.1", port: 4314, strictPort: true },
});
