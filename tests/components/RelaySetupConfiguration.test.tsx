import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { AddHubPage } from "@/components/relay/AddHubPage";
import { RelayDirectoryConnectionPage } from "@/components/relay/onboarding/RelayDirectoryConnectionPage";
import { ServiceConfiguration } from "@/components/relay/onboarding/ServiceConfiguration";
import wire from "../fixtures/relay-setup-wire.json";
const api = vi.hoisted(() => ({
  discoverTiers: vi.fn(),
  prepareApplicationConnection: vi.fn(),
  applyPreparedTier: vi.fn(),
  getSetupOperation: vi.fn(),
  prepareTiers: vi.fn(),
  reconcileSetupOperation: vi.fn(),
  listRelays: vi.fn(),
  refresh: vi.fn(),
  switchTier: vi.fn(),
}));
const complete = vi.hoisted(() => vi.fn());
const dismiss = vi.hoisted(() => vi.fn());
vi.mock("@/components/relay/directory/RelayDirectoryPage", () => ({
  RelayDirectoryPage: ({ onConnected }: any) => (
    <button
      onClick={() => onConnected({ kind: "relay", rowId: 7, name: "Fixture" })}
    >
      Connect fixture account
    </button>
  ),
}));
vi.mock("@/components/relay/OfficialApiPage", () => ({
  OfficialApiPage: () => null,
}));
vi.mock("@/components/relay/accounts/QoderAccountPanel", () => ({
  QoderAccountPanel: () => null,
}));
vi.mock("@/components/providers/AddProviderForm", () => ({
  AddProviderForm: () => null,
}));
vi.mock("@/lib/api/relay", () => ({ relayApi: api }));
vi.mock("@/lib/api/serviceOnboarding", () => ({
  serviceOnboardingApi: {
    status: async () => ({ completed: false }),
    complete,
    dismiss,
  },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? key,
  }),
}));
afterEach(cleanup);
beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  dismiss.mockResolvedValue({ completed: false, shouldPrompt: false });
  api.discoverTiers.mockResolvedValue(wire.discovery);
  api.getSetupOperation.mockResolvedValue(null);
  api.listRelays.mockResolvedValue([]);
});
function page() {
  const onDone = vi.fn(),
    onBack = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  render(
    <QueryClientProvider client={client}>
      <ServiceConfiguration
        account={{ kind: "relay", rowId: 7, name: "Fixture" }}
        sourceAppId="codex"
        onBack={onBack}
        onDone={onDone}
      />
    </QueryClientProvider>,
  );
  return { onDone, onBack, client };
}
it("the production relay page discovers cold choices instead of listing only local tiers", async () => {
  page();
  await waitFor(() => expect(api.discoverTiers).toHaveBeenCalledWith(7));
  expect(api.listRelays).not.toHaveBeenCalled();
  await screen.findByText("https://fixture.example");
  expect(api.prepareTiers).not.toHaveBeenCalled();
  expect(api.refresh).not.toHaveBeenCalled();
});
it("zero selected apps can skip without preparing, switching or completing onboarding", async () => {
  const { onBack, onDone, client } = page();
  const invalidated = vi.spyOn(client, "invalidateQueries");
  await screen.findByText("https://fixture.example");
  expect(dismiss).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "暂不接入" }));
  await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
  expect(onBack).not.toHaveBeenCalled();
  expect(dismiss).toHaveBeenCalledTimes(1);
  expect(invalidated).toHaveBeenCalledWith({
    queryKey: ["service-onboarding"],
  });
  expect(complete).not.toHaveBeenCalled();
  expect(api.prepareTiers).not.toHaveBeenCalled();
  expect(api.switchTier).not.toHaveBeenCalled();
});
it("requires the reviewed impact acknowledgement before preparing only the selected app", async () => {
  page();
  const select = await screen.findByRole("combobox", { name: "Codex" });
  expect((select as HTMLSelectElement).value).toBe("");
  fireEvent.change(select, { target: { value: "a" } });
  fireEvent.click(screen.getByRole("button", { name: "下一步：确认准备" }));
  expect(api.prepareTiers).not.toHaveBeenCalled();
  const confirm = screen.getByRole("button", { name: "确认并准备" });
  expect((confirm as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(
    screen.getByRole("checkbox", {
      name: "我已了解本次只准备所选配置，不会切换应用",
    }),
  );
  api.prepareTiers.mockImplementation(async (request) => ({
    ...wire.receipt,
    operationId: request.operationId,
    status: "settled",
    groups: [{ ...wire.receipt.groups[0], state: "ready", origin: "created" }],
    targets: [{ ...wire.receipt.targets[0], configuration: "prepared" }],
    createdKeyCount: 1,
  }));
  fireEvent.click(confirm);
  await waitFor(() => expect(api.prepareTiers).toHaveBeenCalledTimes(1));
  expect(api.prepareTiers.mock.calls[0][0].selections).toEqual([
    { app: "codex", groupId: "a" },
  ]);
  await screen.findByText("本地配置已准备；本次尚未执行应用接入");
  expect(api.switchTier).not.toHaveBeenCalled();
  expect(complete).not.toHaveBeenCalled();
});
it("an empty successful discovery is not presented as an error or completed setup", async () => {
  api.discoverTiers.mockResolvedValue({ ...wire.discovery, choices: [] });
  page();
  await screen.findByText("未发现可供这些应用选择的分组");
  expect(screen.queryByRole("alert")).toBeNull();
  expect(
    (
      screen.getByRole("button", {
        name: "下一步：确认准备",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  expect(complete).not.toHaveBeenCalled();
});

it("the production prepared result requires a separate application-impact confirmation", async () => {
  const ready = {
    ...wire.receipt,
    status: "settled",
    receiptRevision: 8,
    groups: [{ ...wire.receipt.groups[0], state: "ready" }],
    targets: [
      {
        ...wire.receipt.targets[0],
        configuration: "prepared",
        connection: { state: "notRequested" },
      },
    ],
  };
  api.getSetupOperation.mockResolvedValue(ready);
  const reviewed = {
    ...ready,
    receiptRevision: 9,
    targets: [
      {
        ...ready.targets[0],
        connection: { state: "confirmationRequired", requestId: "routing-1" },
      },
    ],
  };
  api.prepareApplicationConnection.mockResolvedValue({
    receipt: reviewed,
    confirmation: null,
  });
  page();
  fireEvent.click(await screen.findByRole("button", { name: "接入 Codex" }));
  await screen.findByRole("heading", { name: "确认接入应用" });
  expect(api.applyPreparedTier).not.toHaveBeenCalled();
  const confirm = screen.getByRole("button", { name: "确认接入" });
  expect((confirm as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(
    screen.getByRole("checkbox", { name: "我已了解将替换此应用的当前配置" }),
  );
  api.applyPreparedTier.mockResolvedValue({
    receipt: {
      ...reviewed,
      receiptRevision: 10,
      targets: [
        {
          ...reviewed.targets[0],
          connection: {
            state: "applied",
            writeAcknowledged: true,
            observedCurrent: true,
          },
        },
      ],
    },
    result: { status: "switched" },
  });
  fireEvent.click(confirm);
  await screen.findByText("已记录本次接入成功；当前状态可核对");
  expect(api.applyPreparedTier).toHaveBeenCalledWith(
    ready.operationId,
    "codex",
    9,
    false,
  );
  expect(api.switchTier).not.toHaveBeenCalled();
});

it("keeps the original page when explicit dismissal fails and never changes sharing consent", async () => {
  dismiss.mockRejectedValue(new Error("fixture failure"));
  const { onDone, onBack } = page();
  await screen.findByText("https://fixture.example");
  fireEvent.click(screen.getByRole("button", { name: "暂不接入" }));
  await screen.findByRole("alert");
  expect(onDone).not.toHaveBeenCalled();
  expect(onBack).not.toHaveBeenCalled();
  expect(complete).not.toHaveBeenCalled();
});

it("does not claim an existing local configuration has never been active", async () => {
  api.getSetupOperation.mockResolvedValue({
    ...wire.receipt,
    status: "settled",
    targets: [
      {
        ...wire.receipt.targets[0],
        configuration: "existing",
        connection: { state: "notRequested" },
      },
    ],
  });
  page();
  await screen.findByText("本地配置已准备；本次尚未执行应用接入");
});
it("distinguishes known pre-write rejection from an observed configuration change", async () => {
  api.getSetupOperation.mockResolvedValue({
    ...wire.receipt,
    status: "settled",
    targets: [
      {
        ...wire.receipt.targets[0],
        configuration: "prepared",
        connection: { state: "failedBeforeWrite" },
      },
    ],
  });
  page();
  await screen.findByText("本次未执行接入，请重新检查接入影响");
  expect(
    screen.queryByText("应用配置已变化，请重新检查接入影响"),
  ).not.toBeInTheDocument();
});

for (const entry of ["directory", "addHub"] as const) {
  it(`${entry} uses its real final exit callback after explicit skip, not the previous configuration screen`, async () => {
    const onBack = vi.fn();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, gcTime: 0 } },
    });
    render(
      <QueryClientProvider client={client}>
        {entry === "directory" ? (
          <RelayDirectoryConnectionPage sourceAppId="codex" onBack={onBack} />
        ) : (
          <AddHubPage
            sourceAppId="codex"
            initialTab="directory"
            firstVisit
            onBack={onBack}
            onAddProvider={() => undefined as never}
          />
        )}
      </QueryClientProvider>,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Connect fixture account" }),
    );
    await screen.findByText("https://fixture.example");
    expect(onBack).not.toHaveBeenCalled();
    expect(dismiss).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "暂不接入" }));
    await waitFor(() => expect(onBack).toHaveBeenCalledTimes(1));
    expect(dismiss).toHaveBeenCalledTimes(1);
    expect(complete).not.toHaveBeenCalled();
    expect(api.prepareTiers).not.toHaveBeenCalled();
  });
}
it("labels restored current-selection evidence as a previous observation, not a fresh check", async () => {
  api.getSetupOperation.mockResolvedValue({
    ...wire.receipt,
    status: "needsReconcile",
    targets: [
      {
        ...wire.receipt.targets[0],
        configuration: "prepared",
        connection: { state: "unknown", observedCurrent: true },
      },
    ],
  });
  page();
  await screen.findByText("上次核对时配置指向此项，但原接入操作结果仍待核对");
  expect(api.reconcileSetupOperation).not.toHaveBeenCalled();
});
