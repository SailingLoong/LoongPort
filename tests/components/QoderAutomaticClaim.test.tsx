import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClientProvider } from "@tanstack/react-query";
import { ProviderCard } from "@/components/providers/ProviderCard";
import type { Provider } from "@/types";
import { createTestQueryClient } from "../utils/testQueryClient";
import { QoderDailyClaim } from "@/components/providers/qoder/QoderDailyClaim";

const api = vi.hoisted(() => ({
  context: vi.fn(),
  claim: vi.fn(),
  automaticContext: vi.fn(),
  setAutomatic: vi.fn(),
}));
vi.mock("@/lib/api/qoderDaily", () => ({ qoderDailyApi: api }));
vi.mock("@/components/providers/qoder/QoderInferenceIdentity", () => ({
  QoderInferenceIdentity: () => null,
}));
vi.mock("@/components/providers/ProviderActions", () => ({
  ProviderActions: () => null,
}));
vi.mock("@/components/ProviderIcon", () => ({ ProviderIcon: () => null }));
vi.mock("@/components/UsageFooter", () => ({ default: () => null }));
vi.mock("@/components/SubscriptionQuotaFooter", () => ({
  default: () => null,
}));
vi.mock("@/components/CopilotQuotaFooter", () => ({ default: () => null }));
vi.mock("@/components/CodexOauthQuotaFooter", () => ({ default: () => null }));
vi.mock("@/components/XaiOauthQuotaFooter", () => ({ default: () => null }));
vi.mock("@/lib/query/failover", () => ({
  useProviderHealth: () => ({ data: undefined }),
}));
vi.mock("@/lib/query/queries", () => ({
  useUsageQuery: () => ({ data: undefined }),
}));
function providerCard(generation: string) {
  const meta = {
    providerType: "qoder_sdk",
    qoderCampaignState: { generation, pending: null },
  };
  const provider: Provider = {
    id: "fixture",
    name: "Synthetic Qoder",
    settingsConfig: {},
    meta,
  };
  return (
    <QueryClientProvider client={createTestQueryClient()}>
      <ProviderCard
        provider={provider}
        appId="codex"
        isCurrent={false}
        isProxyRunning={false}
        mutationsDisabled={false}
        onSwitch={vi.fn()}
        onEdit={vi.fn()}
        onDelete={vi.fn()}
        onConfigureUsage={vi.fn()}
        onOpenWebsite={vi.fn()}
        onDuplicate={vi.fn()}
      />
    </QueryClientProvider>
  );
}
const context = {
  providerId: "fixture",
  credentialGeneration: "generation-a",
  revision: 4,
  policyVersion: 1,
  enabled: false,
  canEnable: true,
  reason: null,
  subject: null,
};
const consentLabel = "确认国际版 Qoder PAT 并持续授权自动签到";
const toggleLabel = "自动签到";
const unknownMessage =
  "自动签到设置结果尚未确认。请刷新状态确认，期间不会重复提交设置。";
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
async function open() {
  fireEvent.click(screen.getByRole("button", { name: "Qoder 每日积分" }));
  await screen.findByRole("switch", { name: toggleLabel });
  await waitFor(() => expect(api.automaticContext).toHaveBeenCalled());
}
async function authorize() {
  const checkbox = await screen.findByRole("checkbox", { name: consentLabel });
  await waitFor(() => expect(checkbox).toBeEnabled());
  fireEvent.click(checkbox);
  await waitFor(() =>
    expect(screen.getByRole("switch", { name: toggleLabel })).toBeEnabled(),
  );
}
async function close() {
  fireEvent.keyDown(document, { key: "Escape" });
  await waitFor(() =>
    expect(screen.queryByRole("switch")).not.toBeInTheDocument(),
  );
}
beforeEach(() => {
  api.context.mockReset().mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-a",
    deviceDependency: null,
  });
  api.claim.mockReset();
  api.automaticContext.mockReset().mockResolvedValue(context);
  api.setAutomatic
    .mockReset()
    .mockResolvedValue({ ...context, revision: 5, enabled: true });
});

describe("Qoder automatic claim persistent consent", () => {
  it("never enables or reads automatic claim on an ordinary card mount", () => {
    render(<QoderDailyClaim providerId="fixture" />);
    expect(api.automaticContext).not.toHaveBeenCalled();
    expect(api.setAutomatic).not.toHaveBeenCalled();
    expect(api.claim).not.toHaveBeenCalled();
  });

  it("requires separate explicit continuous consent before enabling the displayed generation", async () => {
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    const toggle = screen.getByRole("switch", { name: toggleLabel });
    expect(toggle).not.toBeChecked();
    expect(toggle).toBeDisabled();
    expect(
      screen.getByRole("checkbox", { name: consentLabel }),
    ).not.toBeChecked();
    expect(screen.getByText(/运行期间定期查询并领取/)).toBeInTheDocument();
    expect(screen.getByText(/UTC\+8.*10:00/)).toBeInTheDocument();
    expect(
      screen.getByText(/仅支持.*0\.4\.8.*macOS ARM64/),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "确认当前凭据来源为国际版 Qoder PAT",
      }),
    );
    expect(toggle).toBeDisabled();
    await authorize();
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(api.setAutomatic).toHaveBeenCalledWith(context, true),
    );
    await waitFor(() => expect(toggle).toBeChecked());
    expect(api.claim).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("checkbox", { name: consentLabel }),
    ).not.toBeInTheDocument();
  });

  it("can disable without making a new PAT declaration", async () => {
    api.automaticContext.mockResolvedValue({
      ...context,
      enabled: true,
      canEnable: false,
    });
    api.setAutomatic.mockResolvedValue({
      ...context,
      revision: 5,
      enabled: false,
      canEnable: false,
    });
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    const toggle = screen.getByRole("switch", { name: toggleLabel });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(api.setAutomatic).toHaveBeenCalledWith(
        expect.objectContaining({ enabled: true }),
        false,
      ),
    );
    await waitFor(() => expect(toggle).not.toBeChecked());
    expect(api.claim).not.toHaveBeenCalled();
  });

  it("keeps backend eligibility authoritative", async () => {
    api.automaticContext.mockResolvedValue({
      ...context,
      canEnable: false,
      reason: "OfficialDeviceUnsupported",
    });
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    expect(screen.getByRole("checkbox", { name: consentLabel })).toBeDisabled();
    expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
    expect(api.setAutomatic).not.toHaveBeenCalled();
  });

  it("clears unsubmitted consent on close and reopen", async () => {
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    await authorize();
    await close();
    await open();
    expect(
      screen.getByRole("checkbox", { name: consentLabel }),
    ).not.toBeChecked();
    expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
    expect(api.setAutomatic).not.toHaveBeenCalled();
  });

  it("clears prior consent when a refreshed backend generation changes", async () => {
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    await authorize();
    api.automaticContext.mockResolvedValue({
      ...context,
      credentialGeneration: "generation-b",
      revision: 5,
    });
    fireEvent.click(screen.getByRole("button", { name: "刷新自动签到状态" }));
    await waitFor(() => expect(api.automaticContext).toHaveBeenCalledTimes(2));
    expect(
      screen.getByRole("checkbox", { name: consentLabel }),
    ).not.toBeChecked();
    expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
    await authorize();
    api.setAutomatic.mockResolvedValue({
      ...context,
      credentialGeneration: "generation-b",
      revision: 6,
      enabled: true,
    });
    fireEvent.click(screen.getByRole("switch", { name: toggleLabel }));
    await waitFor(() =>
      expect(api.setAutomatic).toHaveBeenCalledWith(
        expect.objectContaining({
          credentialGeneration: "generation-b",
          revision: 5,
        }),
        true,
      ),
    );
  });

  it("reads back once after a lost setter response without retrying the setter", async () => {
    api.setAutomatic.mockRejectedValue(new Error("lost response"));
    api.automaticContext
      .mockResolvedValueOnce(context)
      .mockResolvedValue({ ...context, revision: 5, enabled: true });
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    await authorize();
    fireEvent.click(screen.getByRole("switch", { name: toggleLabel }));
    await waitFor(() => expect(api.automaticContext).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(screen.getByRole("switch", { name: toggleLabel })).toBeChecked(),
    );
    expect(api.setAutomatic).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(unknownMessage)).not.toBeInTheDocument();
  });

  it.each([3, 4])(
    "keeps revision %s readback unknown and blocked across reopening",
    async (revision) => {
      api.setAutomatic.mockRejectedValue(new Error("lost response"));
      api.automaticContext
        .mockResolvedValueOnce(context)
        .mockResolvedValue({ ...context, revision });
      render(<QoderDailyClaim providerId="fixture" />);
      await open();
      await authorize();
      fireEvent.click(screen.getByRole("switch", { name: toggleLabel }));
      await screen.findByText(unknownMessage);
      expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
      await close();
      await open();
      expect(await screen.findByText(unknownMessage)).toBeInTheDocument();
      expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
      api.automaticContext.mockResolvedValue({
        ...context,
        revision: 5,
        enabled: true,
      });
      fireEvent.click(screen.getByRole("button", { name: "刷新自动签到状态" }));
      await waitFor(() =>
        expect(screen.getByRole("switch", { name: toggleLabel })).toBeEnabled(),
      );
      expect(screen.getByRole("switch", { name: toggleLabel })).toBeChecked();
      expect(api.setAutomatic).toHaveBeenCalledTimes(1);
    },
  );

  it.each([
    { ...context, revision: 5, providerId: "other" },
    { ...context, revision: 5, credentialGeneration: "generation-b" },
    { ...context, revision: 5, policyVersion: 2 },
  ])(
    "does not treat a mismatched readback as confirmation",
    async (readback) => {
      api.setAutomatic.mockRejectedValue(new Error("lost response"));
      api.automaticContext
        .mockResolvedValueOnce(context)
        .mockResolvedValue(readback);
      render(<QoderDailyClaim providerId="fixture" />);
      await open();
      await authorize();
      fireEvent.click(screen.getByRole("switch", { name: toggleLabel }));
      await screen.findByText(unknownMessage);
      expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
      expect(api.setAutomatic).toHaveBeenCalledTimes(1);
    },
  );

  it("keeps a failed readback unknown instead of restoring a writable old toggle", async () => {
    api.setAutomatic.mockRejectedValue(new Error("lost response"));
    api.automaticContext
      .mockResolvedValueOnce(context)
      .mockRejectedValue(new Error("readback unavailable"));
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    await authorize();
    fireEvent.click(screen.getByRole("switch", { name: toggleLabel }));
    await screen.findByText(unknownMessage);
    expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
    expect(api.automaticContext).toHaveBeenCalledTimes(2);
    expect(api.setAutomatic).toHaveBeenCalledTimes(1);
  });

  it("does not let an old reopening read overwrite a newer mutation snapshot", async () => {
    const mutation = deferred<typeof context>();
    const oldRead = deferred<typeof context>();
    api.setAutomatic.mockReturnValue(mutation.promise);
    api.automaticContext
      .mockResolvedValueOnce(context)
      .mockReturnValue(oldRead.promise);
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    await authorize();
    fireEvent.click(screen.getByRole("switch", { name: toggleLabel }));
    await close();
    await open();
    await act(async () =>
      mutation.resolve({ ...context, revision: 5, enabled: true }),
    );
    await act(async () => oldRead.resolve(context));
    const toggle = screen.getByRole("switch", { name: toggleLabel });
    expect(toggle).toBeChecked();
    expect(toggle).toBeEnabled();
    expect(api.setAutomatic).toHaveBeenCalledTimes(1);
  });

  it("rejects duplicate clicks while the only mutation is pending", async () => {
    const mutation = deferred<typeof context>();
    api.setAutomatic.mockReturnValue(mutation.promise);
    render(<QoderDailyClaim providerId="fixture" />);
    await open();
    await authorize();
    const toggle = screen.getByRole("switch", { name: toggleLabel });
    fireEvent.click(toggle);
    fireEvent.click(toggle);
    expect(api.setAutomatic).toHaveBeenCalledTimes(1);
    await act(async () =>
      mutation.resolve({ ...context, revision: 5, enabled: true }),
    );
  });

  it("respects mutation blocking even after the dialog is already open", async () => {
    const view = render(<QoderDailyClaim providerId="fixture" />);
    await open();
    await authorize();
    view.rerender(<QoderDailyClaim providerId="fixture" disabled />);
    expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
    expect(
      screen.getByRole("checkbox", { name: consentLabel }),
    ).not.toBeChecked();
    expect(api.setAutomatic).not.toHaveBeenCalled();
  });

  it("uses a recreated provider's off revision zero without remounting the ProviderCard dialog owner", async () => {
    api.automaticContext.mockResolvedValue({ ...context, enabled: true });
    const view = render(providerCard("generation-a"));
    await open();
    await waitFor(() =>
      expect(screen.getByRole("switch", { name: toggleLabel })).toBeChecked(),
    );
    api.automaticContext.mockResolvedValue({
      ...context,
      credentialGeneration: "generation-b",
      revision: 0,
    });
    view.rerender(providerCard("generation-b"));
    // ProviderCard has no generation key on its Daily dialog. The open dialog
    // proves this is a new backend owner, not a remount clearing local state.
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "刷新自动签到状态" }));
    await waitFor(() =>
      expect(
        screen.getByRole("switch", { name: toggleLabel }),
      ).not.toBeChecked(),
    );
    expect(
      screen.getByRole("checkbox", { name: consentLabel }),
    ).not.toBeChecked();
    await close();
    await open();
    expect(screen.getByRole("switch", { name: toggleLabel })).not.toBeChecked();
    await authorize();
    api.setAutomatic.mockResolvedValue({
      ...context,
      credentialGeneration: "generation-b",
      revision: 1,
      enabled: true,
    });
    fireEvent.click(screen.getByRole("switch", { name: toggleLabel }));
    await waitFor(() =>
      expect(api.setAutomatic).toHaveBeenCalledWith(
        expect.objectContaining({
          credentialGeneration: "generation-b",
          revision: 0,
        }),
        true,
      ),
    );
  });

  it("shows a recreated provider off but preserves the lost old setter's unknown barrier", async () => {
    api.automaticContext.mockResolvedValueOnce({ ...context, enabled: true });
    api.setAutomatic.mockRejectedValue(new Error("lost old-owner response"));
    const view = render(providerCard("generation-a"));
    await open();
    await waitFor(() =>
      expect(screen.getByRole("switch", { name: toggleLabel })).toBeEnabled(),
    );
    api.automaticContext.mockResolvedValue({
      ...context,
      credentialGeneration: "generation-b",
      revision: 0,
    });
    fireEvent.click(screen.getByRole("switch", { name: toggleLabel }));
    view.rerender(providerCard("generation-b"));
    await screen.findByText(unknownMessage);
    expect(screen.getByRole("switch", { name: toggleLabel })).not.toBeChecked();
    expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
    expect(
      screen.getByRole("checkbox", { name: consentLabel }),
    ).not.toBeChecked();
    await close();
    await open();
    expect(await screen.findByText(unknownMessage)).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: toggleLabel })).not.toBeChecked();
    expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
    expect(api.setAutomatic).toHaveBeenCalledTimes(1);
  });

  it("does not resurrect the old generation from a setter response after reading a recreated provider", async () => {
    const mutation = deferred<typeof context>();
    api.setAutomatic.mockReturnValue(mutation.promise);
    const view = render(providerCard("generation-a"));
    await open();
    await authorize();
    fireEvent.click(screen.getByRole("switch", { name: toggleLabel }));
    api.automaticContext.mockResolvedValue({
      ...context,
      credentialGeneration: "generation-b",
      revision: 0,
    });
    view.rerender(providerCard("generation-b"));
    await close();
    await open();
    await waitFor(() => expect(api.automaticContext).toHaveBeenCalledTimes(2));
    await act(async () =>
      mutation.resolve({ ...context, revision: 5, enabled: true }),
    );
    expect(await screen.findByText(unknownMessage)).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: toggleLabel })).not.toBeChecked();
    expect(screen.getByRole("switch", { name: toggleLabel })).toBeDisabled();
    expect(api.setAutomatic).toHaveBeenCalledTimes(1);
  });
});
