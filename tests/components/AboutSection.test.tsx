import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => {
  const checkUpdate = vi.fn();
  return {
    checkUpdates: vi.fn(),
    value: {
      hasUpdate: false,
      updateInfo: null,
      isChecking: false,
      error: "network offline" as string | null,
      isDismissed: false,
      dismissUpdate: vi.fn(),
      checkUpdate,
      resetDismiss: vi.fn(),
    },
  };
});

afterEach(() => {
  mocks.value.checkUpdate.mockReset();
  mocks.checkUpdates.mockReset();
  vi.restoreAllMocks();
});

vi.mock("@/contexts/UpdateContext", () => ({
  useUpdate: () => mocks.value,
}));

vi.mock("@/lib/api", () => ({
  settingsApi: {
    get: vi.fn().mockResolvedValue({ receiveBetaUpdates: false }),
    getToolVersions: vi.fn().mockResolvedValue([]),
    openExternal: vi.fn(),
    checkUpdates: mocks.checkUpdates,
    installUpdateAndRestart: vi.fn(),
    probeToolInstallations: vi.fn().mockResolvedValue([]),
    runToolLifecycleAction: vi.fn(),
  },
  // 反馈入口按远端配置显隐；测试默认按「端点未下发」隐藏。
  feedbackApi: {
    isEndpointConfigured: vi.fn().mockResolvedValue(false),
    readClipboardImage: vi.fn(),
    submit: vi.fn(),
  },
  diagnosticsApi: {
    exportDiagnostics: vi.fn(),
  },
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("3.24.0"),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import { AboutSection } from "@/components/settings/AboutSection";

describe("AboutSection", () => {
  it("displays the current manual update-check error", async () => {
    render(<AboutSection isPortable={false} />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "network offline",
    );
  });

  it("keeps the official download fallback when a manual check rejects", async () => {
    const checkError = new Error("network offline");
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    mocks.value.checkUpdate.mockRejectedValue(checkError);
    render(<AboutSection isPortable={false} />);

    fireEvent.click(
      await screen.findByRole("button", {
        name: "settings.checkForUpdates",
      }),
    );

    await waitFor(() => expect(mocks.checkUpdates).toHaveBeenCalledOnce());
    expect(consoleError).toHaveBeenCalledWith(
      "[AboutSection] Update check failed",
      checkError,
    );
  });
});

describe("AboutSection manual update download progress", () => {
  it("shows percent and speed beside (not on) the updating button while chunks arrive", async () => {
    const settings = await import("@/lib/api");
    const event = await import("@tauri-apps/api/event");
    let handler:
      | ((e: { payload: { downloaded: number; total: number | null } }) => void)
      | undefined;
    vi.mocked(event.listen).mockImplementation(
      // 桩只关心 handler 本体；事件对象的其余字段与泛型收缩用 as 对齐。
      (async (_name: string, cb: never) => {
        handler = cb;
        return () => {};
      }) as unknown as typeof event.listen,
    );
    const emit = (payload: { downloaded: number; total: number | null }) =>
      handler?.({
        payload: {
          ...payload,
          requestId: vi
            .mocked(settings.settingsApi.installUpdateAndRestart)
            .mock.calls.at(-1)?.[1],
        },
      } as never);
    let now = 0;
    const nowSpy = vi.spyOn(performance, "now").mockImplementation(() => now);

    mocks.value = {
      ...mocks.value,
      hasUpdate: true,
      updateInfo: { availableVersion: "9.9.9" } as never,
      error: null,
    };
    vi.mocked(settings.settingsApi.installUpdateAndRestart).mockImplementation(
      () => new Promise<boolean>(() => {}),
    );

    render(<AboutSection isPortable={false} />);
    const download = await screen.findByRole("button", {
      name: /settings.updateTo/,
    });
    fireEvent.click(download);

    await waitFor(() =>
      expect(settings.settingsApi.installUpdateAndRestart).toHaveBeenCalled(),
    );

    await act(async () => {
      emit?.({ downloaded: 0, total: 10 * 1024 * 1024 });
    });
    now = 500;
    await act(async () => {
      emit?.({ downloaded: 2 * 1024 * 1024, total: 10 * 1024 * 1024 });
    });

    const updating = screen.getByRole("button", { name: /settings.updating/ });
    // 进度文案在按钮外的独立文本上（按钮正下方一行——2026-09-19 定调：
    // 不放右边，按钮与同行图标水平位置全程固定），按钮本体只有转圈与「更新中」。
    expect(updating).not.toHaveTextContent("20%");
    const status = screen.getByText(/20% · 4\.0 MB\/s/);
    expect(updating).not.toContainElement(status);
    // tabular-nums 防数字本身跳宽；不再需要 min-w 固宽（不在同一行，
    // 长短变化撑不到按钮——min-w-[19ch] 是旧「同行右放」布局的遗产）。
    expect(status.className).toContain("tabular-nums");

    nowSpy.mockRestore();
  });

  it("releases the busy button after a failed download and offers an explicit source retry", async () => {
    const { settingsApi } = await import("@/lib/api");
    vi.spyOn(console, "error").mockImplementation(() => {});
    mocks.value = {
      ...mocks.value,
      hasUpdate: true,
      updateInfo: { availableVersion: "9.9.9" } as never,
      error: null,
    };
    vi.mocked(settingsApi.installUpdateAndRestart)
      .mockReset()
      .mockRejectedValueOnce(new Error("download stalled"))
      .mockImplementationOnce(() => new Promise<boolean>(() => {}));
    render(<AboutSection isPortable={false} />);
    fireEvent.click(
      await screen.findByRole("button", { name: /settings.updateTo/ }),
    );

    const retry = await screen.findByRole("button", {
      name: "settings.updateRetryOfficial",
    });
    await waitFor(() => expect(retry).toBeEnabled());
    expect(screen.getByRole("alert")).toHaveTextContent("download stalled");
    expect(
      screen.queryByRole("button", { name: /settings.updating/ }),
    ).not.toBeInTheDocument();
    fireEvent.click(retry);
    await waitFor(() =>
      expect(settingsApi.installUpdateAndRestart).toHaveBeenLastCalledWith(
        "official",
        expect.any(String),
      ),
    );
  });

  it("resets speed when the same source switches transport and ignores an old request", async () => {
    const { settingsApi } = await import("@/lib/api");
    const { listen } = await import("@tauri-apps/api/event");
    let handler:
      ((event: { payload: Record<string, unknown> }) => void) | undefined;
    vi.mocked(listen).mockImplementation((async (
      _name: string,
      callback: never,
    ) => {
      handler = callback;
      return () => {};
    }) as unknown as typeof listen);
    vi.mocked(settingsApi.installUpdateAndRestart)
      .mockReset()
      .mockImplementation(() => new Promise<boolean>(() => {}));
    mocks.value = {
      ...mocks.value,
      hasUpdate: true,
      updateInfo: { availableVersion: "9.9.9" } as never,
      error: null,
    };
    let now = 0;
    vi.spyOn(performance, "now").mockImplementation(() => now);
    render(<AboutSection isPortable={false} />);
    fireEvent.click(
      await screen.findByRole("button", { name: /settings.updateTo/ }),
    );
    await waitFor(() =>
      expect(settingsApi.installUpdateAndRestart).toHaveBeenCalled(),
    );
    const requestId = vi.mocked(settingsApi.installUpdateAndRestart).mock
      .calls[0][1];
    const emit = (
      downloaded: number,
      phase = "downloading",
      request = requestId,
    ) =>
      handler?.({
        payload: {
          requestId: request,
          attemptId: 7,
          source: "official",
          downloaded,
          total: 10 * 1024 * 1024,
          phase,
        },
      });
    await act(async () => {
      emit(0);
    });
    now = 500;
    await act(async () => {
      emit(2 * 1024 * 1024);
    });
    expect(screen.getByText(/4\.0 MB\/s/)).toBeInTheDocument();
    await act(async () => {
      emit(0, "switchingSource");
    });
    expect(
      screen.getByText(/settings.updateSwitchingSource/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/MB\/s/)).not.toBeInTheDocument();
    await act(async () => {
      emit(9 * 1024 * 1024, "downloading", "old-request");
    });
    expect(screen.queryByText(/90%/)).not.toBeInTheDocument();
  });

  it("cleans up a late progress subscription after unmount without starting installation", async () => {
    const { settingsApi } = await import("@/lib/api");
    const { listen } = await import("@tauri-apps/api/event");
    const cleanup = vi.fn();
    let resolveListen!: (unlisten: () => void) => void;
    vi.mocked(listen).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveListen = resolve;
        }),
    );
    vi.mocked(settingsApi.installUpdateAndRestart).mockReset();
    mocks.value = {
      ...mocks.value,
      hasUpdate: true,
      updateInfo: { availableVersion: "9.9.9" } as never,
      error: null,
    };
    const { unmount } = render(<AboutSection isPortable={false} />);
    fireEvent.click(
      await screen.findByRole("button", { name: /settings.updateTo/ }),
    );
    await waitFor(() => expect(resolveListen).toBeDefined());
    unmount();
    await act(async () => {
      resolveListen(cleanup);
    });
    expect(cleanup).toHaveBeenCalledOnce();
    expect(settingsApi.installUpdateAndRestart).not.toHaveBeenCalled();
  });
});
