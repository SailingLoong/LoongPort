import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AppModeSegmented } from "@/components/proxy/AppModeSegmented";
import {
  AUTO_MODE_CONFIRMED_STORAGE_KEY,
  markAutoModeConfirmed,
} from "@/components/proxy/autoModeConfirm";

// 模式唯一入口的分段选择器：关注「两态渲染 / 点击落到哪条编排 / 授权时机」；
// 编排的 invoke 顺序由 useEnableAutoMode 自己的测试钉住。
const statusMock = vi.hoisted(() => vi.fn());
const enableFlowMock = vi.hoisted(() => vi.fn());
const disableFlowMock = vi.hoisted(() => vi.fn());

vi.mock("@/lib/query/autoMode", () => ({
  useAutoModeStatus: statusMock,
  useEnableAutoMode: () => ({ mutate: enableFlowMock, isPending: false }),
  useDisableAutoMode: () => ({ mutate: disableFlowMock, isPending: false }),
}));

// ConfirmDialog 的 i18n 资源在测试环境为空，文案渲染成 key 本身，用 key 定位。
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    // i18next 两种默认值形式都支持：t(key, {defaultValue}) 与 t(key, "默认值")
    t: (key: string, opts?: unknown) =>
      typeof opts === "string"
        ? opts
        : ((opts as { defaultValue?: string } | undefined)?.defaultValue ??
          key),
  }),
}));

function stubStatus(overrides: object = {}) {
  statusMock.mockReturnValue({
    data: {
      enabled: false,
      strategy: "cheapest",
      model: null,
      availableModels: [],
      hasCandidates: true,
      cliInstalled: true,
      ...overrides,
    },
    isLoading: false,
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.removeItem(AUTO_MODE_CONFIRMED_STORAGE_KEY);
});

describe("AppModeSegmented", () => {
  it("自主态：省心可点、自主为选中态", () => {
    stubStatus({ enabled: false });
    render(<AppModeSegmented activeApp="claude" />);

    const easy = screen.getByRole("button", { name: "省心" });
    const self = screen.getByRole("button", { name: "自主" });
    expect(easy).toBeEnabled();
    expect(self.className).toContain("emerald");
    // 点已选中的模式是无操作，不触发任何编排
    fireEvent.click(self);
    expect(enableFlowMock).not.toHaveBeenCalled();
    expect(disableFlowMock).not.toHaveBeenCalled();
  });

  it("省心态：点自主走 disableFlow（收接管不停路由）", () => {
    stubStatus({ enabled: true });
    render(<AppModeSegmented activeApp="claude" />);

    fireEvent.click(screen.getByRole("button", { name: "自主" }));
    expect(disableFlowMock).toHaveBeenCalledWith({ appType: "claude" });
    expect(enableFlowMock).not.toHaveBeenCalled();
  });

  it("自主态未授权：点省心先弹一次性授权，确认后 enableFlow", async () => {
    stubStatus({ enabled: false });
    render(<AppModeSegmented activeApp="codex" />);

    fireEvent.click(screen.getByRole("button", { name: "省心" }));
    expect(enableFlowMock).not.toHaveBeenCalled();
    expect(await screen.findByText("开启省心模式")).toBeTruthy();

    fireEvent.click(await screen.findByRole("button", { name: "开启" }));
    expect(enableFlowMock).toHaveBeenCalledWith({ appType: "codex" });
    expect(localStorage.getItem(AUTO_MODE_CONFIRMED_STORAGE_KEY)).toBe("true");
  });

  it("已授权：点省心直接 enableFlow", () => {
    markAutoModeConfirmed();
    stubStatus({ enabled: false });
    render(<AppModeSegmented activeApp="claude" />);

    fireEvent.click(screen.getByRole("button", { name: "省心" }));
    expect(enableFlowMock).toHaveBeenCalledWith({ appType: "claude" });
    expect(disableFlowMock).not.toHaveBeenCalled();
  });

  it("无托管档位或 CLI 未装：省心不可点并给原因", () => {
    stubStatus({ enabled: false, hasCandidates: false });
    const { rerender } = render(<AppModeSegmented activeApp="gemini" />);
    expect(screen.getByRole("button", { name: "省心" })).toBeDisabled();

    stubStatus({ enabled: false, hasCandidates: true, cliInstalled: false });
    rerender(<AppModeSegmented activeApp="gemini" />);
    const easy = screen.getByRole("button", { name: "省心" });
    expect(easy).toBeDisabled();
    // 省心态下（已开启）即使 CLI 后来没了，也随时允许切回自主
    stubStatus({ enabled: true, cliInstalled: false });
    rerender(<AppModeSegmented activeApp="gemini" />);
    expect(screen.getByRole("button", { name: "自主" })).toBeEnabled();
  });
});
