import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

/**
 * 闸：**官网直连行的「编辑配置 / 已手动维护 / 恢复默认」这套。**
 *
 * ## 为什么是读源码断言，而不是渲染查 DOM
 *
 * 照 `OperatorRowHoverActions.test.ts` 与上游 `ProviderCardLayout.test.ts` 的形状。
 * 这里要验的几条性质在 jsdom 里都测不出来：
 *
 * - **三态判据**（`=== true` 而不是 `??`）—— 渲染测试只能验 `true` 与 `false` 两种
 *   入参的结果，而缺陷恰恰在 `null`：写成 `account.userEdited ?? false` 时
 *   `null` 与 `false` 表现相同，测试照样绿，但把 `userEdited` 换成
 *   `!account.userEdited` 那种反向写法就会在 `null` 时误报「已手动维护」。
 * - **amber 那套 class 的优先级** —— Tailwind 的样式与 `:hover` jsdom 都不算。
 * - **两个恢复按钮互斥** —— 藏起来的按钮也在 DOM 里，渲染测试查不出「同时出现两个」。
 *
 * ## 它守的是什么
 *
 * 这套 UI 是从 `OperatorRow` 抄过来的（CLAUDE.md §一：两类行放一起看不出是两个人
 * 写的）。抄的时候最容易丢的正是上面那三条不显眼的约定，而丢了之后
 * 类型检查、prettier、vitest 全绿。
 */
const read = (...segs: string[]) =>
  fs.readFileSync(path.resolve(__dirname, "..", "..", ...segs), "utf8");

const VENDOR_ROW = read("src", "components", "operator", "VendorRow.tsx");
const OPERATOR_ROW = read("src", "components", "operator", "OperatorRow.tsx");
const EDIT_GUARD = read(
  "src",
  "components",
  "operator",
  "useTierEditGuard.tsx",
);
const SECTION = read("src", "components", "operator", "OperatorSection.tsx");
const VENDOR_API = read("src", "lib", "api", "vendor.ts");

describe("官网行的 userEdited 是三态", () => {
  it("判据必须是 `=== true`，不能是 `??` 或裸真值", () => {
    expect(VENDOR_ROW).toContain("account.userEdited === true");
    // 这两种写法都会让 `null`（判不了）被当成一个确定的答案。
    expect(VENDOR_ROW).not.toMatch(/account\.userEdited\s*\?\?/);
    expect(VENDOR_ROW).not.toMatch(/!account\.userEdited/);
  });

  it("与 `OperatorRow` 用同一个判据 —— 两类行的三态语义必须一致", () => {
    expect(OPERATOR_ROW).toContain("userEdited === true");
  });
});

describe("编辑与恢复的入口条件", () => {
  it("没 provision 过的行不给编辑入口（点了必然报错）", () => {
    // `keyReady` 与非空 `providerId` 两条都要 —— 前者说明有 sk，
    // 后者说明那六条 provider 记录的 id 派生出来了（没登录过时是空串）。
    expect(VENDOR_ROW).toContain(
      'account.keyReady && account.providerId !== ""',
    );
  });

  it("两个恢复按钮互斥：hover 版给没改过的，常驻版给改过的", () => {
    // 缺了任一个条件就会同时出现两个恢复按钮（都在 DOM 里，肉眼才看得出）。
    expect(VENDOR_ROW).toContain("canEditConfig && !userEdited");
    expect(VENDOR_ROW).toContain("canEditConfig && userEdited");
  });
});

describe("恢复默认只动当前这一个平台", () => {
  it("`vendorApi.resetTierConfig` 必须吃 appId", () => {
    // 不吃 appId 就只能一次恢复六条 —— 那会把用户在别的 tab 里的编辑一起冲掉。
    expect(VENDOR_API).toMatch(
      /resetTierConfig:\s*\(providerId:\s*string,\s*appId:\s*AppId\)/,
    );
  });

  it("`vendorApi.list` 也要吃 appId —— userEdited 是按平台算的", () => {
    expect(VENDOR_API).toMatch(/list:\s*\(appId:\s*AppId\)/);
  });
});

describe("编辑流程复用 useTierEditGuard", () => {
  it("官网行走的是同一个 hook，不是自己做的一套", () => {
    // 这条守 CLAUDE.md §一：编辑页复用上游 `EditProviderDialog`，
    // 事前警告复用同一个 guard。另做一套等于把那些坑再踩一遍
    // （尤其「保存失败必须 throw，否则弹窗照关、用户配置全丢」那条）。
    expect(SECTION).toContain("requestEdit({");
    expect(SECTION).toContain('kind: "vendor"');
  });

  it("guard 收窄成 EditableTier，不吃整个 TierInfo", () => {
    // 官网行不是档位，没有 TierInfo 那些字段（倍率 / 分组名 / websiteUrl）。
    // 若 guard 仍要求 TierInfo，接线时就得给 vendor 造一堆假字段。
    expect(EDIT_GUARD).toContain("export interface EditableTier");
    expect(EDIT_GUARD).toContain("useState<EditableTier | null>");
    // 断言的是**类型位置**上没有 TierInfo，而不是全文没有它 ——
    // 文档注释里对比两者是有意保留的说明。
    expect(EDIT_GUARD).not.toContain("useState<TierInfo");
    expect(EDIT_GUARD).not.toMatch(/\(tier:\s*TierInfo\)/);
    expect(EDIT_GUARD).not.toMatch(/^import type \{ TierInfo \}/m);
  });

  it("保存失败仍然 throw —— 不能只 toast", () => {
    // `EditProviderDialog` 是 `await onSubmit(); onOpenChange(false)`：
    // 吃掉错误会让它照常关窗，用户刚敲的一屏配置全丢。
    expect(EDIT_GUARD).toMatch(
      /toast\.error\(String\(e\)\);\s*\n\s*\/\/[^\n]*\n\s*throw e;/,
    );
  });
});

describe("官网行的编辑文案不能沿用档位那套", () => {
  it("警告与 hint 都走 `loongport.vendor.*`", () => {
    // 档位那两条写死了「档位」与「刷新分组」—— 官网行不是档位、没有分组
    // （用户按的是「获取密钥」），且一行对应六个平台。
    expect(EDIT_GUARD).toContain("loongport.vendor.editConfirmMessage");
    expect(VENDOR_ROW).toContain("loongport.vendor.userEditedHint");
    expect(VENDOR_ROW).not.toContain("loongport.tier.userEditedHint");
  });

  it("「恢复默认」那组文案**有意共用** —— 它本来就不提档位", () => {
    // `resetConfirmMessage` 说的是「会用默认配置覆盖「{{name}}」的全部编辑，
    // 密钥保留不变」，两类行逐字适用。为它再造一份 vendor 版才是重复。
    expect(SECTION).toContain("loongport.tier.resetConfirmMessage");
  });
});

describe("已手动维护的行用 amber，且优先级低于「当前在用」", () => {
  it("amber 的边框与底色都在", () => {
    expect(VENDOR_ROW).toContain("border-amber-500/50 bg-amber-500/5");
  });

  it("蓝色（当前在用）压过 amber", () => {
    // 三态优先级：当前在用 > 已手动维护 > 普通。「现在生效的是哪个」比
    // 「这个谁维护」更要紧 —— 用户扫列表首先要找到在用的那个。
    const blue = VENDOR_ROW.indexOf("border-blue-500/60");
    const amber = VENDOR_ROW.indexOf("border-amber-500/50");
    expect(blue).toBeGreaterThan(-1);
    expect(amber).toBeGreaterThan(-1);
    expect(blue).toBeLessThan(amber);
  });
});
