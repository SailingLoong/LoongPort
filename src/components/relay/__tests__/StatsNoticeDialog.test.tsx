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

// id 生成做成确定值：断言「存了 id / 没覆盖已有 id」要钉具体值。
vi.mock("@/utils/uuid", () => ({
  generateUUID: () => "generated-test-id",
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

describe("StatsNoticeDialog：「知道了」的回写语义（纯告知形态，2026-09-09 拍板）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    statsEndpointConfigured.mockResolvedValue(true);
    get.mockResolvedValue(makeSettings());
  });

  it("写确认标记并生成 installId，不动 enabled", async () => {
    const captured: { saved?: Partial<Settings> } = {};
    save.mockImplementation(async (s: Partial<Settings>) => {
      captured.saved = s;
    });
    await renderOpenDialog();
    await clickOk();

    expect(save).toHaveBeenCalled();
    expect(captured.saved?.statsNoticeConfirmed).toBe(true);
    expect(captured.saved?.statsInstallId).toBe("generated-test-id");
    // 告知不承载表态：enabled 原样回写（未表态 ⇒ 字段缺省 ⇒ 后端默认 true）。
    expect(captured.saved?.enableAnonymousStats).toBeUndefined();
    expect(screen.queryByText("loongport.stats.body")).toBeNull();
  });

  it("已有 installId：不覆盖", async () => {
    const captured: { saved?: Partial<Settings> } = {};
    get.mockResolvedValue(makeSettings({ statsInstallId: "existing-test-id" }));
    save.mockImplementation(async (s: Partial<Settings>) => {
      captured.saved = s;
    });
    await renderOpenDialog();
    await clickOk();

    expect(captured.saved?.statsInstallId).toBe("existing-test-id");
  });
});
