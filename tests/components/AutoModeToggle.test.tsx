import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AutoModeToggle } from "@/components/proxy/AutoModeToggle";
import { AUTO_MODE_CONFIRMED_STORAGE_KEY } from "@/components/proxy/autoModeConfirm";

// 与 AutoModeTabContent.test 同一套路：hooks 层 mock，本测试只关心顶栏开关的
// 显隐与流转（编排顺序由 useEnableAutoMode 自己的测试钉住）。
const enableFlowMock = vi.hoisted(() => vi.fn());
const disableFlowMock = vi.hoisted(() => vi.fn());
const statusValue = vi.hoisted(() => ({ current: {} as object }));

vi.mock("@/lib/query/autoMode", () => ({
  useAutoModeStatus: () => ({ data: statusValue.current, isLoading: false }),
  useEnableAutoMode: () => ({ mutate: enableFlowMock, isPending: false }),
  useDisableAutoMode: () => ({ mutate: disableFlowMock, isPending: false }),
}));

// ConfirmDialog 的 i18n 资源在测试环境为空，文案渲染成 key 本身，用 key 定位。
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

// 弹窗不是这里的被测对象，桩成「开着就出一个确定按钮」
vi.mock("@/components/ConfirmDialog", () => ({
  ConfirmDialog: ({
    isOpen,
    onConfirm,
  }: {
    isOpen: boolean;
    onConfirm: () => void;
  }) =>
    isOpen ? (
      <button type="button" onClick={onConfirm}>
        confirm-dialog-ok
      </button>
    ) : null,
}));

beforeEach(() => {
  enableFlowMock.mockClear();
  disableFlowMock.mockClear();
  localStorage.removeItem(AUTO_MODE_CONFIRMED_STORAGE_KEY);
});

/** 指定 useAutoModeStatus 的返回值（渲染前生效） */
function stubStatus(status: object) {
  statusValue.current = status;
}

describe("AutoModeToggle", () => {
  it("未开启时也常驻渲染，图标为静默态（不再「生效才出现」）", () => {
    stubStatus({ enabled: false, hasCandidates: true });
    const { container } = render(<AutoModeToggle activeApp="claude" />);
    expect(screen.getByRole("switch")).toBeInTheDocument();
    expect(
      container.querySelector(".status-heartbeat"),
      "未生效不应有生效动画",
    ).toBeNull();
  });

  it("开启态点击即收回（useDisableAutoMode，连路由接管）", () => {
    stubStatus({ enabled: true, hasCandidates: true });
    render(<AutoModeToggle activeApp="claude" />);
    fireEvent.click(screen.getByRole("switch"));
    expect(disableFlowMock).toHaveBeenCalledWith({ appType: "claude" });
  });

  it("首次开启先弹一次性授权，确认后才执行并记住授权", () => {
    stubStatus({ enabled: false, hasCandidates: true });
    render(<AutoModeToggle activeApp="claude" />);
    fireEvent.click(screen.getByRole("switch"));
    expect(enableFlowMock).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("confirm-dialog-ok"));
    expect(enableFlowMock).toHaveBeenCalledWith({ appType: "claude" });
    expect(localStorage.getItem(AUTO_MODE_CONFIRMED_STORAGE_KEY)).toBe("true");
  });

  it("已授权过则直接开启，不再弹", () => {
    localStorage.setItem(AUTO_MODE_CONFIRMED_STORAGE_KEY, "true");
    stubStatus({ enabled: false, hasCandidates: true });
    render(<AutoModeToggle activeApp="claude" />);
    fireEvent.click(screen.getByRole("switch"));
    expect(enableFlowMock).toHaveBeenCalledWith({ appType: "claude" });
    expect(screen.queryByText("confirm-dialog-ok")).toBeNull();
  });

  it("无托管档位且未开启时灰化（可关不可开，与设置页同判据）", () => {
    stubStatus({ enabled: false, hasCandidates: false });
    render(<AutoModeToggle activeApp="claude" />);
    expect(screen.getByRole("switch")).toBeDisabled();
  });
});
