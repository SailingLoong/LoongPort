import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AutoModeTabContent } from "@/components/settings/AutoModeTabContent";
import {
  AUTO_MODE_CONFIRMED_STORAGE_KEY,
  markAutoModeConfirmed,
} from "@/components/proxy/autoModeConfirm";

// 在 hooks 层 mock：本测试关注「卡片如何决定走哪条开启路径 / 授权何时弹」，
// 编排的 invoke 顺序由 useEnableAutoMode 自己的测试钉住。
const enableFlowMock = vi.hoisted(() => vi.fn());
const setEnabledMock = vi.hoisted(() => vi.fn());
const setStrategyMock = vi.hoisted(() => vi.fn());
const setModelMock = vi.hoisted(() => vi.fn());

vi.mock("@/lib/query/autoMode", () => ({
  useAutoModeStatus: vi.fn((appType: string) => ({
    data: {
      enabled: appType === "codex",
      strategy: "cheapest",
      model: appType === "codex" ? "gpt-5.6-sol" : null,
      availableModels:
        appType === "codex"
          ? ["gpt-5.6-sol", "gpt-5.6-nano"]
          : appType === "gemini"
            ? ["gemini-3-pro"]
            : [],
    },
  })),
  useSetAutoModeEnabled: () => ({ mutate: setEnabledMock, isPending: false }),
  useEnableAutoMode: () => ({ mutate: enableFlowMock, isPending: false }),
  useSetAutoModeStrategy: () => ({ mutate: setStrategyMock, isPending: false }),
  useSetAutoModeModel: () => ({ mutate: setModelMock, isPending: false }),
}));

const useProxyStatusMock = vi.hoisted(() => vi.fn());
vi.mock("@/hooks/useProxyStatus", () => ({
  useProxyStatus: useProxyStatusMock,
}));

// ConfirmDialog 的 i18n 资源在测试环境为空，文案渲染成 key 本身，用 key 定位。
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const settings = {
  enableFailoverToggle: false,
  failoverConfirmed: true,
} as never;

function renderTab() {
  return render(
    <AutoModeTabContent settings={settings} onAutoSave={vi.fn()} />,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.removeItem(AUTO_MODE_CONFIRMED_STORAGE_KEY);
  useProxyStatusMock.mockReturnValue({
    isRunning: true,
    takeoverStatus: {
      claude: true,
      codex: true,
      gemini: true,
      grokbuild: true,
    },
  });
});

describe("AutoModeTabContent", () => {
  it("四个 app 一页全见，页头带 Beta 徽标与全局策略", () => {
    renderTab();

    expect(screen.getAllByText("autoMode.beta").length).toBeGreaterThanOrEqual(
      1,
    );
    // 四张卡 = 四个 app 名（getAppLabel 在测试环境走 key/默认值，用状态徽标数验证更稳：
    // codex 开着 → 恰好一个「生效中」）
    expect(screen.getAllByText("autoMode.statusActive")).toHaveLength(1);
    expect(screen.getByText("autoMode.strategyLabel")).toBeTruthy();
  });

  it("前置已满足时开启直接走 setEnabled，不弹授权（已授权过）", () => {
    markAutoModeConfirmed();
    renderTab();

    const switches = screen.getAllByRole("switch");
    // 未开启的 app（claude 排第一张卡）：点开 → 前置满足 → 直接 setEnabled
    fireEvent.click(switches[0]);
    expect(setEnabledMock).toHaveBeenCalledWith(
      expect.objectContaining({ appType: "claude", enabled: true }),
    );
    expect(enableFlowMock).not.toHaveBeenCalled();
  });

  it("前置未满足时开启走一键编排", () => {
    markAutoModeConfirmed();
    useProxyStatusMock.mockReturnValue({
      isRunning: false,
      takeoverStatus: undefined,
    });
    renderTab();

    const switches = screen.getAllByRole("switch");
    fireEvent.click(switches[0]);
    expect(enableFlowMock).toHaveBeenCalledWith({ appType: "claude" });
    expect(setEnabledMock).not.toHaveBeenCalled();
  });

  it("未授权过时先弹一次性授权，确认后才继续", () => {
    renderTab();

    const switches = screen.getAllByRole("switch");
    fireEvent.click(switches[0]);
    expect(setEnabledMock).not.toHaveBeenCalled();
    expect(enableFlowMock).not.toHaveBeenCalled();
    expect(screen.getByText("autoMode.confirm.title")).toBeTruthy();

    fireEvent.click(
      screen.getByRole("button", { name: "autoMode.confirm.confirm" }),
    );
    expect(setEnabledMock).toHaveBeenCalledWith(
      expect.objectContaining({ appType: "claude", enabled: true }),
    );
    // 授权落 localStorage，下次不再弹
    expect(localStorage.getItem(AUTO_MODE_CONFIRMED_STORAGE_KEY)).toBe("true");
  });

  it("关闭走 setEnabled(false)，不弹授权", () => {
    renderTab();

    // codex 卡开着（第二张）：点关直接关
    const switches = screen.getAllByRole("switch");
    fireEvent.click(switches[1]);
    expect(setEnabledMock).toHaveBeenCalledWith(
      expect.objectContaining({ appType: "codex", enabled: false }),
    );
    expect(screen.queryByText("autoMode.confirm.title")).toBeNull();
  });

  it("有模型目录的 app 显示模型偏好下拉，显式选择调用 setModel", () => {
    renderTab();

    const comboboxes = screen.getAllByRole("combobox");
    expect(comboboxes.length).toBe(2); // codex 与 gemini 有目录；claude/grokbuild 无

    // codex 当前偏好 gpt-5.6-sol；切到「不限模型」。
    // 注意 gemini 卡偏好为空、闭合触发器也显示「不限模型」，
    // 所以用 role=option 只定位当前展开列表里的那一项。
    fireEvent.click(comboboxes[0]);
    const anyOption = screen
      .getAllByRole("option")
      .find((el) => el.textContent === "autoMode.modelAny");
    expect(anyOption).toBeTruthy();
    fireEvent.click(anyOption!);
    expect(setModelMock).toHaveBeenCalledWith({
      appType: "codex",
      model: null,
    });
  });
});
