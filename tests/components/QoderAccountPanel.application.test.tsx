import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { AddHubPage } from "@/components/relay/AddHubPage";
import { ServicesPage } from "@/components/relay/accounts/ServicesPage";
import { QoderAccountPanel } from "@/components/relay/accounts/QoderAccountPanel";
import { AccountApplicationNavigation } from "@/components/relay/accounts/AccountWorkspace";
import { providersApi } from "@/lib/api/providers";
import { serviceSourcesApi } from "@/lib/api/serviceSources";
vi.mock("@/components/relay/directory/RelayDirectoryPage", () => ({
  RelayDirectoryPage: () => null,
}));
vi.mock("@/components/relay/OfficialApiPage", () => ({
  OfficialApiPage: () => null,
}));
vi.mock("@/components/providers/AddProviderForm", () => ({
  AddProviderForm: () => null,
}));
vi.mock("@/lib/api/relay", () => ({
  relayApi: { listRelays: vi.fn().mockResolvedValue([]) },
}));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: {
    list: vi.fn().mockResolvedValue({ supported: false, accounts: [] }),
  },
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("@/lib/api/providers", () => ({ providersApi: { getAll: vi.fn() } }));
vi.mock("@/lib/api/serviceSources", () => ({
  serviceSourcesApi: { list: vi.fn() },
}));
vi.mock("@/components/providers/qoder/QoderInferenceIdentity", () => ({
  QoderInferenceIdentity: () => null,
}));
vi.mock("@/components/providers/qoder/QoderDailyClaim", () => ({
  QoderDailyClaim: () => null,
}));
vi.mock("@/components/providers/qoder/QoderCnDeviceLogin", () => ({
  QoderCnDeviceLogin: () => null,
}));
vi.mock("@/components/providers/qoder/QoderCnCampaigns", () => ({
  QoderCnCampaigns: () => null,
}));
const rows = {
  a: {
    id: "a",
    name: "Account A",
    settingsConfig: {},
    meta: { providerType: "qoder_sdk" },
  },
  b: {
    id: "b",
    name: "Account B",
    settingsConfig: {},
    meta: { providerType: "qoder_sdk" },
  },
};
const source = (id: string, model: string) => ({
  sourceId: `source-${id}`,
  account: {
    kind: "nativeAccount",
    platform: "qoder_sdk",
    id: `account-${id}`,
  },
  name: `Source ${id}`,
  serviceName: null,
  accountLabel: null,
  configurationName: null,
  bindings: [
    {
      appId: "codex",
      providerId: id,
      model,
      usageState: "current",
      canSelect: false,
    },
  ],
});
function setup() {
  const open = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <AccountApplicationNavigation.Provider value={open}>
        <QoderAccountPanel />
      </AccountApplicationNavigation.Provider>
    </QueryClientProvider>,
  );
  return { open, client };
}
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(providersApi.getAll).mockResolvedValue(rows as never);
});
it("shows only current provider binding and repeated navigation never switches account", async () => {
  vi.mocked(serviceSourcesApi.list).mockResolvedValue([
    source("a", "model-a"),
    source("b", "model-b"),
  ] as never);
  const { open } = setup();
  expect(await screen.findByText("model-a")).toBeInTheDocument();
  expect(screen.queryByText("model-b")).not.toBeInTheDocument();
  const button = screen.getByRole("button", { name: /model-a/ });
  fireEvent.click(button);
  fireEvent.click(button);
  expect(open.mock.calls).toEqual([["codex"], ["codex"]]);
  expect(
    screen.getByRole("heading", { name: "Account A" }),
  ).toBeInTheDocument();
  expect(providersApi.getAll).toHaveBeenCalledTimes(1);
});
it("shows the confirmed-empty and read-error states", async () => {
  vi.mocked(serviceSourcesApi.list)
    .mockResolvedValueOnce([])
    .mockRejectedValueOnce(new Error("private"));
  setup();
  expect(await screen.findByText(/暂无后端确认的应用绑定/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "查看 Account B" }));
  expect(await screen.findByText(/应用绑定读取失败/)).toBeInTheDocument();
  expect(screen.queryByText("private")).not.toBeInTheDocument();
});
it("does not show a late response from the previous account", async () => {
  let resolveA!: (value: never) => void;
  vi.mocked(serviceSourcesApi.list)
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveA = resolve;
        }),
    )
    .mockResolvedValueOnce([source("b", "model-b")] as never);
  setup();
  await screen.findByRole("button", { name: "查看 Account B" });
  await waitFor(() => expect(serviceSourcesApi.list).toHaveBeenCalledTimes(1));
  fireEvent.click(screen.getByRole("button", { name: "查看 Account B" }));
  expect(await screen.findByText("model-b")).toBeInTheDocument();
  await act(async () => resolveA([source("a", "model-a")] as never));
  expect(screen.queryByText("model-a")).not.toBeInTheDocument();
  expect(screen.getByText("model-b")).toBeInTheDocument();
});

it("AddHub actual Qoder boundary forwards repeated clicks to original app", async () => {
  vi.mocked(serviceSourcesApi.list).mockResolvedValue([
    source("a", "model-a"),
    source("b", "model-b"),
  ] as never);
  const open = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const extra = { onOpenApp: open };
  render(
    <QueryClientProvider client={client}>
      <AddHubPage
        sourceAppId="codex"
        onBack={vi.fn()}
        onAddProvider={vi.fn()}
        entryRequest={{
          id: 1,
          tab: "accounts",
          intent: { kind: "account-boundary", platform: "qoder" },
        }}
        {...extra}
      />
    </QueryClientProvider>,
  );
  expect(await screen.findByText("model-a")).toBeInTheDocument();
  expect(screen.queryByText("model-b")).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /model-a/ }));
  fireEvent.click(screen.getByRole("button", { name: /model-a/ }));
  expect(open.mock.calls).toEqual([["codex"], ["codex"]]);
  expect(
    screen.getByRole("heading", { name: "Account A" }),
  ).toBeInTheDocument();
});
it("Services actual Qoder platform forwards to original app", async () => {
  vi.mocked(serviceSourcesApi.list).mockResolvedValue([
    source("a", "model-a"),
  ] as never);
  const open = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ServicesPage
        appId="codex"
        platform="qoder"
        onOpenApp={open}
        onOpenAddHub={vi.fn()}
      />
    </QueryClientProvider>,
  );
  fireEvent.click(await screen.findByRole("button", { name: /model-a/ }));
  expect(open).toHaveBeenCalledWith("codex");
});
it("AddHub without a navigation callback disables the binding and explains why", async () => {
  vi.mocked(serviceSourcesApi.list).mockResolvedValue([
    source("a", "model-a"),
  ] as never);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <AddHubPage
        sourceAppId="codex"
        onBack={vi.fn()}
        onAddProvider={vi.fn()}
        entryRequest={{
          id: 1,
          tab: "accounts",
          intent: { kind: "account-boundary", platform: "qoder" },
        }}
      />
    </QueryClientProvider>,
  );
  expect(await screen.findByRole("button", { name: /model-a/ })).toBeDisabled();
  expect(screen.getByText(/当前页面无法打开原应用/)).toBeInTheDocument();
});
it("Services Qoder binding refreshes after the existing sources query invalidation", async () => {
  vi.mocked(serviceSourcesApi.list)
    .mockResolvedValueOnce([source("a", "old-model")] as never)
    .mockResolvedValueOnce([source("a", "new-model")] as never);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ServicesPage
        appId="codex"
        platform="qoder"
        onOpenApp={vi.fn()}
        onOpenAddHub={vi.fn()}
      />
    </QueryClientProvider>,
  );
  expect(await screen.findByText("old-model")).toBeInTheDocument();
  await act(async () => {
    await client.invalidateQueries({ queryKey: ["serviceSources"] });
  });
  expect(await screen.findByText("new-model")).toBeInTheDocument();
  expect(screen.queryByText("old-model")).not.toBeInTheDocument();
});
