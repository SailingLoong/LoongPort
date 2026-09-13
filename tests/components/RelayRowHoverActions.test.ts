import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

/**
 * 钉住「动作按钮 hover 才显形」这套布局约定。
 *
 * ## 为什么是读源码断言 class，而不是渲染后查 DOM
 *
 * 照 `ProviderCardLayout.test.ts` 的形状（上游自己就用这个模式钉 `ProviderCard`
 * 的布局）。要验的性质是**纯 CSS 的**：`opacity-0` + `group-hover:opacity-100`
 * 在 jsdom 里查不出效果 —— jsdom 不算 Tailwind 的样式，也不模拟 `:hover`。
 * 渲染测试只能断言「按钮在 DOM 里」，而它本来就在（藏起来的按钮也在 DOM 里），
 * 于是这次的缺陷渲染测试**照样绿**。
 *
 * ## 它守的是什么契约
 *
 * 「信息常驻、动作 hover」——2026-09-13 用户定调（档位多时每行常驻按钮全是
 * 噪音）。#400 曾把主按钮提出组外常驻、当天收回；本闸钉住**整组（含主按钮）
 * hover 才显形**，谁再提出去都会红。
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
    // 上游那串 class 里 `pointer-events-none` 与 `opacity-0` 总是成对出现。
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

  it("puts the main enable button inside the hover group, like ProviderActions does", () => {
    // ⭐ **这次的缺陷本身**：主按钮曾被留在 hover 容器外面 ⇒ 常驻。
    //
    // 判据取自上游 `ProviderCard.tsx`：那个 hover 容器包住整个 `ProviderActions`，
    // 而 `ProviderActions` 的第一个孩子就是主按钮 ⇒ 没 hover 时右侧彻底空的。
    const tierItem = source.slice(source.indexOf("function TierItem"));

    // ⚠️ **断言的是 JSX 的嵌套关系，不是字符串先后顺序。**
    //
    // 初版断言「`provider.enable` 出现在 `TIER_HOVER_ACTIONS` 之后」—— 那是**假闸**：
    // 变异测试证明把 `: TIER_HOVER_ACTIONS` 整条删掉（按钮就常驻了）它照样绿，
    // 因为两个字符串的相对位置没变。必须真的确认主按钮是那个容器的**孩子**。
    //
    // 做法：从 `TIER_HOVER_ACTIONS` 所在的 `<div>` 起，按 `<div`/`</div>` 数深度找到
    // **配对**的收标签，截出真正的子树。
    //
    // ⚠️ 不能图省事用 `indexOf("</div>")` —— 容器里有 `{tier.isCurrent ? (…) : (…)}`
    // 三元，里面还有 div，第一个 `</div>` 是内层的（初版这么写，对正确的代码也报红）。
    // ⚠️ lastIndexOf：行根 div 的注释里也提到常量名，indexOf 会截出整行（假闸）。
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

    // 主按钮的两种文案都必须在这个子树**内**（提出去就常驻了）。
    for (const key of ["provider.enable", "provider.inUse"]) {
      expect(
        subtree,
        `${key} 不在 hover 容器的子树里 ⇒ 它会常驻显示`,
      ).toContain(key);
    }
    // 而且容器**真的**把那两个常量用在了 className 的三元里。
    //
    // ⚠️ 断言 `subtree.toContain("TIER_HOVER_ACTIONS")` 是不够的（变异测试证明的）：
    // 那个名字也出现在子树的**注释**里，于是把 `: TIER_HOVER_ACTIONS` 整条删掉
    // （按钮就常驻了）它照样绿。必须匹配到实际的表达式形状。
    expect(subtree, "hover 容器的 className 没在用 TIER_HOVER_ACTIONS").toMatch(
      /\?\s*HOVER_ACTIONS_PINNED\s*\n?\s*:\s*TIER_HOVER_ACTIONS/,
    );
  });

  it("pins the group visible while an action is running", () => {
    // 操作进行中鼠标一移开就看不到自己点的东西还在跑 —— 图标与主按钮的
    // busy 标志都要算（主按钮的 switching 转圈也在组里）。
    expect(source).toContain("HOVER_ACTIONS_PINNED");
    expect(source).toMatch(
      /checking \|\| resetting \|\| switching \|\| modelSwitching \|\| verifying\s*\n?\s*\?\s*HOVER_ACTIONS_PINNED/,
    );
  });

  it("keeps ProviderActions on the same shape: main button inside the gate", () => {
    // 跨文件的同一事实要有闸（CLAUDE.md §三点六）：「整组 hover（含主按钮）」
    // 这条分界线在 RelayRow（上面的用例）与 ProviderActions（ProviderCard 的
    // 动作区，gating 已从 ProviderCard 外层包装收进组件内部）两处各写一份。
    const actions = fs.readFileSync(PROVIDER_ACTIONS_TSX, "utf8");
    const gate = actions.indexOf("pointer-events-none opacity-0");
    const main = actions.indexOf("buttonState.text");
    expect(gate, "ProviderActions 没有 hover 组？").toBeGreaterThan(0);
    expect(main, "ProviderActions 的主按钮渲染没了？").toBeGreaterThan(0);
    // 主按钮在 hover 闸**之后**出现 ⇒ 在组内；先于闸出现就是被提出组外常驻了。
    expect(main).toBeGreaterThan(gate);

    // ProviderCard 那层不许再包一层自己的 hover 容器（双闸会打架）。
    const card = fs.readFileSync(PROVIDER_CARD_TSX, "utf8");
    expect(card, "ProviderCard 又包了一层自己的 hover 容器").not.toContain(
      "opacity-0 pointer-events-none group-hover:opacity-100",
    );
  });
});
