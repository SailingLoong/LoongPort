import { act, renderHook, waitFor, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import type { RelayRow } from "@/lib/api/relay";
import type { VendorAccountRow } from "@/lib/api/vendor";
import {
  useServiceAccountDetailsViewModel,
  type ServiceAccountDetailsOptions,
} from "../useServiceAccountDetailsViewModel";
import { serviceAccountsKey } from "../useServiceAccounts";

const mocks = vi.hoisted(() => ({
  relays: vi.fn(),
  vendors: vi.fn(),
  cost: vi.fn(),
}));
vi.mock("@/lib/api/relay", () => ({ relayApi: { listRelays: mocks.relays } }));
vi.mock("@/lib/api/vendor", () => ({ vendorApi: { list: mocks.vendors } }));
vi.mock("@/lib/api/usage", () => ({
  usageApi: { providersWindowCost: mocks.cost },
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { language: "en", resolvedLanguage: "en" },
  }),
}));
const relay = (id: number, providerId: string) =>
  ({
    id,
    siteName: `Relay ${id}`,
    accountLabel: `Account ${id}`,
    status: "usable",
    canQueryBalance: false,
    tiers: [{ providerId, displayName: providerId, subscriptionWindows: [] }],
  }) as unknown as RelayRow;
const vendor = {
  id: 3,
  vendorId: "opencode",
  vendorName: "OpenCode",
  status: "usable",
  canQueryBalance: false,
  plans: [],
} as unknown as VendorAccountRow;
const clients: QueryClient[] = [];
beforeEach(() => {
  vi.clearAllMocks();
  mocks.relays.mockImplementation(async (app: string) =>
    app === "codex" ? [relay(1, "p1"), relay(2, "p2")] : [],
  );
  mocks.vendors.mockImplementation(async (app: string) => ({
    supported: true,
    accounts: app === "claude" ? [vendor] : [],
  }));
  mocks.cost.mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  clients.forEach((client) => client.clear());
  clients.length = 0;
});
function setup(overrides: Partial<ServiceAccountDetailsOptions> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  clients.push(client);
  const onSelectAccount = vi.fn(),
    onOpenApp = vi.fn();
  const options: ServiceAccountDetailsOptions = {
    appId: "gemini",
    onOpenApp,
    ...overrides,
  };
  const result = renderHook(
    (props: ServiceAccountDetailsOptions) =>
      useServiceAccountDetailsViewModel(props),
    {
      initialProps: options,
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  return { ...result, client, options, onSelectAccount, onOpenApp };
}

describe("service account details VM with original Query owner", () => {
  it("projects original inventory and opens details using the account App without writing any configuration", async () => {
    const { result, onOpenApp } = setup();
    await waitFor(() => expect(result.current.rows).toHaveLength(3));
    act(() => result.current.rows[0].openDetails());
    expect(result.current.detail).toMatchObject({
      account: { kind: "relay", id: 1 },
      appId: "codex",
      snapshot: { appId: "codex", row: { id: 1 } },
    });
    act(() => result.current.detail!.openApp());
    expect(onOpenApp).toHaveBeenCalledExactlyOnceWith("codex");
    act(() => result.current.back());
    expect(result.current.detail).toBeNull();
  });
  it("scopes OpenCode and ordinary accounts before selecting and passes the routed App to the original callback", async () => {
    const onSelectAccount = vi.fn();
    const { result } = setup({ accountKind: "opencode", onSelectAccount });
    await waitFor(() => expect(result.current.rows).toHaveLength(1));
    act(() => result.current.rows[0].openDetails());
    expect(onSelectAccount).toHaveBeenCalledExactlyOnceWith(
      { kind: "vendor", id: 3, appId: "claude" },
      "claude",
    );
    expect(result.current.detail).toBeNull(); // controlled selection waits for its owner
  });
  it("retains a routed account target but delegates a missing App snapshot as null", async () => {
    const { result } = setup({
      account: { kind: "relay", id: 1 },
      appId: "gemini",
      onSelectAccount: vi.fn(),
    });
    await waitFor(() => expect(result.current.detail?.name).toBe("Relay 1"));
    expect(result.current.detail?.snapshot).toBeNull();
    expect(result.current.detail?.showBack).toBe(false);
  });
  it("keeps excluded routed OpenCode details out of the ordinary account manager", async () => {
    const { result } = setup({
      accountKind: "relay-and-vendor",
      account: { kind: "vendor", id: 3 },
      onSelectAccount: vi.fn(),
    });
    await waitFor(() => expect(result.current.rows).toHaveLength(2));
    expect(result.current.detail).toBeNull();
  });
  it("follows a new workspace navigation target and repeated request without changing the App owner", async () => {
    const onSelectAccount = vi.fn();
    const { options, rerender } = setup({
      accountKind: "opencode",
      target: { id: "3", request: 1 },
      onSelectAccount,
    });
    await waitFor(() => expect(onSelectAccount).toHaveBeenCalledTimes(1));
    rerender({ ...options, target: { id: "3", request: 2 } });
    await waitFor(() => expect(onSelectAccount).toHaveBeenCalledTimes(2));
    expect(onSelectAccount.mock.calls).toEqual([
      [{ kind: "vendor", id: 3, appId: "gemini" }, "gemini"],
      [{ kind: "vendor", id: 3, appId: "gemini" }, "gemini"],
    ]);
  });
  it("rejects a fulfilled Query refetch error when the existing save path requires fresh data, still invalidating source bindings", async () => {
    const { result, client } = setup();
    await waitFor(() => expect(result.current.rows).toHaveLength(3));
    client.setQueryData(["serviceSources"], [{ sourceId: "old" }]);
    const failure = new Error("synthetic read failed");
    mocks.relays.mockRejectedValue(failure);
    await act(async () => {
      await expect(result.current.reloadAccounts(true)).rejects.toBe(failure);
    });
    expect(client.getQueryState(["serviceSources"])?.isInvalidated).toBe(true);
    expect(client.getQueryState(serviceAccountsKey)?.status).toBe("error");
  });
  it("keeps ordinary refresh non-throwing and exposes the original Query error instead of claiming a fresh snapshot", async () => {
    const { result } = setup();
    await waitFor(() => expect(result.current.rows).toHaveLength(3));
    const failure = new Error("synthetic read failed");
    mocks.relays.mockRejectedValue(failure);
    await act(async () => {
      await expect(result.current.reloadAccounts()).resolves.toBeUndefined();
    });
    await waitFor(() => expect(result.current.error).toBe(failure));
  });
  it("never applies an old account's late usage result to a newly selected account", async () => {
    let finish!: (value: any) => void;
    mocks.cost.mockImplementation((ids: string[]) =>
      ids.includes("p1")
        ? new Promise((resolve) => {
            finish = resolve;
          })
        : Promise.resolve([
            { providerId: "p2", stats: { costUsd7d: 2, costUsd30d: 3 } },
          ]),
    );
    const { result, options, rerender } = setup({
      appId: "codex",
      account: { kind: "relay", id: 1 },
      onSelectAccount: vi.fn(),
    });
    await waitFor(() => expect(mocks.cost).toHaveBeenCalledWith(["p1"]));
    rerender({ ...options, account: { kind: "relay", id: 2 } });
    await waitFor(() => expect(result.current.detail?.usage?.total30).toBe(3));
    await act(async () =>
      finish([{ providerId: "p1", stats: { costUsd7d: 77, costUsd30d: 99 } }]),
    );
    expect(result.current.detail).toMatchObject({
      account: { kind: "relay", id: 2 },
      usage: { total30: 3 },
    });
  });
  it("does not fetch usage for a platform manager or vendor account", async () => {
    const { result } = setup({ platform: "workbuddy" });
    await waitFor(() => expect(result.current.rows).toHaveLength(3));
    expect(result.current.entry.kind).toBe("platform");
    expect(mocks.cost).not.toHaveBeenCalled();
  });
});

it("treats workspace target as a navigation request, not a new command whenever the shell callback changes", async () => {
  const first = vi.fn(),
    second = vi.fn();
  const { options, rerender } = setup({
    accountKind: "opencode",
    target: { id: "3", request: 1 },
    onSelectAccount: first,
  });
  await waitFor(() => expect(first).toHaveBeenCalledTimes(1));
  rerender({ ...options, onSelectAccount: second });
  expect(second).not.toHaveBeenCalled();
});
