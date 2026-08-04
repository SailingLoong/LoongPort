import path from "node:path";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./tests/setupGlobals.ts", "./tests/setupTests.ts"],
    globals: true,
    // ⚠️ **必须限定到这两处，不能用默认的全仓扫**。
    //
    // 默认 `include` 是 `**/*.{test,spec}.?(c|m)[jt]s?(x)`，而默认 `exclude` 只挡
    // `node_modules/**` 本身 —— 挡不住 `.claude/worktrees/<名字>/src/**.test.tsx`。
    // 那些是 git worktree（开发时随手建的隔离工作区），每个都带一份完整仓副本 +
    // 自己的 `node_modules/react` ⇒ 同一个测试被收集两遍，且两份 React 实例混在
    // 一个进程里，症状是 `Invalid Hook Call`。**后果不是变慢，是整个
    // `vitest run` 不能当门禁用**（它报的红与本次改动无关）。
    //
    // 用白名单而不是 `exclude: [".claude/**"]`：白名单陈述的是「测试就在这两处」
    // 这个事实，对将来任何形式的仓内副本都成立；黑名单要为每一种新副本再补一条。
    include: [
      "src/**/*.{test,spec}.?(c|m)[jt]s?(x)",
      "tests/**/*.{test,spec}.?(c|m)[jt]s?(x)",
    ],
    coverage: {
      reporter: ["text", "lcov"],
    },
  },
});
