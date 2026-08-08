import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * 闸：**统计告知弹窗的两个前置条件**（端点已配 + 用户没表过态）。
 *
 * ## 为什么这条闸必须有
 *
 * 上报端点现在还是占位（`stats.rs` 的 `ENDPOINT` 含 `.invalid`），所以这一屏
 * **有意不弹** —— 那时同意与不同意的实际后果完全相同，问了是消耗用户的信任
 * 却换不到数据。
 *
 * 而这个「不弹」与「坏掉了所以不弹」在界面上**长得一模一样**。端点配好那天
 * 若这个条件写错（比如把 `&&` 写成了取反、或忘了在 endpoint 为 true 时放行），
 * 表现是**永远不弹**：编译过、其它测试全绿、没有任何东西报错，
 * 而维护者会以为用户都已经被告知过了。
 *
 * ⇒ 所以两个方向都要钉：**没配时不弹**，以及**配好了就弹**。
 * 只钉前者的话，一个「恒为 false」的实现也能过。
 *
 * ## 为什么是渲染测试
 *
 * 要验的是「这一屏到底出不出现」这个行为。源码断言（grep 出现过
 * `statsEndpointConfigured`）会漏掉真实的失败形态：读了那个值但没用进条件、
 * 或者用错了方向，字符串照样匹配得上。
 */
const { statsEndpointConfigured, getSettings, saveSettings } = vi.hoisted(
  () => ({
    statsEndpointConfigured: vi.fn(),
    getSettings: vi.fn(),
    saveSettings: vi.fn(),
  }),
);

vi.mock("@/lib/api", () => ({
  relayApi: { statsEndpointConfigured },
  settingsApi: { get: getSettings, save: saveSettings },
}));

import { StatsNoticeDialog } from "@/components/relay/StatsNoticeDialog";

/** 一份「还没表过态」的设置（`statsNoticeConfirmed` 缺席即未表态）。 */
const notYetAsked = { enableAnonymousStats: true };

/** 弹窗出现的判据：标题那个 i18n key（全局 setup 的资源为空 ⇒ `t()` 回 key 本身）。 */
const TITLE_KEY = "loongport.stats.title";

describe("统计告知弹窗的前置条件", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getSettings.mockResolvedValue(notYetAsked);
  });

  it("端点还没配就不弹（同意与不同意后果相同，问了也白问）", async () => {
    statsEndpointConfigured.mockResolvedValue(false);
    render(<StatsNoticeDialog />);

    // 等那两个读都跑完再断言「没弹」，否则这条测试对任何实现都绿
    // —— 包括「弹了但渲染慢一拍」。
    await waitFor(() => expect(statsEndpointConfigured).toHaveBeenCalled());
    await waitFor(() => expect(getSettings).toHaveBeenCalled());
    expect(screen.queryByText(TITLE_KEY)).toBeNull();
  });

  it("端点配好了且用户没表过态就弹（否则告知永远不会发生）", async () => {
    statsEndpointConfigured.mockResolvedValue(true);
    render(<StatsNoticeDialog />);

    // ⭐ 这条是上一条的**反向闸**：少了它，一个「恒不弹」的实现也能全绿，
    // 而那个 bug 的表现是端点配好之后用户永远收不到告知。
    await waitFor(() => expect(screen.getByText(TITLE_KEY)).toBeTruthy());
  });

  it("用户已经表过态就不弹，哪怕端点已配（选了什么都不该再被打扰）", async () => {
    statsEndpointConfigured.mockResolvedValue(true);
    // 表过态 = 这个字段有值。这里取 `false`（选了不参与）——
    // 那也是一次有效表态，不该被重复询问。
    getSettings.mockResolvedValue({
      enableAnonymousStats: false,
      statsNoticeConfirmed: true,
    });
    render(<StatsNoticeDialog />);

    await waitFor(() => expect(getSettings).toHaveBeenCalled());
    expect(screen.queryByText(TITLE_KEY)).toBeNull();
  });

  it("读端点失败就不弹（不为一个统计功能在启动时弹报错）", async () => {
    statsEndpointConfigured.mockRejectedValue(new Error("命令没注册"));
    render(<StatsNoticeDialog />);

    await waitFor(() => expect(statsEndpointConfigured).toHaveBeenCalled());
    expect(screen.queryByText(TITLE_KEY)).toBeNull();
  });
});
