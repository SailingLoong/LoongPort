import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QoderCnCampaigns } from "@/components/providers/qoder/QoderCnCampaigns";
import type { QoderCnCampaignReadResult } from "@/lib/api/qoderCnCampaigns";
const api = vi.hoisted(() => ({
  context: vi.fn(),
  read: vi.fn(),
  claim: vi.fn(),
  automaticContext: vi.fn(),
  setAutomatic: vi.fn(),
}));
vi.mock("@/lib/api/qoderHttpAuth", () => ({
  qoderHttpAuthApi: { context: api.context },
}));
vi.mock("@/lib/api/qoderCnCampaigns", () => ({
  qoderCnCampaignsApi: {
    read: api.read,
    claim: api.claim,
    automaticContext: api.automaticContext,
    setAutomatic: api.setAutomatic,
  },
}));
const context = {
  providerId: "cn",
  credentialGeneration: "g1",
  source: "chinaQoderPat",
  status: "ready",
  allowedActions: [],
  expiresAtMs: null,
};
const policy = {
  providerId: "cn",
  credentialGeneration: "g1",
  revision: 0,
  policyVersion: 2,
  enabled: false,
  canEnable: true,
  reason: null,
  subject: null,
};
const completed = {
  status: "completed",
  providerId: "cn",
  credentialGeneration: "g1",
  activities: [
    {
      campaignId: "free.one",
      outcome: "claimedDeltaUnknown",
      reason: null,
      benefitAmount: 100,
    },
    {
      campaignId: "free.two",
      outcome: "unknown",
      reason: "response_unknown",
      benefitAmount: null,
    },
  ],
};
const observed: QoderCnCampaignReadResult = {
  status: "observed",
  providerId: "cn",
  credentialGeneration: "g1",
  source: "chinaQoderPat",
  campaigns: [
    {
      campaignId: "synthetic-benefit",
      campaignKey: null,
      claimStatus: "claimed",
      startAt: null,
      endAt: null,
      benefitAmount: 100,
    },
  ],
  runtimeIdentity: "not_requested",
};
function deferred<T>() {
  let resolve!: (v: T) => void;
  return {
    promise: new Promise<T>((r) => {
      resolve = r;
    }),
    resolve: (v: T) => resolve(v),
  };
}
async function open() {
  fireEvent.click(screen.getByRole("button", { name: "CN 活动状态" }));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "查询活动状态" })).toBeEnabled(),
  );
}
async function query() {
  fireEvent.click(screen.getByRole("button", { name: "查询活动状态" }));
}
beforeEach(() => {
  api.context.mockReset().mockResolvedValue(context);
  api.read.mockReset().mockResolvedValue(observed);
  api.claim.mockReset().mockResolvedValue(completed);
  api.automaticContext.mockReset().mockResolvedValue(policy);
  api.setAutomatic
    .mockReset()
    .mockImplementation(async (_context, enabled) => ({
      ...policy,
      enabled,
      revision: 1,
    }));
});
describe("explicit original-owner CN campaign reads", () => {
  it("stays inert while the preserved account view is disabled", () => {
    render(<QoderCnCampaigns providerId="cn" disabled />);
    expect(screen.getByRole("button", { name: "CN 活动状态" })).toBeDisabled();
    expect(api.context).not.toHaveBeenCalled();
    expect(api.read).not.toHaveBeenCalled();
  });
  it.each([{ providerId: "other" }, { credentialGeneration: "g2" }])(
    "does not query when local context no longer matches the displayed source: %s",
    async (mismatch) => {
      api.context.mockResolvedValue({ ...context, ...mismatch });
      render(<QoderCnCampaigns providerId="cn" sourceGeneration="g1" />);
      fireEvent.click(screen.getByRole("button", { name: "CN 活动状态" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("状态已变化");
      expect(
        screen.getByRole("button", { name: "查询活动状态" }),
      ).toBeDisabled();
      expect(api.read).not.toHaveBeenCalled();
    },
  );
  it("does not query on mount or open; only explicit click queries the current source", async () => {
    render(<QoderCnCampaigns providerId="cn" sourceGeneration="g1" />);
    expect(api.context).not.toHaveBeenCalled();
    expect(api.read).not.toHaveBeenCalled();
    await open();
    expect(api.read).not.toHaveBeenCalled();
    await query();
    expect(api.read).toHaveBeenCalledExactlyOnceWith({
      providerId: "cn",
      credentialGeneration: "g1",
      source: "chinaQoderPat",
    });
    expect(await screen.findByText("已领取（活动状态）")).toBeInTheDocument();
    expect(
      screen.getByText(/不代表今日签到成功或本次新增积分/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/不会购买、订阅、扣积分或完成额外任务/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/100/)).not.toBeInTheDocument();
    expect(api.claim).not.toHaveBeenCalled();
    expect(api.setAutomatic).not.toHaveBeenCalled();
  });
  it("routes independent device identity without pretending to supply official runtime", async () => {
    api.context.mockResolvedValue({ ...context, source: "chinaQoderDevice" });
    api.read.mockResolvedValue({
      ...observed,
      source: "chinaQoderDevice",
      campaigns: [],
    });
    render(<QoderCnCampaigns providerId="cn" />);
    await open();
    await query();
    expect(api.read).toHaveBeenCalledWith(
      expect.objectContaining({ source: "chinaQoderDevice" }),
    );
    expect(
      await screen.findByText(/未返回可展示的福利活动/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/未读取或提交官方客户端运行时身份/),
    ).toBeInTheDocument();
  });
  it.each(["internationalQoderPat", null])(
    "does not send %s as a CN declaration",
    async (source) => {
      api.context.mockResolvedValue({ ...context, source });
      render(<QoderCnCampaigns providerId="cn" />);
      fireEvent.click(screen.getByRole("button", { name: "CN 活动状态" }));
      expect(await screen.findByRole("alert")).toHaveTextContent(
        "当前 CN 凭据来源不可用",
      );
      expect(
        screen.getByRole("button", { name: "查询活动状态" }),
      ).toBeDisabled();
      expect(api.read).not.toHaveBeenCalled();
    },
  );
  it("does not submit twice while the same read is unresolved", async () => {
    const pending = deferred<QoderCnCampaignReadResult>();
    api.read.mockReturnValue(pending.promise);
    render(<QoderCnCampaigns providerId="cn" />);
    await open();
    const button = screen.getByRole("button", { name: "查询活动状态" });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(api.read).toHaveBeenCalledTimes(1);
    await act(async () => pending.resolve(observed));
  });
  it("discards a reply after closing and does not autoquery on reopening", async () => {
    const pending = deferred<QoderCnCampaignReadResult>();
    api.read.mockReturnValue(pending.promise);
    render(<QoderCnCampaigns providerId="cn" />);
    await open();
    await query();
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    await act(async () => pending.resolve(observed));
    await open();
    expect(screen.queryByText("synthetic-benefit")).not.toBeInTheDocument();
    expect(api.read).toHaveBeenCalledTimes(1);
  });
  it.each(["generation", "provider", "disabled"])(
    "discards a late result after %s changes",
    async (change) => {
      const pending = deferred<QoderCnCampaignReadResult>();
      api.read.mockReturnValue(pending.promise);
      const view = render(
        <QoderCnCampaigns providerId="cn" sourceGeneration="g1" />,
      );
      await open();
      await query();
      api.context.mockResolvedValue({
        ...context,
        providerId: change === "provider" ? "other" : "cn",
        credentialGeneration: "g2",
      });
      view.rerender(
        <QoderCnCampaigns
          providerId={change === "provider" ? "other" : "cn"}
          sourceGeneration="g2"
          disabled={change === "disabled"}
        />,
      );
      await act(async () => pending.resolve(observed));
      expect(screen.queryByText("synthetic-benefit")).not.toBeInTheDocument();
    },
  );
  it.each([
    { providerId: "other" },
    { credentialGeneration: "g2" },
    { source: "chinaQoderDevice" },
  ])("rejects response binding mismatch %s", async (mismatch) => {
    api.read.mockResolvedValue({ ...observed, ...mismatch });
    render(<QoderCnCampaigns providerId="cn" />);
    await open();
    await query();
    expect(await screen.findByRole("alert")).toHaveTextContent("状态已变化");
    expect(screen.queryByText("synthetic-benefit")).not.toBeInTheDocument();
  });
  it.each([
    "identity_expired",
    "identity_pending",
    "verification_required",
    "invalid_response",
  ])("displays safe diagnostic %s with no refresh or claim", async (code) => {
    api.read.mockResolvedValue({ status: "failed", code });
    render(<QoderCnCampaigns providerId="cn" />);
    await open();
    await query();
    expect(await screen.findByRole("alert")).not.toHaveTextContent(code);
    expect(api.read).toHaveBeenCalledTimes(1);
  });
  it("does not expose raw transport errors", async () => {
    api.read.mockRejectedValue(new Error("synthetic-sensitive-wire-body"));
    render(<QoderCnCampaigns providerId="cn" />);
    await open();
    await query();
    expect(await screen.findByRole("alert")).toHaveTextContent("查询未完成");
    expect(screen.queryByText(/synthetic-sensitive/)).not.toBeInTheDocument();
  });
});

describe("explicit free-benefit actions and continuing opt-in", () => {
  it("stays default off on open; manual batch uses the original source and never displays +100", async () => {
    render(<QoderCnCampaigns providerId="cn" />);
    await open();
    expect(api.claim).not.toHaveBeenCalled();
    expect(api.setAutomatic).not.toHaveBeenCalled();
    expect(
      await screen.findByRole("button", { name: "开启自动领取免费福利" }),
    ).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "领取免费福利" }));
    expect(api.claim).toHaveBeenCalledExactlyOnceWith({
      providerId: "cn",
      credentialGeneration: "g1",
      source: "chinaQoderPat",
    });
    expect(await screen.findByText("free.one")).toBeInTheDocument();
    expect(
      screen.getByText("已确认领取状态，本次新增数量未知"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("结果未确认，仅查询恢复，不会重复领取"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/100/)).not.toBeInTheDocument();
  });
  it("enables only after explicit click and disables through the same revision owner", async () => {
    api.context.mockResolvedValue({ ...context, source: "chinaQoderDevice" });
    render(<QoderCnCampaigns providerId="cn" />);
    await open();
    fireEvent.click(
      await screen.findByRole("button", { name: "开启自动领取免费福利" }),
    );
    expect(api.setAutomatic).toHaveBeenCalledWith(
      policy,
      true,
      "chinaQoderDevice",
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "关闭自动领取免费福利" }),
    );
    expect(api.setAutomatic).toHaveBeenLastCalledWith(
      { ...policy, enabled: true, revision: 1 },
      false,
      "chinaQoderDevice",
    );
    expect(api.claim).not.toHaveBeenCalled();
  });
  it("does not replay or display raw errors after ambiguous policy IPC", async () => {
    api.setAutomatic.mockRejectedValue(new Error("SYNTHETIC-SECRET"));
    render(<QoderCnCampaigns providerId="cn" />);
    await open();
    fireEvent.click(
      await screen.findByRole("button", { name: "开启自动领取免费福利" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "设置结果未确认",
    );
    expect(api.setAutomatic).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/SYNTHETIC/)).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "开启自动领取免费福利" }),
    ).toBeDisabled();
  });
  it.each(["provider", "generation", "disabled"])(
    "discards late claim after %s change",
    async (change) => {
      const pending = deferred<typeof completed>();
      api.claim.mockReturnValue(pending.promise);
      const view = render(
        <QoderCnCampaigns providerId="cn" sourceGeneration="g1" />,
      );
      await open();
      fireEvent.click(screen.getByRole("button", { name: "领取免费福利" }));
      view.rerender(
        <QoderCnCampaigns
          providerId={change === "provider" ? "other" : "cn"}
          sourceGeneration={change === "generation" ? "g2" : "g1"}
          disabled={change === "disabled"}
        />,
      );
      await act(async () => pending.resolve(completed));
      expect(screen.queryByText("free.one")).not.toBeInTheDocument();
    },
  );
  it("single-flights a batch and rejects mismatching owner results", async () => {
    const pending = deferred<typeof completed>();
    api.claim.mockReturnValue(pending.promise);
    render(<QoderCnCampaigns providerId="cn" />);
    await open();
    const button = screen.getByRole("button", { name: "领取免费福利" });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(api.claim).toHaveBeenCalledTimes(1);
    await act(async () =>
      pending.resolve({ ...completed, credentialGeneration: "g2" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("状态已变化");
    expect(screen.queryByText("free.one")).not.toBeInTheDocument();
  });
});

it("can revoke automatic consent during a batch and discards its late reply", async () => {
  api.automaticContext.mockResolvedValue({ ...policy, enabled: true });
  const pending = deferred<typeof completed>();
  api.claim.mockReturnValue(pending.promise);
  render(<QoderCnCampaigns providerId="cn" />);
  await open();
  fireEvent.click(screen.getByRole("button", { name: "领取免费福利" }));
  const disable = await screen.findByRole("button", {
    name: "关闭自动领取免费福利",
  });
  expect(disable).toBeEnabled();
  fireEvent.click(disable);
  await screen.findByRole("button", { name: "开启自动领取免费福利" });
  await act(async () => pending.resolve(completed));
  expect(screen.queryByText("free.one")).not.toBeInTheDocument();
});
it("explains why a previously uncertain no-window claim cannot automatically reopen", async () => {
  api.claim.mockResolvedValue({
    ...completed,
    activities: [
      {
        campaignId: "unconfirmed-old",
        outcome: "skipped",
        reason: "reconciled_no_new_round",
        benefitAmount: null,
      },
    ],
  });
  render(<QoderCnCampaigns providerId="cn" />);
  await open();
  fireEvent.click(screen.getByRole("button", { name: "领取免费福利" }));
  expect(
    await screen.findByText(/缺少明确新活动区间，不能自动重新领取/),
  ).toBeInTheDocument();
});
