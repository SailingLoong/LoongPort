import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { resolve, join } from "node:path";

import {
  LAST_APP_STORAGE_KEY,
  LAST_VIEW_STORAGE_KEY,
} from "../../src/config/constants";

/**
 * 守「两个 localStorage key 只有一份定义」。
 *
 * ## 这道闸为什么存在：它守的那件事**已经坏过一次**
 *
 * 上游把 `cc-switch-last-app` 在两个文件里各写一份字面量（`App.tsx` 读、
 * `AppSwitcher.tsx` 写）。fork 改名时只改到 `App.tsx` ⇒ 写入端还是上游那个 key、
 * 读取端已是 `loongport-last-app` ⇒ 「记住上次用的 app」整段不工作，每次启动都回落
 * `claude`，**不报错、不崩、无日志**，直到审计才发现。
 *
 * 漏的原因值得记下来：`App.tsx` 里那个常量**只有读没有写**，改的人在那个文件里
 * 搜不到 `setItem`，自然以为改完了。所以「改名时小心一点」不是解法 ——
 * 要么把定义收成一份（已做），要么加闸（本文件）。两个都做了。
 *
 * ## 为什么扫全仓，而不是只查那两个文件
 *
 * 只查 `App.tsx` + `AppSwitcher.tsx` 的闸挡不住**第三个组件**重新写一份字面量 ——
 * 而那正是同一个 bug 的下一次发作。所以这里遍历整个 `src/`，唯一允许出现这两个
 * 字符串的地方是 `src/config/constants.ts`。
 *
 * 会红的改法（都是真实的退化路径）：
 * - 在任何组件里写回 `"loongport-last-app"` 字面量而不 import 常量；
 * - 把 key 改回上游的 `cc-switch-*`（那会让 fork 读到上游残留值）；
 * - 删掉常量定义、退回各写一份。
 */
const SRC = resolve(__dirname, "../../src");
const CONSTANTS_REL = "config/constants.ts";

/** `src/` 下所有 `.ts` / `.tsx`（跳过 `.d.ts`，那里面不会有逻辑）。 */
function sourceFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      sourceFiles(full, acc);
    } else if (/\.tsx?$/.test(entry) && !entry.endsWith(".d.ts")) {
      acc.push(full);
    }
  }
  return acc;
}

const files = sourceFiles(SRC).map((path) => ({
  rel: path.slice(SRC.length + 1),
  text: readFileSync(path, "utf-8"),
}));

describe("localStorage key 的单一来源", () => {
  it("两个 key 的字面量只出现在 config/constants.ts 里", () => {
    for (const key of [LAST_APP_STORAGE_KEY, LAST_VIEW_STORAGE_KEY]) {
      const offenders = files
        .filter((f) => f.rel !== CONSTANTS_REL && f.text.includes(`"${key}"`))
        .map((f) => f.rel);
      expect(
        offenders,
        `${key} 的字面量出现在 ${offenders.join(", ")} —— ` +
          `请 import @/config/constants 里的常量。各写一份就是上次那个 bug 的复发条件`,
      ).toEqual([]);
    }
  });

  it("App 读取与保存实际导航状态共用常量，每个 key 只有一个写入点", () => {
    const app = files.find((f) => f.rel === "App.tsx");
    const appSwitcher = files.find(
      (f) => f.rel === "components/AppSwitcher.tsx",
    );
    expect(app, "找不到 App.tsx").toBeDefined();
    expect(appSwitcher, "找不到 AppSwitcher.tsx").toBeDefined();
    expect(app!.text).toMatch(/from\s+"@\/config\/constants"/);

    for (const [key, state] of [
      ["LAST_APP_STORAGE_KEY", "activeApp"],
      ["LAST_VIEW_STORAGE_KEY", "currentView"],
    ]) {
      const writes = new RegExp(
        String.raw`localStorage\s*\.\s*setItem\(\s*${key}\b`,
        "g",
      );
      // 测试用例会预置恢复状态，不属于生产写点；其 key 字面量仍受上方全 src 扫描保护。
      const owners = files
        .filter(
          (file) => !/(^|\/)__tests__\/|\.(test|spec)\.tsx?$/.test(file.rel),
        )
        .flatMap((file) =>
          Array.from(file.text.matchAll(writes), () => file.rel),
        );
      // App 的实际状态 effect 是唯一写点；切换请求尚待草稿确认时不得提前写。
      // 同文件第二次写也算重复，不能只对文件名去重。
      expect(owners, `${key} 只能由 App 保存一次`).toEqual(["App.tsx"]);
      expect(app!.text).toMatch(
        new RegExp(String.raw`localStorage\s*\.\s*getItem\(\s*${key}\s*\)`),
      );
      expect(app!.text).toMatch(
        new RegExp(
          String.raw`localStorage\s*\.\s*setItem\(\s*${key}\s*,\s*${state}\s*,?\s*\)`,
        ),
      );
      expect(appSwitcher!.text).not.toMatch(writes);
    }
  });

  it("没有退回上游的 cc-switch 前缀", () => {
    // fork 的存储要与上游隔开：同一台机器上装过 cc-switch 时别读到它的值。
    for (const key of [LAST_APP_STORAGE_KEY, LAST_VIEW_STORAGE_KEY]) {
      expect(key.startsWith("loongport-"), `${key} 该带 loongport- 前缀`).toBe(
        true,
      );
    }
    const offenders = files
      .filter((f) => /"cc-switch-last-(app|view)"/.test(f.text))
      .map((f) => f.rel);
    expect(
      offenders,
      `${offenders.join(", ")} 里还有上游的 key 字面量`,
    ).toEqual([]);
  });
});
