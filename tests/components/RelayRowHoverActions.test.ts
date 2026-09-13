import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

/**
 * 钉住「主按钮常驻、次要图标 hover 才显形」这套布局约定。
 *
 * ## 为什么是读源码断言 class，而不是渲染后查 DOM
 *
 * 照 `ProviderCardLayout.test.ts` 的形状（上游自己就用这个模式钉 `ProviderCard`
 * 的布局）。要验的性质是**纯 CSS 的**：`opacity-0` + `group-hover:opacity-100`
 * 在 jsdom 里查不出效果 —— jsdom 不算 Tailwind 的样式，也不模拟 `:hover`。
 * 渲染测试只能断言「按钮在 DOM 里」，而它本来就在（藏起来的按钮也在 DOM 里），
 * 于是这类缺陷渲染测试**照样绿**。
 *
 * ## 它守的是什么缺陷
 *
 * - 2026-08-03 一度把主按钮留在 hover 容器**外**常驻、与上游整组 hover 的形态
 *   分叉；随后对齐成「整组（含主按钮）hover 才显形」。
 * - 2026-09-13 用户定调反转：**行的主操作不靠鼠标扫出来** —— 主按钮（启用/使用中）
 *   常驻在 hover 组外，只有次要图标（检测/验真/编辑/恢复）留在组里；
 *   `ProviderActions`（ProviderCard）、`VendorRow` 同批改成同一形状。
 *   本闸现在钉的就是这个新契约：主按钮进组（回退旧形态）或图标组裸奔都会红。
 */
const RELAY_ROW_TSX = path.resolve(
  __dirname,
  "..",
  "..",
  "src",
  "components",
  "relay",
  "RelayRow.tsx",
);
const PROVIDER_CARD_TSX = path.resolve(
  __dirname,
  "..",
  "..",
  "src",
  "components",
  "providers",
  "ProviderCard.tsx",
);
const PROVIDER_ACTIONS_TSX = path.resolve(
  __dirname,
  "..",
  "..",
  "src",
  "components",
  "providers",
  "ProviderActions.tsx",
);

const source = fs.readFileSync(RELAY_ROW_TSX, "utf8");

describe("RelayRow hover-reveal actions", () => {
  it("keeps pointer-events in lockstep with opacity", () => {
    // ⚠️ 只改 opacity 的话透明按钮**仍然可点** —— 鼠标扫过看似空白的地方会误触删除。
    // 那串 class 里 `pointer-events-none` 与 `opacity-0` 总是成对出现。
    for (const constant of ["ROW_HOVER_ACTIONS", "TIER_HOVER_ACTIONS"]) {
      const match = source.match(
        new RegExp(`const ${constant} =\\s*\\n?\\s*"([^"]+)"`),
      );
      expect(match, `${constant} 不见了？`).toBeTruthy();
      const value = match![1];
      expect(value, `${constant} 少了 opacity-0`).toContain("opacity-0");
      expect(value, `${constant} 少了 pointer-events-none`).toContain(
        "pointer-events-none",
      );
      // 显形时两样都要恢复，否则按钮看得见点不动。
      expect(value).toMatch(/group-hover\/\w+:opacity-100/);
      expect(value).toMatch(/group-hover\/\w+:pointer-events-auto/);
      // 键盘可达：Tab 进去也要显形（上游同样带 focus-within）。
      expect(value).toMatch(/group-focus-within\/\w+:opacity-100/);
    }
  });

  it("uses named groups so the outer row does not reveal every nested tier", () => {
    // 档位行**嵌在**中转站行里。裸 `group-hover:` 编译成 `.group:hover &`，
    // 会匹配任意带 `group` 的祖先 ⇒ 鼠标停在中转站行上就把里面所有档位行点亮。
    expect(source).toContain("group/row");
    expect(source).toContain("group/tier");
    // 两层各自只认自己那一层。
    expect(source).toMatch(/ROW_HOVER_ACTIONS[\s\S]{0,400}?group-hover\/row:/);
    expect(source).toMatch(
      /TIER_HOVER_ACTIONS[\s\S]{0,400}?group-hover\/tier:/,
    );

    // ⚠️ 裸 `group` / `group-hover:`（不带 `/name`）会让两层串起来。
    //
    // 只扫**字符串字面量**里的 class，不扫注释 —— 本文件的注释正是在讲
    // 「别用裸的」，连它一起禁会让这条闸对着自己的说明文字报错（初版踩过）。
    const classLiterals = source.match(/"[^"\n]*"/g) ?? [];
    for (const literal of classLiterals) {
      expect(
        literal,
        "裸 group- 前缀会让外层 hover 点亮内层所有档位行",
      ).not.toMatch(/\bgroup-(hover|focus-within):/);
      // 裸的 `group` 作为独立 token（`group/row` 带斜杠是允许的）。
      expect(literal, "裸 group 类会被两层同时匹配").not.toMatch(
        /(^"|\s)group(\s|"$)/,
      );
    }
  });

  it("keeps the main enable button outside the hover group (always visible)", () => {
    // ⭐ 主按钮（启用/使用中）**常驻**在 hover 组外 —— 行的主操作不靠 hover 扫出来
    // （2026-09-13 定调；旧闸恰好断言相反的旧形态，已随形态一起反转）。
    const tierItem = source.slice(source.indexOf("function TierItem"));

    // ⚠️ **断言的是 JSX 的嵌套关系，不是字符串先后顺序。**
    //
    // 做法：从 `TIER_HOVER_ACTIONS` 所在的 `<div>` 起，按 `<div`/`</div>` 数深度找到
    // **配对**的收标签，截出真正的子树（不能图省事用第一个 `</div>` —— 容器里有
    // 三元，里面还有 div）。
    //
    // ⚠️ 用 **lastIndexOf**：`indexOf` 会先命中行根 div 注释里的常量名
    // （「见 `TIER_HOVER_ACTIONS` 的说明」），截出来的「子树」是整行，断言失真。
    // TierItem 切片内最后一次出现才是图标组 className 三元里的真用点。
    const anchor = tierItem.lastIndexOf("TIER_HOVER_ACTIONS");
    expect(anchor, "档位行没有 hover 组？").toBeGreaterThan(0);
    const divStart = tierItem.lastIndexOf("<div", anchor);
    const subtree = (() => {
      let depth = 0;
      const tag = /<div\b|<\/div>/g;
      tag.lastIndex = divStart;
      for (let m = tag.exec(tierItem); m; m = tag.exec(tierItem)) {
        depth += m[0] === "</div>" ? -1 : 1;
        if (depth === 0) return tierItem.slice(divStart, m.index + m[0].length);
      }
      throw new Error("hover 容器的 <div> 没有配对的收标签？");
    })();

    // 主按钮的两种文案都不许进 hover 组 —— 进去就回退成「没 hover 时右侧全空」。
    for (const key of ["provider.enable", "provider.inUse"]) {
      expect(
        subtree,
        `${key} 跑进 hover 组了 ⇒ 主按钮会整组藏起来，回到旧形态`,
      ).not.toContain(key);
    }
    // 组里住的是次要图标，且 className 的三元真的在用 TIER_HOVER_ACTIONS
    // （断言表达式形状而非名字 —— 名字也出现在注释里，名字断言是假闸）。
    expect(subtree).toContain('t("loongport.tier.checkConnectivity")');
    expect(subtree, "hover 容器的 className 没在用 TIER_HOVER_ACTIONS").toMatch(
      /\?\s*HOVER_ACTIONS_PINNED\s*\n?\s*:\s*TIER_HOVER_ACTIONS/,
    );
    // 主按钮渲染在组**之前**（同一个外层动作容器里），不能只是被删掉。
    expect(tierItem.slice(0, divStart)).toContain('t("provider.enable")');
    expect(tierItem.slice(0, divStart)).toContain('t("provider.inUse")');
  });

  it("pins the group visible while an icon action is running", () => {
    // 操作进行中鼠标一移开就看不到自己点的东西还在跑 —— 图标侧的 busy 都要算
    // （主按钮常驻，它自己的 switching 转圈天然可见，不靠钉）。
    expect(source).toContain("HOVER_ACTIONS_PINNED");
    expect(source).toMatch(
      /checking \|\| resetting \|\| verifying\s*\n?\s*\?\s*HOVER_ACTIONS_PINNED/,
    );
  });

  it("keeps ProviderActions on the same shape: primary outside, icons hover-gated", () => {
    // 跨文件的同一事实要有闸（CLAUDE.md §三点六）：「主按钮常驻、图标组 hover」
    // 这条分界线在 RelayRow（本文件上面的用例）与 ProviderActions（ProviderCard
    // 的动作区）两处各写了一份 —— 哪边回退成整组 hover 都会悄悄分叉。
    const actions = fs.readFileSync(PROVIDER_ACTIONS_TSX, "utf8");
    const gate = actions.indexOf("pointer-events-none opacity-0");
    const main = actions.indexOf("buttonState.text");
    expect(gate, "ProviderActions 没有图标 hover 组？").toBeGreaterThan(0);
    expect(main, "ProviderActions 的主按钮渲染没了？").toBeGreaterThan(0);
    // 主按钮先于 hover 闸出现 ⇒ 在组外常驻；反过来就是整组被包住了。
    expect(main).toBeLessThan(gate);

    // ProviderCard 那层不许再把整个动作区包回 hover 容器（旧形态的出处）。
    const card = fs.readFileSync(PROVIDER_CARD_TSX, "utf8");
    expect(
      card,
      "ProviderCard 又把整个动作区（含主按钮）包进 hover 容器了",
    ).not.toContain("opacity-0 pointer-events-none group-hover:opacity-100");
  });
});
