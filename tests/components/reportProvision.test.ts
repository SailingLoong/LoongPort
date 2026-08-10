import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TFunction } from "i18next";
import type { ProvisionSummary, TierInfo } from "@/lib/api/relay";

/**
 * 闸：**「备好密钥」这一步的播报说的是当前那一屏的事实。**
 *
 * ## 它守的是什么
 *
 * `provision` 一次探**全部平台**，`ProvisionSummary.tiers` 是全平台的结果，而用户
 * 面前那一屏只显示当前 app 的档位。所以播报里的数字**必须筛过平台**，否则会说出
 * 「已备好 4 个档位」而他眼前是空的 —— 那比不说更糟（他会以为是渲染坏了）。
 *
 * 三条分支各自都会说错话，所以逐条钉：
 *
 * 1. 当前平台有档位 ⇒ 报**它的**数目，不是全平台总数
 * 2. 当前平台零档位、别的平台有 ⇒ 说清「去别的 tab 看」，别让他一直点「获取密钥」
 * 3. 有分组建密钥失败 ⇒ **不许**说「落在别的平台」（那是错误归因，见下）
 */
const toastSuccess = vi.fn();
const toastInfo = vi.fn();
const toastWarning = vi.fn();
vi.mock("sonner", () => ({
  toast: {
    success: toastSuccess,
    info: toastInfo,
    warning: toastWarning,
    error: vi.fn(),
  },
}));

const { reportProvision } = await import("@/components/relay/reportProvision");

/** 把 key 与插值拼进返回值 —— 断言才管得到「说出的数字对不对」。 */
const t = ((key: string, options?: Record<string, unknown>) =>
  options ? `${key} ${JSON.stringify(options)}` : key) as unknown as TFunction;

function tier(appId: TierInfo["appId"], n: number): TierInfo {
  return {
    providerId: `loongport-00000000000000${n}`,
    appId,
    groupName: `g${n}`,
    displayName: `g${n}`,
    rateMultiplier: null,
    isCurrent: false,
    userEdited: null,
    allowImageGeneration: null,
  };
}

function summary(over: Partial<ProvisionSummary> = {}): ProvisionSummary {
  return { tiers: [], failures: [], keysCreated: 0, mergedProviders: [], ...over };
}

describe("provision 结果的播报", () => {
  beforeEach(() => vi.clearAllMocks());

  it("报的是当前平台的档位数，不是全平台总数", () => {
    reportProvision(
      t,
      summary({
        tiers: [tier("codex", 1), tier("claude", 2), tier("claude", 3)],
      }),
      "codex",
    );

    // codex 只有 1 个。说 3 就是把别的 tab 的算进来了。
    expect(toastSuccess).toHaveBeenCalledWith(
      expect.stringContaining('"count":1'),
    );
    expect(toastInfo).not.toHaveBeenCalled();
  });

  it("当前平台零档位、别的平台有 ⇒ 指路去那边，不报「已备好」", () => {
    reportProvision(
      t,
      summary({ tiers: [tier("claude", 1), tier("gemini", 2)] }),
      "codex",
    );

    // 关键：不能说「已备好 0 个档位」（读起来像成功了），要说清分组在别处。
    expect(toastInfo).toHaveBeenCalledWith(
      expect.stringContaining("landedOnOtherPlatforms"),
    );
    expect(toastSuccess).not.toHaveBeenCalled();
  });

  /**
   * ⭐ 这条是**错误归因**的闸。有分组建密钥失败时，「当前平台零档位」的原因可能
   * 就是那次失败（Key 上限 / 5xx），而不是「这个平台没有分组」。说成后者会把用户
   * 指向别的 tab ⇒ 他永远不去重试那条真能修好的路。
   */
  it("有分组失败时不说「落在别的平台」，而是逐条报失败", () => {
    reportProvision(
      t,
      summary({
        tiers: [tier("claude", 1)],
        failures: [{ groupName: "pro池", reason: "已达 Key 上限" }],
      }),
      "codex",
    );

    expect(toastInfo).not.toHaveBeenCalled();
    // 失败必须点名说出来 —— 静默吞掉会让用户以为全部备好了，
    // 直到点那一条拿到看不懂的 401 才发现。
    expect(toastWarning).toHaveBeenCalledWith(expect.stringContaining("pro池"));
    expect(toastWarning).toHaveBeenCalledWith(
      expect.stringContaining("已达 Key 上限"),
    );
  });

  it("新建了密钥时报出把数（那是账号级信号：每次都在新建说明认领坏了）", () => {
    reportProvision(
      t,
      summary({ tiers: [tier("codex", 1)], keysCreated: 2 }),
      "codex",
    );

    expect(toastSuccess).toHaveBeenCalledWith(
      expect.stringContaining("readyWithKeys"),
    );
    expect(toastSuccess).toHaveBeenCalledWith(
      expect.stringContaining('"keys":2'),
    );
  });

  it("收编重复配置时明确报告，不让删除行为静默发生", () => {
    reportProvision(
      t,
      summary({
        mergedProviders: [
          { name: "Imported duplicate", appId: "codex" },
          { name: "Another duplicate", appId: "claude" },
        ],
      }),
      "codex",
    );

    expect(toastInfo).toHaveBeenCalledWith(
      expect.stringContaining("mergedProviders"),
    );
    expect(toastInfo).toHaveBeenCalledWith(
      expect.stringContaining('"count":2'),
    );
  });
});
