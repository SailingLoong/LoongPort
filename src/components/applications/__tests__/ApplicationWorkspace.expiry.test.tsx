import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ApplicationWorkspace } from "../ApplicationWorkspace";

const boundary = vi.hoisted(() => ({
  stored: "cheapest",
  reads: 0,
  writes: [] as string[],
  failRead: false,
  failWrite: false,
  loseReply: false,
  setFailover: vi.fn(),
}));
// Only IPC is replaced. The production API, query cache, select and Workspace
// remain real; Rust tests exercise this exact command against the real database.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (command: string, payload: Record<string, string>) => {
    if (command === "get_auto_mode_status") {
      boundary.reads++;
      if (boundary.failRead) throw new Error("synthetic read failure");
      return {
        strategy: boundary.stored,
        enabled: false,
        model: null,
        availableModels: [],
        hasCandidates: true,
        cliInstalled: true,
      };
    }
    if (command === "set_auto_mode_strategy") {
      boundary.writes.push(payload.strategy);
      if (boundary.failWrite) throw new Error("synthetic save failure");
      boundary.stored = payload.strategy;
      if (boundary.loseReply)
        throw new Error("synthetic reply lost after commit");
      return;
    }
    throw new Error(`unexpected IPC ${command}`);
  },
}));
vi.mock("../useApplicationOverview", () => ({
  useApplicationOverview: () => ({
    data: { configurations: [], recentProviderIds: [], isAdditive: false },
    isPending: false,
    error: null,
    busy: false,
    refetch: vi.fn(),
  }),
}));
vi.mock("../useApplicationRouting", () => ({
  useApplicationRouting: () => ({
    data: { tiers: [], autoFailoverEnabled: false, model: null },
    isPending: false,
    error: null,
    busy: false,
    refetch: vi.fn(),
    setFailover: boundary.setFailover,
  }),
}));
vi.mock("@/lib/api/orderProfiles", () => ({
  orderProfilesApi: {
    list: vi.fn().mockResolvedValue({ profiles: [], current: null }),
  },
}));
vi.mock("@/lib/api/modelVerification", () => ({
  modelVerificationApi: { listSummaries: vi.fn().mockResolvedValue([]) },
}));
vi.mock("@/lib/api/startupUpgrade", () => ({
  startupUpgradeApi: {
    query: vi
      .fn()
      .mockResolvedValue({ status: "runtime_active", reviewToken: null }),
  },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
let clients: QueryClient[];
beforeEach(() => {
  boundary.stored = "cheapest";
  boundary.reads = 0;
  boundary.writes = [];
  boundary.failRead = false;
  boundary.failWrite = false;
  boundary.loseReply = false;
  boundary.setFailover.mockClear();
  clients = [];
});
afterEach(() => {
  cleanup();
  clients.forEach((client) => client.clear());
});
function mount(
  appId: "claude" | "codex" = "claude",
  sharedClient?: QueryClient,
) {
  const client =
    sharedClient ??
    new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
  clients.push(client);
  return render(
    <QueryClientProvider client={client}>
      <ApplicationWorkspace
        appId={appId}
        providers={{}}
        onSwitchProvider={vi.fn()}
        onOpenAccount={vi.fn()}
        onAdd={vi.fn()}
        children={null}
      />
    </QueryClientProvider>,
  );
}
async function choose(label: string) {
  const select = await screen.findByRole("combobox", {
    name: "applications.expiryStrategyLabel",
  });
  await waitFor(() => expect(select).toBeEnabled());
  await userEvent.click(select);
  await userEvent.click(await screen.findByRole("option", { name: label }));
}
it("saves through IPC, rereads on another app, and restores applied order without toggling failover", async () => {
  const first = mount();
  await choose("applications.expiryStrategyKnown");
  await waitFor(() =>
    expect(
      screen.getByRole("combobox", {
        name: "applications.expiryStrategyLabel",
      }),
    ).toHaveTextContent("applications.expiryStrategyKnown"),
  );
  expect(boundary.stored).toBe("known_expiry_first");
  expect(boundary.reads).toBeGreaterThan(1);
  first.unmount();
  mount("codex");
  await waitFor(() =>
    expect(
      screen.getByRole("combobox", {
        name: "applications.expiryStrategyLabel",
      }),
    ).toHaveTextContent("applications.expiryStrategyKnown"),
  );
  await choose("applications.expiryStrategyOrder");
  await waitFor(() => expect(boundary.stored).toBe("cheapest"));
  await waitFor(() =>
    expect(
      screen.getByRole("combobox", {
        name: "applications.expiryStrategyLabel",
      }),
    ).toHaveTextContent("applications.expiryStrategyOrder"),
  );
  expect(boundary.writes).toEqual(["known_expiry_first", "cheapest"]);
  expect(boundary.setFailover).not.toHaveBeenCalled();
});
it("keeps the confirmed selection and exposes a rejected save", async () => {
  boundary.failWrite = true;
  mount();
  await choose("applications.expiryStrategyKnown");
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "applications.expiryStrategySaveFailed",
  );
  expect(
    screen.getByRole("combobox", { name: "applications.expiryStrategyLabel" }),
  ).toHaveTextContent("applications.expiryStrategyOrder");
  expect(boundary.stored).toBe("cheapest");
});
it("does not invent an off value on read failure and allows retry", async () => {
  boundary.stored = "known_expiry_first";
  boundary.failRead = true;
  mount();
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "applications.expiryStrategyReadFailed",
  );
  expect(
    screen.getByRole("combobox", { name: "applications.expiryStrategyLabel" }),
  ).toBeDisabled();
  expect(boundary.writes).toEqual([]);
  boundary.failRead = false;
  await userEvent.click(screen.getByRole("button", { name: "common.refresh" }));
  await waitFor(() =>
    expect(
      screen.getByRole("combobox", {
        name: "applications.expiryStrategyLabel",
      }),
    ).toHaveTextContent("applications.expiryStrategyKnown"),
  );
});
it("shows legacy fastest as applied order without silently rewriting it", async () => {
  boundary.stored = "fastest";
  mount();
  await waitFor(() =>
    expect(
      screen.getByRole("combobox", {
        name: "applications.expiryStrategyLabel",
      }),
    ).toHaveTextContent("applications.expiryStrategyOrder"),
  );
  expect(boundary.writes).toEqual([]);
});

it("rereads a committed value after a lost save reply instead of showing an optimistic rollback", async () => {
  boundary.loseReply = true;
  mount();
  await choose("applications.expiryStrategyKnown");
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "applications.expiryStrategySaveFailed",
  );
  await waitFor(() =>
    expect(
      screen.getByRole("combobox", {
        name: "applications.expiryStrategyLabel",
      }),
    ).toHaveTextContent("applications.expiryStrategyKnown"),
  );
  expect(boundary.writes).toEqual(["known_expiry_first"]);
  expect(boundary.reads).toBeGreaterThan(1);
});

it("refreshes both application snapshots in the shared query cache after a global save", async () => {
  mount("claude");
  mount("codex", clients[0]);
  const selects = await screen.findAllByRole("combobox", {
    name: "applications.expiryStrategyLabel",
  });
  await waitFor(() =>
    selects.forEach((select) => expect(select).toBeEnabled()),
  );
  await userEvent.click(selects[0]);
  await userEvent.click(
    await screen.findByRole("option", {
      name: "applications.expiryStrategyKnown",
    }),
  );
  await waitFor(() =>
    selects.forEach((select) =>
      expect(select).toHaveTextContent("applications.expiryStrategyKnown"),
    ),
  );
  expect(boundary.reads).toBeGreaterThanOrEqual(4);
  expect(boundary.writes).toEqual(["known_expiry_first"]);
});
