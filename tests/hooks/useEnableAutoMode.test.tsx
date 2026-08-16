import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  useDisableAutoMode,
  useEnableAutoMode,
  useSetAutoModeAll,
} from "@/lib/query/autoMode";
import { createTestQueryClient } from "../utils/testQueryClient";

// 编排链路是 useProxyStatus → proxyApi/autoModeApi → invoke，在 invoke 层 mock
// 可以让真实的 mutation/react-query 层参与测试（与 ReconcileDialog 测试同一手法）。
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => false,
  invoke,
}));

// toast 副作用与本测试无关。
vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

function wrapper({ children }: { children: React.ReactNode }) {
  return (
    <QueryClientProvider client={createTestQueryClient()}>
      {children}
    </QueryClientProvider>
  );
}

const calls = () => invoke.mock.calls.map(([cmd]) => cmd as string);

/** 编排动作（排除 invalidate 触发的状态重拉，只看相对顺序）。 */
const orchestration = () =>
  calls().filter((cmd) =>
    [
      "start_proxy_server",
      "set_proxy_takeover_for_app",
      "set_auto_mode_enabled",
    ].includes(cmd),
  );

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case "get_proxy_status":
        // useProxyStatus 轮询读状态；编排前服务未运行。
        return { running: false };
      case "get_proxy_takeover_status":
        return {
          claude: false,
          codex: false,
          gemini: false,
          grokbuild: false,
          opencode: false,
          openclaw: false,
        };
      default:
        return {};
    }
  });
});

describe("useEnableAutoMode", () => {
  it("顺带补齐前置：起路由 → 接管该 app → 开自动模式", async () => {
    const { result } = renderHook(() => useEnableAutoMode(), { wrapper });

    await act(async () => {
      await result.current.mutateAsync({ appType: "codex" });
    });

    expect(orchestration()).toEqual([
      "start_proxy_server",
      "set_proxy_takeover_for_app",
      "set_auto_mode_enabled",
    ]);
    // 前置是现查的：动作前必有状态查询。
    expect(calls().indexOf("get_proxy_status")).toBeLessThan(
      calls().indexOf("start_proxy_server"),
    );
    expect(invoke).toHaveBeenCalledWith("set_proxy_takeover_for_app", {
      appType: "codex",
      enabled: true,
    });
    expect(invoke).toHaveBeenCalledWith("set_auto_mode_enabled", {
      appType: "codex",
      enabled: true,
    });
  });

  it("前置已满足时不重复起服务/接管，直接开自动模式", async () => {
    invoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "get_proxy_status":
          return { running: true };
        case "get_proxy_takeover_status":
          return {
            claude: true,
            codex: true,
            gemini: false,
            grokbuild: false,
            opencode: false,
            openclaw: false,
          };
        default:
          return {};
      }
    });

    const { result } = renderHook(() => useEnableAutoMode(), { wrapper });

    await act(async () => {
      await result.current.mutateAsync({ appType: "codex" });
    });

    const sequence = orchestration();
    expect(sequence).not.toContain("start_proxy_server");
    expect(sequence).not.toContain("set_proxy_takeover_for_app");
    expect(sequence).toContain("set_auto_mode_enabled");
  });

  it("中途失败即停：起服务失败不再往下接管/开启", async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "start_proxy_server") {
        throw new Error("port in use");
      }
      return {};
    });

    const { result } = renderHook(() => useEnableAutoMode(), { wrapper });

    await act(async () => {
      await expect(
        result.current.mutateAsync({ appType: "codex" }),
      ).rejects.toThrow("port in use");
    });

    const sequence = orchestration();
    expect(sequence).toContain("start_proxy_server");
    expect(sequence).not.toContain("set_proxy_takeover_for_app");
    expect(sequence).not.toContain("set_auto_mode_enabled");
    await waitFor(() => expect(result.current.isError).toBe(true));
  });

  it("关态编排：关省心模式后连该 app 的接管一并收回", async () => {
    invoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "get_proxy_status":
          return { running: true };
        case "get_proxy_takeover_status":
          return {
            claude: false,
            codex: true, // codex 正被接管
            gemini: false,
            grokbuild: false,
            opencode: false,
            openclaw: false,
          };
        default:
          return {};
      }
    });

    const { result } = renderHook(() => useDisableAutoMode(), { wrapper });

    await act(async () => {
      await result.current.mutateAsync({ appType: "codex" });
    });

    const sequence = orchestration();
    expect(sequence).toEqual([
      "set_auto_mode_enabled",
      "set_proxy_takeover_for_app",
    ]);
    expect(invoke).toHaveBeenCalledWith("set_auto_mode_enabled", {
      appType: "codex",
      enabled: false,
    });
    expect(invoke).toHaveBeenCalledWith("set_proxy_takeover_for_app", {
      appType: "codex",
      enabled: false,
    });
  });

  it("关态编排：该 app 未被接管时不再多余调收回", async () => {
    invoke.mockImplementation(async () => ({
      claude: false,
      codex: false,
      gemini: false,
      grokbuild: false,
      opencode: false,
      openclaw: false,
    }));

    const { result } = renderHook(() => useDisableAutoMode(), { wrapper });

    await act(async () => {
      await result.current.mutateAsync({ appType: "codex" });
    });

    const sequence = orchestration();
    expect(sequence).toEqual(["set_auto_mode_enabled"]);
  });

  it("总开关静默容错：CLI 未装的 app（接管报配置不存在）跳过，其余照开", async () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    invoke.mockImplementation(
      async (cmd: string, args?: { appType?: string }) => {
        switch (cmd) {
          case "get_proxy_status":
            return { running: true };
          case "get_proxy_takeover_status":
            return {
              claude: false,
              codex: false,
              gemini: false,
              grokbuild: false,
              opencode: false,
              openclaw: false,
            };
          case "set_proxy_takeover_for_app":
            // gemini CLI 没装：接管报「配置文件不存在」。
            if (args?.appType === "gemini") {
              throw new Error("Gemini .env 文件不存在");
            }
            return {};
          default:
            return {};
        }
      },
    );

    const { result } = renderHook(() => useSetAutoModeAll(), { wrapper });

    await act(async () => {
      // 不 reject：gemini 的失败被逐 app 吞掉，claude/codex 照常开。
      await result.current.mutateAsync({
        apps: ["claude", "codex", "gemini"],
        enable: true,
      });
    });

    const enabledCalls = invoke.mock.calls.filter(
      ([cmd]) => cmd === "set_auto_mode_enabled",
    );
    expect(enabledCalls).toHaveLength(2); // gemini 没走到 enable
    expect(enabledCalls.map(([, args]) => args)).toEqual(
      expect.arrayContaining([
        { appType: "claude", enabled: true },
        { appType: "codex", enabled: true },
      ]),
    );
    expect(warnSpy).toHaveBeenCalledWith(
      expect.stringContaining("gemini"),
      expect.anything(),
    );
    warnSpy.mockRestore();
  });
});
