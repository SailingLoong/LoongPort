import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AutoModeTabContent } from "@/components/settings/AutoModeTabContent";
import {
  AUTO_MODE_CONFIRMED_STORAGE_KEY,
  markAutoModeConfirmed,
} from "@/components/proxy/autoModeConfirm";

// 在 hooks 层 mock：本测试关注「总开关/卡片如何决定走哪条路径 / 授权何时弹」，
// 编排的 invoke 顺序由 useEnableAutoMode 自己的测试钉住。
const enableFlowMock = vi.hoisted(() => vi.fn());
const disableFlowMock = vi.hoisted(() => vi.fn());
const setAllMock = vi.hoisted(() => vi.fn());
const setFailoverAllMock = vi.hoisted(() => vi.fn());
const setEnabledMock = vi.hoisted(() => vi.fn());
const setStrategyMock = vi.hoisted(() => vi.fn());
const setModelMock = vi.hoisted(() => vi.fn());

const statusMock = vi.hoisted(() => vi.fn());
vi.mock("@/lib/query/autoMode", () => ({
  useAutoModeStatus: statusMock,
  useSetAutoModeEnabled: () => ({ mutate: setEnabledMock, isPending: false }),
  useEnableAutoMode: () => ({ mutate: enableFlowMock, isPending: false }),
  useDisableAutoMode: () => ({ mutate: disableFlowMock, isPending: false }),
  useSetAutoModeAll: () => ({ mutate: setAllMock, isPending: false }),
  useSetAutoModeStrategy: () => ({ mutate: setStrategyMock, isPending: false }),
  useSetAutoModeModel: () => ({ mutate: setModelMock, isPending: false }),
  useSetFailoverAll: () => ({ mutate: setFailoverAllMock, isPending: false }),
}));

const failoverEnabledMock = vi.hoisted(() => vi.fn());
vi.mock("@/lib/query/failover", () => ({
  useAutoFailoverEnabled: failoverEnabledMock,
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

/** 四个 app 的省心模式状态。默认：claude/codex 有档位，codex/gemini 开着。 */
function stubStatuses(overrides: Record<string, object> = {}) {
  const base = {
    claude: { enabled: false, hasCandidates: true, availableModels: [] },
    codex: {
      enabled: true,
      hasCandidates: true,
      model: "gpt-5.6-sol",
      availableModels: ["gpt-5.6-sol", "gpt-5.6-nano"],
    },
    gemini: { enabled: true, hasCandidates: false, availableModels: [] },
    grokbuild: { enabled: false, hasCandidates: false, availableModels: [] },
  } as Record<string, object>;
  statusMock.mockImplementation((appType: string) => ({
    data: {
      strategy: "cheapest",
      model: null,
      ...base[appType],
      ...overrides[appType],
    },
  }));
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
  stubStatuses();
  failoverEnabledMock.mockReturnValue({ data: true });
});

describe("AutoModeTabContent", () => {
  it("页头带 Beta 徽标与总开关；卡片按档位情况区分可开性", () => {
    renderTab();

    expect(screen.getAllByText("autoMode.beta").length).toBeGreaterThanOrEqual(
      1,
    );
    // codex 与 gemini（历史遗留的开启态）都开着 → 两个「生效中」
    expect(screen.getAllByText("autoMode.statusActive")).toHaveLength(2);
    // gemini / grokbuild 没档位 → 两张卡提示不可开
    expect(screen.getAllByText("autoMode.noCandidatesHint")).toHaveLength(2);
  });

  it("总开关：未全开时点开只对有档位的 app 批量开启", () => {
    renderTab();

    // claude（有档位但未开）存在 ⇒ 总开关未全开
    const switches = screen.getAllByRole("switch");
    fireEvent.click(switches[0]);
    expect(setAllMock).toHaveBeenCalledWith({
      apps: ["claude", "codex"], // 只有有档位的 app
      enable: true,
    });
  });

  it("总开关全开时点关，对已开的 app 批量关闭", () => {
    stubStatuses({ claude: { enabled: true } });
    renderTab();

    const switches = screen.getAllByRole("switch");
    fireEvent.click(switches[0]);
    expect(setAllMock).toHaveBeenCalledWith({
      apps: ["claude", "codex", "gemini"], // enabled 的都关
      enable: false,
    });
  });

  it("统一故障转移行：切换走 setFailoverAll（全部 app）", () => {
    renderTab();

    expect(screen.getByText("proxy.failover.autoSwitch")).toBeTruthy();
    const switches = screen.getAllByRole("switch");
    // 末尾两个 switch：统一故障转移行 + 主页面显示开关
    const failoverSwitch = switches[switches.length - 2];
    fireEvent.click(failoverSwitch);
    expect(setFailoverAllMock).toHaveBeenCalledWith({
      apps: ["claude", "codex", "gemini", "grokbuild"],
      enabled: false,
    });
  });

  it("卡片：前置已满足时开启直接 setEnabled（已授权过）", () => {
    markAutoModeConfirmed();
    renderTab();

    const switches = screen.getAllByRole("switch");
    // switch 顺序：总开关(0)、claude 卡(1)、codex 卡(2)……
    fireEvent.click(switches[1]);
    expect(setEnabledMock).toHaveBeenCalledWith(
      expect.objectContaining({ appType: "claude", enabled: true }),
    );
  });

  it("卡片：前置未满足时开启走一键编排", () => {
    markAutoModeConfirmed();
    // mock 要在渲染前生效（mockReturnValue 不会触发已渲染组件重算）
    useProxyStatusMock.mockReturnValue({
      isRunning: false,
      takeoverStatus: undefined,
    });
    renderTab();

    const switches = screen.getAllByRole("switch");
    fireEvent.click(switches[1]);
    expect(enableFlowMock).toHaveBeenCalledWith({ appType: "claude" });
    expect(setEnabledMock).not.toHaveBeenCalled();
  });

  it("卡片关闭走关态编排（连路由一起收回），不弹授权", () => {
    renderTab();

    const switches = screen.getAllByRole("switch");
    // codex 卡开着（第 3 个 switch：总开关、claude、codex）
    fireEvent.click(switches[2]);
    expect(disableFlowMock).toHaveBeenCalledWith({ appType: "codex" });
    expect(screen.queryByText("autoMode.confirm.title")).toBeNull();
  });

  it("未授权过时先弹一次性授权，确认后才继续", () => {
    renderTab();

    const switches = screen.getAllByRole("switch");
    fireEvent.click(switches[1]);
    expect(setEnabledMock).not.toHaveBeenCalled();
    expect(enableFlowMock).not.toHaveBeenCalled();
    expect(screen.getByText("autoMode.confirm.title")).toBeTruthy();

    fireEvent.click(
      screen.getByRole("button", { name: "autoMode.confirm.confirm" }),
    );
    expect(setEnabledMock).toHaveBeenCalledWith(
      expect.objectContaining({ appType: "claude", enabled: true }),
    );
    expect(localStorage.getItem(AUTO_MODE_CONFIRMED_STORAGE_KEY)).toBe("true");
  });

  it("有模型目录的 app 显示模型偏好下拉，显式选择调用 setModel", () => {
    renderTab();

    const comboboxes = screen.getAllByRole("combobox");
    expect(comboboxes.length).toBe(1); // 只有 codex 有目录

    // codex 当前偏好 gpt-5.6-sol；切到「不限模型」。
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
