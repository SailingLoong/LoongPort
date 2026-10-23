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
const api = vi.hoisted(() => ({ context: vi.fn(), read: vi.fn() }));
vi.mock("@/lib/api/qoderHttpAuth", () => ({
  qoderHttpAuthApi: { context: api.context },
}));
vi.mock("@/lib/api/qoderCnCampaigns", () => ({
  qoderCnCampaignsApi: { read: api.read },
}));
const context = {
  providerId: "cn",
  credentialGeneration: "g1",
  source: "chinaQoderPat",
  status: "ready",
  allowedActions: [],
  expiresAtMs: null,
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
  automaticClaimEnabled: false,
  blockedReason: "daily_contract_unverified",
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
    expect(screen.getByText(/每日活动标识与轮次尚未确认/)).toBeInTheDocument();
    expect(screen.queryByText(/100/)).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /领取|自动/ }),
    ).not.toBeInTheDocument();
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
