import { render, screen, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * 闸：**统计告知弹窗的三个前置条件**（端点已配 + 没看过告知 + 统计仍开着）。
 *
 * ## 为什么这条闸必须有
 *
 * 「弹与不弹」的每个条件都可能被写反（`&&` 写成取反、忘了放行），而那个 bug
 * 的表现是**永远不弹**：编译过、其它测试全绿、没有任何东西报错，维护者会以为
 * 用户都已经被告知过了 —— 端点 2026-09-09 已切生产，告知真的在发生，这不是
 * 理论风险。
 *
 * ⇒ 所以两个方向都要钉：**不该弹的场景不弹**，以及**该弹的场景弹**。
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
import { createTestQueryClient } from "../utils/testQueryClient";

/** 一份「还没看过告知」的设置（`statsNoticeConfirmed` 缺席即没看过）。 */
const notYetAsked = { enableAnonymousStats: true };

/** 弹窗出现的判据：标题那个 i18n key（全局 setup 的资源为空 ⇒ `t()` 回 key 本身）。 */
const TITLE_KEY = "loongport.stats.title";

function renderDialog() {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      <StatsNoticeDialog />
    </QueryClientProvider>,
  );
}

describe("统计告知弹窗的前置条件（2026-09-09 起纯告知形态）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getSettings.mockResolvedValue(notYetAsked);
  });

  it("端点还没配就不弹（回退/预发占位时没有数据流，问了也白问）", async () => {
    statsEndpointConfigured.mockResolvedValue(false);
    renderDialog();

    // 等那两个读都跑完再断言「没弹」，否则这条测试对任何实现都绿
    // —— 包括「弹了但渲染慢一拍」。
    await waitFor(() => expect(statsEndpointConfigured).toHaveBeenCalled());
    await waitFor(() => expect(getSettings).toHaveBeenCalled());
    expect(screen.queryByText(TITLE_KEY)).toBeNull();
  });

  it("端点配好了、没看过告知、统计开着：弹（反向闸 —— 恒不弹的 bug 只有这条能抓）", async () => {
    statsEndpointConfigured.mockResolvedValue(true);
    renderDialog();

    await waitFor(() => expect(screen.getByText(TITLE_KEY)).toBeTruthy());
  });

  it("看过告知就不弹，哪怕端点已配（不该再被打扰）", async () => {
    statsEndpointConfigured.mockResolvedValue(true);
    getSettings.mockResolvedValue({
      enableAnonymousStats: false,
      statsNoticeConfirmed: true,
    });
    renderDialog();

    await waitFor(() => expect(getSettings).toHaveBeenCalled());
    expect(screen.queryByText(TITLE_KEY)).toBeNull();
  });

  it("已在设置里显式关过（enabled=false 且没看过告知）：不弹 —— 关过的人不收「已默认参与」错报", async () => {
    statsEndpointConfigured.mockResolvedValue(true);
    getSettings.mockResolvedValue({ enableAnonymousStats: false });
    renderDialog();

    await waitFor(() => expect(getSettings).toHaveBeenCalled());
    expect(screen.queryByText(TITLE_KEY)).toBeNull();
  });

  it("读端点失败就不弹（不为一个统计功能在启动时弹报错）", async () => {
    statsEndpointConfigured.mockRejectedValue(new Error("命令没注册"));
    renderDialog();

    await waitFor(() => expect(statsEndpointConfigured).toHaveBeenCalled());
    expect(screen.queryByText(TITLE_KEY)).toBeNull();
  });
});
