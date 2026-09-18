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

  it("keeps the Releases fallback when a manual check rejects", async () => {
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
      handler?.({ payload });
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

    await act(async () => {
      emit?.({ downloaded: 0, total: 10 * 1024 * 1024 });
    });
    now = 500;
    await act(async () => {
      emit?.({ downloaded: 2 * 1024 * 1024, total: 10 * 1024 * 1024 });
    });

    const updating = screen.getByRole("button", { name: /settings.updating/ });
    // 进度文案在按钮外的独立文本上（对比度 + 按钮宽度稳定），按钮本体只有转圈与「更新中」。
    expect(updating).not.toHaveTextContent("20%");
    const status = screen.getByText(/20% · 4\.0 MB\/s/);
    expect(updating).not.toContainElement(status);
    // 固定最小宽度：`9%`↔`100%`、KB/s↔MB/s 的长短变化全被盒子吃掉，
    // 按钮和图标不随进度事件横移（2026-09-17 用户报「icon 飘」）。
    expect(status.className).toContain("min-w-[19ch]");
    expect(status.className).toContain("tabular-nums");

    nowSpy.mockRestore();
  });
});
