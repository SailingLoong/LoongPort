import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import i18n from "i18next";
import zh from "@/i18n/locales/zh.json";
import { AutomaticClaimStatus } from "@/components/relay/accounts/AutomaticClaimStatus";
import { useAutomaticClaimStatus } from "@/hooks/useAutomaticClaimStatus";
import type {
  AutomaticClaimSnapshot,
  AutomaticClaimProvider,
} from "@/lib/api/automaticClaim";
import { emitTauriEvent } from "../msw/tauriMocks";
const api = vi.hoisted(() => ({ status: vi.fn() }));
vi.mock("@/lib/api/automaticClaim", () => ({ automaticClaimApi: api }));
const snapshot: AutomaticClaimSnapshot = {
  provider: "workbuddy",
  phase: "waiting",
  lastCheckedAt: 1780000000000,
  nextCheckAt: 1780000900000,
  result: "completed",
  reason: null,
};
beforeEach(async () => {
  api.status.mockReset().mockResolvedValue(snapshot);
  await i18n.changeLanguage("zh");
  i18n.addResourceBundle("zh", "translation", zh, true, true);
});
function View({
  provider = "workbuddy",
  changed = () => {},
}: {
  provider?: AutomaticClaimProvider;
  changed?: () => void;
}) {
  const state = useAutomaticClaimStatus(provider, changed);
  return <AutomaticClaimStatus {...state} />;
}
it("reads the initial backend snapshot and never labels completed checking as a successful claim", async () => {
  render(<View />);
  expect(await screen.findByText("最近结果：后台检查已完成")).toBeVisible();
  expect(screen.getByText(/检查完成不等于领取成功/)).toBeVisible();
  expect(screen.getByText(/最近检查：/)).toBeVisible();
  expect(screen.getByText(/下次计划检查：/)).toBeVisible();
  expect(screen.queryByText("已领取")).not.toBeInTheDocument();
  expect(api.status.mock.calls).toEqual([["workbuddy"], ["workbuddy"]]);
});
it("a matching event only reloads local state, ignores another platform, and unsubscribes", async () => {
  const changed = vi.fn();
  const view = render(<View changed={changed} />);
  await screen.findByText("最近结果：后台检查已完成");
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      ...snapshot,
      provider: "minimaxCode",
    }),
  );
  expect(api.status).toHaveBeenCalledTimes(2);
  api.status.mockResolvedValue({ ...snapshot, phase: "checking" });
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      ...snapshot,
      phase: "waiting",
    }),
  );
  expect(await screen.findByText("正在检查服务端资格")).toBeVisible();
  expect(changed).toHaveBeenCalledTimes(2);
  view.unmount();
  act(() => emitTauriEvent("automatic-claim-status-updated", snapshot));
  expect(api.status).toHaveBeenCalledTimes(3);
});
it("a late initial response cannot replace a newer event read", async () => {
  let finish!: (value: AutomaticClaimSnapshot) => void;
  api.status.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  render(<View />);
  api.status.mockResolvedValue({ ...snapshot, phase: "checking" });
  act(() => emitTauriEvent("automatic-claim-status-updated", snapshot));
  await screen.findByText("正在检查服务端资格");
  await act(async () => finish(snapshot));
  expect(screen.getByText("正在检查服务端资格")).toBeVisible();
});
it("failed reads hide stale checking and raw errors", async () => {
  api.status.mockResolvedValue({ ...snapshot, phase: "checking" });
  render(<View />);
  await screen.findByText("正在检查服务端资格");
  api.status.mockRejectedValue(new Error("SYNTHETIC_SECRET_CANARY"));
  act(() => emitTauriEvent("automatic-claim-status-updated", snapshot));
  await screen.findByText("后台检查状态暂不可用，请重新读取后台状态。");
  expect(screen.queryByText("正在检查服务端资格")).not.toBeInTheDocument();
  expect(screen.queryByText(/SYNTHETIC_SECRET_CANARY/)).not.toBeInTheDocument();
});
it.each([
  "locked",
  "disabled",
  "pending",
  "notEligible",
  "checkFailed",
] as const)(
  "shows the backend reason %s without inventing a schedule",
  async (reason) => {
    api.status.mockResolvedValue({
      ...snapshot,
      result: "skipped",
      reason,
      lastCheckedAt: null,
      nextCheckAt: null,
    });
    render(<View />);
    await waitFor(() =>
      expect(screen.getByText(zh.automaticStatus.reason[reason])).toBeVisible(),
    );
    expect(screen.queryByText(/最近检查：/)).not.toBeInTheDocument();
    expect(screen.queryByText(/下次计划检查：/)).not.toBeInTheDocument();
  },
);

it("does not schedule frontend polling while waiting for backend events", async () => {
  render(<View />);
  await screen.findByText("最近结果：后台检查已完成");
  vi.useFakeTimers();
  try {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30 * 60 * 1000);
    });
    expect(api.status).toHaveBeenCalledTimes(2);
  } finally {
    vi.useRealTimers();
  }
});

it("cannot show another provider's late snapshot after switching platforms", async () => {
  let finish!: (value: AutomaticClaimSnapshot) => void;
  api.status.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<View />);
  api.status.mockResolvedValue({
    ...snapshot,
    provider: "minimaxCode",
    phase: "checking",
  });
  view.rerender(<View provider="minimaxCode" />);
  await screen.findByText("正在检查服务端资格");
  await act(async () => finish(snapshot));
  expect(screen.getByText("正在检查服务端资格")).toBeVisible();
  expect(api.status.mock.calls).toEqual([
    ["workbuddy"],
    ["minimaxCode"],
    ["minimaxCode"],
  ]);
});

it("reloads only scheduler status after an unavailable read", async () => {
  const changed = vi.fn();
  api.status.mockRejectedValue(new Error("unavailable"));
  render(<View changed={changed} />);
  const reload = await screen.findByRole("button", {
    name: "重新读取后台状态",
  });
  api.status.mockResolvedValue(snapshot);
  changed.mockClear();
  await act(async () => reload.click());
  await screen.findByText("最近结果：后台检查已完成");
  expect(api.status.mock.calls).toEqual([
    ["workbuddy"],
    ["workbuddy"],
    ["workbuddy"],
  ]);
  expect(changed).not.toHaveBeenCalled();
});

it("closes the snapshot-to-listener gap with status and catalog reads after registration", async () => {
  const events = await import("@tauri-apps/api/event");
  let ready!: (off: () => void) => void;
  const subscription = vi.spyOn(events, "listen").mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        ready = resolve;
      }),
  );
  const changed = vi.fn();
  try {
    api.status.mockResolvedValueOnce({ ...snapshot, phase: "checking" });
    render(<View changed={changed} />);
    await screen.findByText("正在检查服务端资格");
    // Completion occurred before the native listener existed; no event can be received.
    api.status.mockResolvedValue(snapshot);
    await act(async () => ready(vi.fn()));
    expect(await screen.findByText("等待下一次检查")).toBeVisible();
    expect(changed).toHaveBeenCalledOnce();
    expect(api.status).toHaveBeenCalledTimes(2);
  } finally {
    subscription.mockRestore();
  }
});

it("does not re-read after registration resolves for an unmounted page", async () => {
  const events = await import("@tauri-apps/api/event");
  let ready!: (off: () => void) => void;
  const off = vi.fn();
  const subscription = vi.spyOn(events, "listen").mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        ready = resolve;
      }),
  );
  const changed = vi.fn();
  try {
    const view = render(<View changed={changed} />);
    await screen.findByText("等待下一次检查");
    view.unmount();
    await act(async () => ready(off));
    expect(off).toHaveBeenCalledOnce();
    expect(api.status).toHaveBeenCalledOnce();
    expect(changed).not.toHaveBeenCalled();
  } finally {
    subscription.mockRestore();
  }
});

it("retries a failed native subscription without claiming or exposing raw errors", async () => {
  const events = await import("@tauri-apps/api/event");
  const subscription = vi
    .spyOn(events, "listen")
    .mockRejectedValueOnce(new Error("SYNTHETIC_SUBSCRIPTION_SECRET"));
  try {
    render(<View />);
    const retry = await screen.findByRole("button", {
      name: "重新读取后台状态",
    });
    expect(
      screen.queryByText(/SYNTHETIC_SUBSCRIPTION_SECRET/),
    ).not.toBeInTheDocument();
    await act(async () => retry.click());
    await screen.findByText("等待下一次检查");
    expect(subscription).toHaveBeenCalledTimes(2);
  } finally {
    subscription.mockRestore();
  }
});
