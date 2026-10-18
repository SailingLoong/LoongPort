import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QoderDailyClaim } from "@/components/providers/qoder/QoderDailyClaim";
import type { QoderClaimContext, QoderClaimResult } from "@/lib/api/qoderDaily";
const api = vi.hoisted(() => ({ context: vi.fn(), claim: vi.fn() }));
vi.mock("@/lib/api/qoderDaily", () => ({ qoderDailyApi: api }));
const context: QoderClaimContext = {
  providerId: "fixture",
  credentialGeneration: "generation-a",
  deviceDependency: null,
};
const unknown: QoderClaimResult = {
  Observed: {
    action: "Get",
    outcome: "OutcomeUnknown",
    added_credits: null,
    operation: {
      attemptId: "attempt",
      campaignId: "old-window",
      campaignKey: "opaque",
      startAt: 100,
      endAt: 200,
    },
  },
};
async function open() {
  fireEvent.click(screen.getByRole("button", { name: "Qoder 每日积分" }));
  await waitFor(() => expect(api.context).toHaveBeenCalledWith("fixture"));
}
beforeEach(() => {
  api.context.mockReset().mockResolvedValue(context);
  api.claim.mockReset().mockResolvedValue(unknown);
});
describe("explicit Qoder daily claim", () => {
  it("does not fetch or claim during ordinary card rendering", () => {
    render(<QoderDailyClaim providerId="fixture" />);
    expect(api.context).not.toHaveBeenCalled();
    expect(api.claim).not.toHaveBeenCalled();
  });
  it("requires an explicit fresh declaration and sends the displayed generation", async () => {
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    expect(screen.getByRole("button", { name: "查询并领取" })).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox"));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "查询并领取" })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: "查询并领取" }));
    await waitFor(() => expect(api.claim).toHaveBeenCalledWith(context));
    expect(
      await screen.findByText(
        "领取结果尚未确认。再次点击只查询原记录，不重新提交。",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("old-window · 100–200")).toBeInTheDocument();
  });
  it.each([
    "OfficialDeviceUnavailable",
    "OfficialDeviceUnverified",
    "OfficialDeviceUnsupported",
  ])("blocks %s without invoking exchange/claim", async (dependency) => {
    api.context.mockResolvedValue({
      ...context,
      deviceDependency: dependency,
    });
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    fireEvent.click(screen.getByRole("checkbox"));
    await screen.findByText(
      dependency === "OfficialDeviceUnsupported"
        ? /目前仅支持已安装的 Qoder Desktop 0.4.8/
        : /当前无法领取/,
    );
    expect(screen.getByRole("button", { name: "查询并领取" })).toBeDisabled();
    expect(api.claim).not.toHaveBeenCalled();
  });
  it("does not silently replace the generation or submit twice while pending", async () => {
    let resolve!: (value: QoderClaimResult) => void;
    api.claim.mockImplementation(
      () =>
        new Promise<QoderClaimResult>((r) => {
          resolve = r;
        }),
    );
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    fireEvent.click(screen.getByRole("checkbox"));
    const button = screen.getByRole("button", { name: "查询并领取" });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    fireEvent.click(button);
    expect(api.claim).toHaveBeenCalledTimes(1);
    expect(api.claim).toHaveBeenCalledWith(context);
    resolve(unknown);
    await screen.findByText(/领取结果尚未确认/);
  });
  it("never shows 100 added for a receipt with unknown delta", async () => {
    api.claim.mockResolvedValue({
      ...unknown,
      Observed: { ...unknown.Observed, outcome: "ClaimedDeltaUnknown" },
    });
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    fireEvent.click(screen.getByRole("checkbox"));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "查询并领取" })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: "查询并领取" }));
    await screen.findByText("已确认领取记录，本次新增额度未确认。");
    expect(
      screen.queryByText("已确认本次新增 100 Credits。"),
    ).not.toBeInTheDocument();
  });
  it("clears declaration on close and reopen", async () => {
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("checkbox")).not.toBeInTheDocument(),
    );
    await open();
    expect(screen.getByRole("checkbox")).not.toBeChecked();
    expect(api.claim).not.toHaveBeenCalled();
  });
});
