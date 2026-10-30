import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { QoderAccountPanel } from "@/components/relay/accounts/QoderAccountPanel";
import type { Provider } from "@/types";
const api = vi.hoisted(() => ({
  getAll: vi.fn(),
  context: vi.fn(),
  automaticContext: vi.fn(),
  setAutomatic: vi.fn(),
}));
vi.mock("@/lib/api/providers", () => ({
  providersApi: { getAll: api.getAll },
}));
vi.mock("@/lib/api/qoderDaily", () => ({ qoderDailyApi: api }));
vi.mock("@/components/relay/accounts/QoderApplicationEntry", () => ({
  QoderApplicationEntry: () => null,
}));
const row = (generation: string): Provider =>
  ({
    id: "fixture",
    name: "Synthetic account",
    settingsConfig: {},
    meta: { providerType: "qoder_sdk", qoderCampaignState: { generation } },
  }) as Provider;
const policy = {
  providerId: "fixture",
  credentialGeneration: "generation-a",
  revision: 4,
  policyVersion: 1,
  enabled: false,
  canEnable: true,
  reason: null,
  subject: null,
};
it("keeps unresolved policy receipts when the original provider's credential generation changes", async () => {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  client.setQueryData(["providers", "codex"], {
    providers: { fixture: row("generation-a") },
  });
  api.getAll.mockResolvedValue({ fixture: row("generation-a") });
  api.context.mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-a",
    deviceDependency: null,
  });
  api.automaticContext.mockResolvedValue(policy);
  api.setAutomatic.mockRejectedValue(new Error("synthetic response loss"));
  render(
    <QueryClientProvider client={client}>
      <QoderAccountPanel />
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Qoder 每日积分" }));
  const consent = await screen.findByRole("checkbox", {
    name: "确认国际版 Qoder PAT 并持续授权自动签到",
  });
  await waitFor(() => expect(consent).toBeEnabled());
  fireEvent.click(consent);
  fireEvent.click(screen.getByRole("switch", { name: "自动签到" }));
  await screen.findByText(/自动签到设置结果尚未确认/);
  fireEvent.keyDown(document, { key: "Escape" });
  await waitFor(() =>
    expect(
      screen.queryByRole("switch", { name: "自动签到" }),
    ).not.toBeInTheDocument(),
  );
  api.context.mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-b",
    deviceDependency: null,
  });
  api.automaticContext.mockResolvedValue({
    ...policy,
    credentialGeneration: "generation-b",
    revision: 100,
    subject: "synthetic-new-generation",
  });
  await act(async () =>
    client.setQueryData(["providers", "codex"], {
      providers: { fixture: row("generation-b") },
    }),
  );
  fireEvent.click(screen.getByRole("button", { name: "Qoder 每日积分" }));
  await screen.findByRole("switch", { name: "自动签到" });
  await screen.findByText("synthetic-new-generation");
  expect(screen.getByText(/自动签到设置结果尚未确认/)).toBeInTheDocument();
  expect(screen.getByRole("switch", { name: "自动签到" })).toBeDisabled();
  expect(api.setAutomatic).toHaveBeenCalledTimes(1);
});
