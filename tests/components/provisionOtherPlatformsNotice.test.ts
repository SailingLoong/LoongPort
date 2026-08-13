import { describe, expect, it } from "vitest";
import type { ProvisionSummary, TierInfo } from "@/lib/api/relay";
import {
  countTiersForApp,
  sumTiersForApp,
  tiersLandedElsewhere,
} from "@/components/relay/provisionScope";

/**
 * 闸：**provision 拉到了分组、但一个都不属于当前平台时，要说清楚。**
 *
 * ## 它守的是什么缺陷（TODO 债 11）
 *
 * `provision` 一次探**全部平台**，每个分组按自己的 `platform` 落到对应 CLI。
 * 于是「某个站压根没有 anthropic 分组」时，claude 页那一行是零档位 ——
 * 而 UI 上它与「拉取失败」长得**一模一样**（都落到后端的「当前平台没有档位」
 * 状态，显示「该账号在此平台下没有可用分组」）。
 *
 * 那句话本身没说错，但它把两种完全不同的处境说成了一种：
 *
 * - **真的没有** —— 这个站不卖 Claude 分组，用户再点一百次「获取密钥」也不会有。
 *   他该做的是去别的平台 tab，或换一个站。
 * - **拉失败了** —— 网络 / sk 撤销 / 服务端 5xx，重试有意义。
 *
 * 而 `ProvisionSummary` 手上**本来就有**区分它们所需的信息：`tiers` 是全平台的，
 * 里头有几条、分别落在哪个平台都知道。少的只是把它说出来。
 *
 * ## 为什么判据是纯函数而不是写在组件里
 *
 * 它要回答的是「这次 provision 的结果，对当前这一屏意味着什么」—— 一条领域规则。
 * 写在 JSX 里就得渲染整个面板才能测三种边界；抽出来后一个纯函数钉住。
 * 形状照 `lowBalance.ts` / `removeConfirmWording.ts`（同目录先例）。
 */
describe("provision 结果落在别的平台", () => {
  it("当前平台一条都没拿到、别的平台拿到了 ⇒ 要报出来", () => {
    expect(
      tiersLandedElsewhere({
        currentAppTiers: 0,
        totalTiers: 3,
      }),
    ).toBe(3);
  });

  /**
   * 当前平台拿到了档位就什么都不用说 —— 那一行会显示档位数，用户看得见结果。
   * 此时别的平台也拿到了是**常态**（一次 provision 探全部平台），报出来只是噪音。
   */
  it("当前平台拿到了 ⇒ 不报", () => {
    expect(
      tiersLandedElsewhere({
        currentAppTiers: 2,
        totalTiers: 5,
      }),
    ).toBe(0);
  });

  /**
   * 这条是**这个判据存在的边界**：一条都没拉到（全平台皆空）时不能说
   * 「分组落在别的平台了」—— 那是在编一个不存在的去处。
   *
   * 这种情况后端本来就会报错（`provision::provision` 在 `usable.is_empty()` 时
   * 返回「这个账号下没有本客户端支持的活跃分组」），走的是 catch 分支。
   * 判据这里仍要挡住，否则将来后端放宽成「返回空 summary」时会静默说错话。
   */
  it("全平台都是零 ⇒ 不报（没有「别的平台」可指）", () => {
    expect(
      tiersLandedElsewhere({
        currentAppTiers: 0,
        totalTiers: 0,
      }),
    ).toBe(0);
  });

  /**
   * ⭐ **有分组建 sk 失败时不许说「落在别的平台」** —— 那是**错误归因**，
   * 正是这条债要消灭的那一类。
   *
   * 场景：这个站有 codex 分组，但那一条建密钥失败了（Key 上限 / 服务端 5xx）。
   * 此刻 `currentAppTiers === 0`（那条没成为档位）而 `totalTiers > 0`
   * （别的平台成了）—— 只看这两个数会得出「你的分组都在别的平台」，
   * 而真相是「这个平台的分组建密钥失败了，重试有意义」。
   *
   * 把用户指向别的 tab 会让他**永远不去重试那条真能修好的路**。
   * 失败原因由 `failures` 那几条 warning 逐条说，这里保持沉默即可。
   */
  it("有分组失败时不报（那时零档位的原因是失败，不是平台不符）", () => {
    expect(
      tiersLandedElsewhere({
        currentAppTiers: 0,
        totalTiers: 2,
        failureCount: 1,
      }),
    ).toBe(0);
  });

  /**
   * ⚠️ **这条钉住一处「知情的漏报」，不是钉住正确行为**（review 抓出）。
   *
   * 场景：站上**真的没有**当前平台的分组，同时别的平台有一条建 sk 失败。
   * 本该指路（「你的分组都在别的 tab」），但上面那道 failureCount 守卫会让它沉默 ⇒
   * 用户只看到一条与他这屏无关的 warning，继续点「获取密钥」。
   *
   * 仍然这么选，是因为**错误归因比漏报更糟**：反过来说「分组在别的平台」时，
   * 若真相是「这个平台的分组建密钥失败了」，用户会跑去别的 tab 找一个不存在的东西，
   * 而那条真能修好的路（重试）他永远不会走。
   *
   * 写成测试而不是只写注释：这样「有人哪天把守卫去掉」时，是**这条**变红并让他读到
   * 上面这段取舍，而不是让漏报悄悄变成错误归因。
   *
   * 真修要给 `ProvisionSummary.failures` 带上 platform（现在是
   * `(group_name, reason)`，分不出失败的那条属于哪个平台）—— 那是另一笔债。
   */
  it("已知代价：真·平台不符 + 别平台有失败 ⇒ 也沉默（宁可漏报不错误归因）", () => {
    expect(
      tiersLandedElsewhere({
        // 当前平台一条都没有（站上真没有这个平台的分组）
        currentAppTiers: 0,
        // 别的平台成了 3 条
        totalTiers: 3,
        // 但别的平台还有一条失败了 ⇒ 守卫触发 ⇒ 本该指路却沉默
        failureCount: 1,
      }),
    ).toBe(0);
  });
});

/**
 * `countTiersForApp` 是**上面那个判据的唯一喂料口**，所以它自己也要有闸：
 * 它退化成 `r.tiers.length`（不筛）时，「当前平台拿到了没有」就永远答成「拿到了」
 * ⇒ 债 11 原封不动，而 `tiersLandedElsewhere` 那三条照样绿。
 */
describe("数出落在当前平台的档位", () => {
  function summary(appIds: TierInfo["appId"][]): ProvisionSummary {
    return {
      tiers: appIds.map((appId, i) => ({
        providerId: `loongport-000000000000000${i}`,
        appId,
        groupName: `g${i}`,
        displayName: `g${i}`,
        model: "gpt-5.6-sol",
        models: ["gpt-5.6-sol"],
        rateMultiplier: null,
        isCurrent: false,
        userEdited: null,
        isImageModel: false,
        allowImageGeneration: null,
      })),
      failures: [],
      keysCreated: 0,
      mergedProviders: [],
    };
  }

  it("只数属于这个平台的，别的平台不算进来", () => {
    const r = summary(["codex", "claude", "codex", "gemini"]);

    expect(countTiersForApp(r, "codex")).toBe(2);
    expect(countTiersForApp(r, "claude")).toBe(1);
    // 这条是关键：某个平台一条分组都没有时必须数出 0，而不是跟着总数走
    // —— 退化成 `r.tiers.length` 会让它答 4，债 11 原封不动。
    expect(countTiersForApp(r, "openclaw")).toBe(0);
  });

  it("空结果数出零", () => {
    expect(countTiersForApp(summary([]), "codex")).toBe(0);
  });
});

/**
 * ⭐ 顶部「刷新」那条**批量**路径的同一个缺陷。
 *
 * 它对每个已登录中转站各跑一次 provision，然后把结果累加成一句
 * 「已刷新 N 个中转站，共 M 个档位」。而 `M` 原来累的是 `r.value.tiers.length`
 * —— **全平台总数**。
 *
 * 失败场景：三个中转站各有 codex/claude/gemini 三种分组，用户在 codex 屏点「刷新」
 * ⇒ 提示「共 9 个档位」，而他眼前那一屏只有 3 个。数字对不上时用户没法判断是
 * 提示错了还是界面漏了，而这条提示是绿色的成功语气。
 */
describe("批量刷新的档位计数", () => {
  function summaryFor(appIds: TierInfo["appId"][]): ProvisionSummary {
    return {
      tiers: appIds.map((appId, i) => ({
        providerId: `loongport-00000000000000${i}`,
        appId,
        groupName: `g${i}`,
        displayName: `g${i}`,
        model: "gpt-5.6-sol",
        models: ["gpt-5.6-sol"],
        rateMultiplier: null,
        isCurrent: false,
        userEdited: null,
        isImageModel: false,
        allowImageGeneration: null,
      })),
      failures: [],
      keysCreated: 0,
      mergedProviders: [],
    };
  }

  it("跨多个中转站累加时也只数当前平台的", () => {
    const perRelay = [
      summaryFor(["codex", "claude", "gemini"]),
      summaryFor(["codex", "codex", "claude"]),
      summaryFor(["claude"]),
    ];

    // codex：1 + 2 + 0 = 3。退化成 `tiers.length` 会得 3+3+1 = 7。
    expect(sumTiersForApp(perRelay, "codex")).toBe(3);
    expect(sumTiersForApp(perRelay, "claude")).toBe(3);
    expect(sumTiersForApp(perRelay, "gemini")).toBe(1);
    // 一条都没有的平台必须是 0，不能跟着总数走。
    expect(sumTiersForApp(perRelay, "hermes")).toBe(0);
  });

  it("空列表是 0（一个中转站都没登录时）", () => {
    expect(sumTiersForApp([], "codex")).toBe(0);
  });
});
