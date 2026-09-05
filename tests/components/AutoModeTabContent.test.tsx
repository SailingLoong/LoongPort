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
const masterFlowMock = vi.hoisted(() => vi.fn());
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
  useSetEasyModeMaster: () => ({ mutate: masterFlowMock, isPending: false }),
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
    claude: {
      enabled: false,
      hasCandidates: true,
      cliInstalled: true,
      availableModels: [],
    },
    codex: {
      enabled: true,
      hasCandidates: true,
      cliInstalled: true,
      model: "gpt-5.6-sol",
      availableModels: ["gpt-5.6-sol", "gpt-5.6-nano"],
    },
    gemini: {
      enabled: true,
      hasCandidates: false,
      cliInstalled: true,
      availableModels: [],
    },
    grokbuild: {
      enabled: false,
      hasCandidates: false,
      cliInstalled: false,
      availableModels: [],
    },
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
  it("页头带总开关；卡片按档位情况区分可开性", () => {
    renderTab();

    // codex 与 gemini（历史遗留的开启态）都开着 → 两个「生效中」
    expect(screen.getAllByText("autoMode.statusActive")).toHaveLength(2);
    // gemini / grokbuild 没档位 → 两张卡提示不可开
    expect(screen.getAllByText("autoMode.noCandidatesHint")).toHaveLength(2);
  });

  it("总开关 = 本地路由运行态：运行中显示为开，点关走 masterFlow(enable:false)", () => {
    renderTab();

    const master = screen.getAllByRole("switch")[0];
    expect(master.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(master);
    expect(masterFlowMock).toHaveBeenCalledWith({ enable: false });
  });

  it("总开关：路由未开且未授权时先弹授权，确认后 masterFlow(enable:true)", () => {
    useProxyStatusMock.mockReturnValue({
      isRunning: false,
      takeoverStatus: undefined,
    });
    renderTab();

    const master = screen.getAllByRole("switch")[0];
    expect(master.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(master);
    expect(masterFlowMock).not.toHaveBeenCalled();
    expect(screen.getByText("autoMode.confirm.title")).toBeTruthy();

    fireEvent.click(
      screen.getByRole("button", { name: "autoMode.confirm.confirm" }),
    );
    expect(masterFlowMock).toHaveBeenCalledWith({ enable: true });
    expect(localStorage.getItem(AUTO_MODE_CONFIRMED_STORAGE_KEY)).toBe("true");
  });

  it("总开关：已授权（或路由已在跑）时点开直接 masterFlow(enable:true)", () => {
    markAutoModeConfirmed();
    useProxyStatusMock.mockReturnValue({
      isRunning: false,
      takeoverStatus: undefined,
    });
    renderTab();

    fireEvent.click(screen.getAllByRole("switch")[0]);
    expect(masterFlowMock).toHaveBeenCalledWith({ enable: true });
    expect(screen.queryByText("autoMode.confirm.title")).toBeNull();
  });

  it("卡片不再承载模式开关：只剩总开关，卡片显示模式徽章与指路提示", () => {
    useProxyStatusMock.mockReturnValue({
      isRunning: false,
      takeoverStatus: undefined,
    });
    renderTab();

    // 唯一入口在主页面 —— 卡片不再有 switch；剩余 3 个 = 总开关 + 统一故障转移 + 主页面显示
    expect(screen.getAllByRole("switch")).toHaveLength(3);
    // 开着的 codex 显示「省心」徽章、关着的 claude 显示「自主」
    expect(screen.getAllByText("autoMode.runMode.easy")).toHaveLength(2);
    expect(screen.getAllByText("autoMode.runMode.self")).toHaveLength(2);
    expect(screen.getAllByText("autoMode.tab.switchMovedHint")).toHaveLength(4);
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
