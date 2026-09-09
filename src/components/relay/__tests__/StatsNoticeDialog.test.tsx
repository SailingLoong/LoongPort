import { act, fireEvent, render, screen } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * 「知道了」的回写语义。弹与不弹的前置条件在
 * tests/components/StatsNoticeDialogGate.test.tsx（渲染闸，唯源）；
 * 这里只钉按下按钮那一刻写了什么、没写什么。
 */
import { createTestQueryClient } from "../../../../tests/utils/testQueryClient";
import type { Settings } from "@/types";

import { StatsNoticeDialog } from "../StatsNoticeDialog";

const { get, save, statsEndpointConfigured } = vi.hoisted(() => ({
  get: vi.fn(),
  save: vi.fn(),
  statsEndpointConfigured: vi.fn(),
}));

// 组件从 "@/lib/api" 同时取 settingsApi 与 relayApi（re-export），mock 合在同一处。
vi.mock("@/lib/api", () => ({
  settingsApi: { get, save },
  relayApi: { statsEndpointConfigured },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { resolvedLanguage: "zh" },
  }),
}));

function makeSettings(overrides: Partial<Settings> = {}): Settings {
  return {
    minimizeToTrayOnClose: true,
    ...overrides,
  } as Settings;
}

async function renderOpenDialog() {
  // 前置条件凑齐（端点已配 + 没看过告知 + 统计开着）让弹窗打开，
  // 渲染并冲刷 mount 那条 promise 链（mock 都已 resolve）。
  let utils!: ReturnType<typeof render>;
  await act(async () => {
    utils = render(
      <QueryClientProvider client={createTestQueryClient()}>
        <StatsNoticeDialog />
      </QueryClientProvider>,
    );
  });
  return utils;
}

async function clickOk() {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "loongport.stats.ok" }));
  });
}

describe("StatsNoticeDialog：「知道了」的回写语义（纯告知，2026-09-09）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    statsEndpointConfigured.mockResolvedValue(true);
    get.mockResolvedValue(makeSettings());
  });

  it("只写确认标记：不动 enabled，也不生成 install id（id 归后端首次上报时自管）", async () => {
    const captured: { saved?: Partial<Settings> } = {};
    save.mockImplementation(async (s: Partial<Settings>) => {
      captured.saved = s;
    });
    await renderOpenDialog();
    await clickOk();

    expect(save).toHaveBeenCalled();
    expect(captured.saved?.statsNoticeConfirmed).toBe(true);
    // 告知不承载表态：enabled 原样回写（未表态 ⇒ 字段缺省 ⇒ 后端默认 true）。
    expect(captured.saved?.enableAnonymousStats).toBeUndefined();
    // id 的唯一写入者是后端上报任务 —— 这屏不许碰（第二个写入者=两个事实源）。
    expect(captured.saved?.statsInstallId).toBeUndefined();
    expect(screen.queryByText("loongport.stats.body")).toBeNull();
  });

  it("已有 install id（后端早已生成）：原样透传，不被丢弃", async () => {
    const captured: { saved?: Partial<Settings> } = {};
    get.mockResolvedValue(makeSettings({ statsInstallId: "backend-made-id" }));
    save.mockImplementation(async (s: Partial<Settings>) => {
      captured.saved = s;
    });
    await renderOpenDialog();
    await clickOk();

    expect(captured.saved?.statsNoticeConfirmed).toBe(true);
    expect(captured.saved?.statsInstallId).toBe("backend-made-id");
  });
});
