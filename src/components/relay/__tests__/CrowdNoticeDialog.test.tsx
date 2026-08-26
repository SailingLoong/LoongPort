import { act, fireEvent, render, screen } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createTestQueryClient } from "../../../../tests/utils/testQueryClient";
import type { Settings } from "@/types";

import {
  CROWD_NOTICE_OPEN_EVENT,
  CrowdNoticeDialog,
} from "../CrowdNoticeDialog";

const { get, save } = vi.hoisted(() => ({ get: vi.fn(), save: vi.fn() }));

vi.mock("@/lib/api", () => ({
  settingsApi: { get, save },
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

function renderDialog() {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      <CrowdNoticeDialog />
    </QueryClientProvider>,
  );
}

/** 推进假时钟并冲刷其间结算的微任务（mock 的 get 是已 resolve 的 promise）。 */
async function tick(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("CrowdNoticeDialog 主动告知（维护者 2026-08-26 拍板：同意先于功能可见）", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    save.mockReset();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("未表态：启动延迟后主动弹一次", async () => {
    get.mockResolvedValue(makeSettings()); // crowdMetricsNoticeConfirmed === undefined
    renderDialog();
    expect(screen.queryByText("loongport.crowd.notice.title")).toBeNull();

    await tick(5_100);
    expect(screen.getByText("loongport.crowd.notice.title")).toBeTruthy();
  });

  it("表过态（含拒绝过）：启动后不主动弹", async () => {
    get.mockResolvedValue(makeSettings({ crowdMetricsNoticeConfirmed: true }));
    renderDialog();
    await tick(6_000);
    expect(screen.queryByText("loongport.crowd.notice.title")).toBeNull();
  });

  it("广场再入口事件可以唤起（拒绝过的用户改主意）", async () => {
    get.mockResolvedValue(makeSettings({ crowdMetricsNoticeConfirmed: true }));
    renderDialog();
    await tick(6_000);
    expect(screen.queryByText("loongport.crowd.notice.title")).toBeNull();

    act(() => {
      window.dispatchEvent(new Event(CROWD_NOTICE_OPEN_EVENT));
    });
    await tick(0);
    expect(screen.getByText("loongport.crowd.notice.title")).toBeTruthy();
  });

  it("拒绝也写 confirmed（之后不再主动弹），且 enable=false", async () => {
    // 对象持有者：闭包内赋值的 let 会被 TS 流分析窄化成 null，读不出来。
    const captured: { saved?: Partial<Settings> } = {};
    get.mockResolvedValue(makeSettings());
    save.mockImplementation(async (s: Partial<Settings>) => {
      captured.saved = s;
    });
    renderDialog();
    await tick(5_100);

    // fireEvent 而非 userEvent：假时钟下指针模拟的 setTimeout 链会死锁，
    // 这里只要 onClick 触发 respond()。
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "loongport.crowd.notice.decline" }),
      );
      await vi.advanceTimersByTimeAsync(0);
    });

    expect(save).toHaveBeenCalled();
    expect(captured.saved?.crowdMetricsNoticeConfirmed).toBe(true);
    expect(captured.saved?.crowdMetricsEnabled).toBe(false);
    expect(screen.queryByText("loongport.crowd.notice.title")).toBeNull();
  });
});
