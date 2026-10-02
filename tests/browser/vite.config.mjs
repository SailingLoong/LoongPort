import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  root: path.resolve(import.meta.dirname, "../.."),
  plugins: [react()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "../../src") } },
  server: { host: "127.0.0.1", port: 4314, strictPort: true },
});
