import path from "node:path";
import { execFileSync } from "node:child_process";
import { existsSync, lstatSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const root = path.resolve(import.meta.dirname, "../..");
export default defineConfig(async () => {
  const collect = process.env.UNIFIED_LAYOUT_COLLECT !== "0";
  const plugins = [react()];
  if (collect) {
    const helperUrl = pathToFileURL(
      path.join(root, "tests/browser/screenshot-source.mjs"),
    ).href;
    const { createScreenshotSource } = await import(helperUrl);
    plugins.push({
      name: "unified-screenshot-source-evidence",
      apply: "build",
      generateBundle() {
        const modules = [];
        for (const id of this.getModuleIds()) {
          const filename = id.split("?")[0];
          if (
            !path.isAbsolute(filename) ||
            !filename.startsWith(`${root}${path.sep}`)
          )
            continue;
          const relative = path
            .relative(root, filename)
            .split(path.sep)
            .join("/");
          if (
            relative.startsWith("node_modules/") ||
            !existsSync(filename) ||
            !lstatSync(filename).isFile()
          )
            continue;
          modules.push(relative);
        }
        const evidence = createScreenshotSource(root, modules, {
          strict: process.env.UNIFIED_LAYOUT_STRICT === "1",
        });
        this.emitFile({
          type: "asset",
          fileName: "unified-source-evidence.json",
          source: JSON.stringify(evidence, null, 2),
        });
      },
    });
  }
  return {
    root,
    plugins,
    define: {
      __UNIFIED_LAYOUT_CHECKOUT__: JSON.stringify(
        collect
          ? execFileSync("git", ["rev-parse", "HEAD"], {
              cwd: root,
              encoding: "utf8",
            }).trim()
          : "unrecorded",
      ),
    },
    resolve: { alias: { "@": path.resolve(root, "src") } },
    build: {
      outDir: process.env.ZCODE_LAYOUT_OUTPUT ?? "dist/zcode-layout",
      emptyOutDir: true,
      target: "esnext",
      rollupOptions: {
        input: [
          path.resolve(import.meta.dirname, "zcode-provider.html"),
          path.resolve(import.meta.dirname, "unified-accounts.html"),
          path.resolve(import.meta.dirname, "applications-c2.html"),
        ],
      },
    },
    server: { host: "127.0.0.1", port: 4314, strictPort: true },
  };
});
