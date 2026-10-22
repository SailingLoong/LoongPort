import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { QoderAccountPanel } from "@/components/relay/accounts/QoderAccountPanel";
const api = vi.hoisted(() => ({
  context: vi.fn(),
  action: vi.fn(),
  claimContext: vi.fn(),
  claim: vi.fn(),
  automaticContext: vi.fn(),
  setAutomatic: vi.fn(),
}));
vi.mock("@/lib/api/providers", () => ({
  providersApi: {
    getAll: vi.fn(async () => ({
      one: {
        id: "one",
        name: "Original Qoder",
        settingsConfig: {},
        meta: {
          providerType: "qoder_sdk",
          qoderCampaignState: { generation: "r1" },
        },
      },
    })),
  },
}));
vi.mock("@/lib/api/qoderHttpAuth", () => ({
  qoderHttpAuthApi: { context: api.context, action: api.action },
}));
vi.mock("@/lib/api/qoderDaily", () => ({
  qoderDailyApi: {
    context: api.claimContext,
    claim: api.claim,
    automaticContext: api.automaticContext,
    setAutomatic: api.setAutomatic,
  },
}));
it("opens the actual original identity and claim dialogs, with no implicit authentication or claim", async () => {
  api.context.mockResolvedValue({
    providerId: "one",
    status: "unavailable",
    allowedActions: [],
  });
  api.claimContext.mockResolvedValue({
    providerId: "one",
    credentialGeneration: "r1",
    deviceDependency: "OfficialDeviceUnsupported",
  });
  api.automaticContext.mockResolvedValue({
    providerId: "one",
    credentialGeneration: "r1",
    revision: 0,
    policyVersion: 1,
    enabled: false,
    canEnable: false,
    reason: "OfficialDeviceUnsupported",
    subject: null,
  });
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <QoderAccountPanel />
    </QueryClientProvider>,
  );
  await screen.findByText("Original Qoder");
  expect(api.context).not.toHaveBeenCalled();
  expect(api.claimContext).not.toHaveBeenCalled();
  expect(api.automaticContext).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Qoder 推理身份" }));
  await waitFor(() => expect(api.context).toHaveBeenCalledWith("one"));
  expect(
    await screen.findByText(
      "当前 Qoder 推理身份不可用，请检查原配置与凭据保管状态后重试。",
    ),
  ).toBeInTheDocument();
  fireEvent.keyDown(document, { key: "Escape" });
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  fireEvent.click(screen.getByRole("button", { name: "Qoder 每日积分" }));
  await waitFor(() => expect(api.claimContext).toHaveBeenCalledWith("one"));
  expect(
    await screen.findByText(/目前仅支持已安装的 Qoder Desktop 0.4.8/),
  ).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "查询并领取" })).toBeDisabled();
  expect(api.action).not.toHaveBeenCalled();
  expect(api.claim).not.toHaveBeenCalled();
  expect(api.setAutomatic).not.toHaveBeenCalled();
});
