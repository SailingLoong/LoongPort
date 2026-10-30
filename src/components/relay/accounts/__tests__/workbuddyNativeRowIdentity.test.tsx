import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import type {
  PlatformAccountSummary,
  PlatformPresenceSnapshot,
} from "../useAccountPlatformPresence";
import { useAccountsOverviewViewModel } from "../useAccountsOverviewViewModel";
import { createAccountOverviewPreferencesRepository } from "../accountOverviewPreferences";
const source = vi.hoisted(() => ({
  invoke: vi.fn(),
  presence: {
    data: undefined as PlatformPresenceSnapshot | undefined,
    dataUpdatedAt: 10000,
    isFetching: false,
    isPending: false,
    error: null as Error | null,
    refetch: vi.fn(),
  },
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: source.invoke }));
vi.mock("../useAccountPlatformPresence", () => ({
  useAccountPlatformPresence: () => source.presence,
}));
vi.mock("../useServiceAccounts", () => ({
  useServiceAccounts: () => ({
    accounts: [],
    isPending: false,
    error: null,
    reload: vi.fn(),
  }),
}));
const lifetimeA = "d54c6503-ae85-4ec5-b591-6393e9d41930",
  lifetimeB = "4c6ddd34-cfb2-4ef1-bcb0-380cf25036a8";
const wb = (
  generation = 3,
  label = "Synthetic row",
  id = "synthetic-a",
): PlatformAccountSummary =>
  ({
    source: "workbuddy",
    platform: "workbuddy",
    id,
    label,
    snapshot: {
      kind: "workbuddy",
      value: {
        id,
        label,
        generation,
        autoClaimEnabled: false,
        credits: { totalRemaining: 20 },
      },
    },
  }) as PlatformAccountSummary;
const captured = (
  generation = 3,
  incarnation = lifetimeA,
  id = "synthetic-a",
) => ({
  selection: { id, generation, incarnation },
  label: "Different catalog label",
  realm: "www.workbuddy.cn",
});
const reply = (accounts = [captured()]) => ({
  accounts,
  requestOutcome: "unknown",
  activationAvailable: false,
});
function defer() {
  let resolve!: (value: unknown) => void, reject!: (error: unknown) => void;
  const promise = new Promise<unknown>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const clients: QueryClient[] = [];

it("enables file switching only for a fresh matching native target and backend capability", async () => {
  source.presence.data = {
    accounts: [wb()],
    platforms: ["workbuddy"],
    unavailable: [],
    connections: [],
  };
  source.invoke.mockResolvedValue({ ...reply(), activationAvailable: true });
  const hook = setup();
  await waitFor(() => expect(identity(hook)?.status).toBe("ready"));
  const getRow = () =>
    hook.result.current.orderedGroups.find((group) => group.id === "workbuddy")!
      .rows[0];
  expect(getRow().switchAction.enabled).toBe(true);
  source.presence.isFetching = true;
  hook.rerender();
  expect(getRow().switchAction.enabled).toBe(false);
});
function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  clients.push(client);
  const hook = renderHook(
    () =>
      useAccountsOverviewViewModel({
        preferencesRepository: createAccountOverviewPreferencesRepository(null),
      }),
    {
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  return { ...hook, client };
}
function identity(hook: ReturnType<typeof setup>) {
  return hook.result.current.orderedGroups.find(
    (group) => group.id === "workbuddy",
  )?.rows[0]?.nativeIdentity;
}
beforeEach(() => {
  vi.clearAllMocks();
  source.presence = {
    data: {
      accounts: [wb()],
      platforms: ["workbuddy"],
      unavailable: [],
      connections: [],
    },
    dataUpdatedAt: 10000,
    isFetching: false,
    isPending: false,
    error: null,
    refetch: vi.fn(),
  };
  source.invoke.mockResolvedValue(reply());
});
afterEach(() => {
  cleanup();
  clients.splice(0).forEach((client) => client.clear());
});
it("reads one existing catalog and matches only the exact id/generation, never label or capability", async () => {
  const hook = setup();
  await waitFor(() =>
    expect(identity(hook)).toEqual({
      status: "ready",
      selection: captured().selection,
    }),
  );
  expect(source.invoke.mock.calls).toEqual([
    ["list_workbuddy_native_captures"],
  ]);
  expect(
    hook.result.current.orderedGroups[0].rows[0].switchAction.enabled,
  ).toBe(false);
});
it.each([
  {
    name: "stale generation",
    catalog: [captured(2)],
    rows: [wb()],
    status: "unavailable",
  },
  {
    name: "same label, different id",
    catalog: [{ ...captured(3, lifetimeA, "other"), label: "Synthetic row" }],
    rows: [wb()],
    status: "unavailable",
  },
  {
    name: "duplicate catalog lifetime",
    catalog: [captured(), captured(3, lifetimeB)],
    rows: [wb()],
    status: "ambiguous",
  },
  {
    name: "duplicate ordinary id",
    catalog: [captured()],
    rows: [wb(), wb(4, "Duplicate")],
    status: "ambiguous",
  },
])("fails closed for $name", async ({ catalog, rows, status }) => {
  source.presence.data!.accounts = rows;
  source.invoke.mockResolvedValue(reply(catalog));
  const hook = setup();
  await waitFor(() =>
    expect(identity(hook)).toEqual({ status, selection: null }),
  );
});
it("ignores a late old-generation catalog after a fresh inventory changes", async () => {
  const old = defer();
  source.invoke
    .mockImplementationOnce(() => old.promise)
    .mockResolvedValueOnce(reply([captured(4, lifetimeB)]));
  const hook = setup();
  await waitFor(() => expect(source.invoke).toHaveBeenCalledTimes(1));
  source.presence.data = { ...source.presence.data!, accounts: [wb(4)] };
  source.presence.dataUpdatedAt++;
  hook.rerender();
  await waitFor(() =>
    expect(identity(hook)).toEqual({
      status: "ready",
      selection: captured(4, lifetimeB).selection,
    }),
  );
  await act(async () => {
    old.resolve(reply());
    await Promise.resolve();
  });
  expect(identity(hook)?.selection).toEqual(captured(4, lifetimeB).selection);
  expect(source.invoke.mock.calls.map(([command]) => command)).toEqual([
    "list_workbuddy_native_captures",
    "list_workbuddy_native_captures",
  ]);
});
it("never reuses a cached lifetime after observed delete/recreate even with the same timestamp/id/generation", async () => {
  const next = defer();
  source.invoke
    .mockResolvedValueOnce(reply())
    .mockImplementationOnce(() => next.promise);
  const hook = setup();
  await waitFor(() =>
    expect(identity(hook)?.selection).toEqual(captured().selection),
  );
  source.presence.data = { ...source.presence.data!, accounts: [] };
  hook.rerender();
  expect(identity(hook)).toBeUndefined();
  source.presence.data = { ...source.presence.data, accounts: [wb()] };
  hook.rerender();
  expect(identity(hook)?.selection).toBeNull();
  await waitFor(() => expect(source.invoke).toHaveBeenCalledTimes(2));
  await act(async () => next.resolve(reply([captured(3, lifetimeB)])));
  await waitFor(() =>
    expect(identity(hook)?.selection).toEqual(captured(3, lifetimeB).selection),
  );
});
it("clears a prepared target during inventory refresh and does not trust a failed owner read", async () => {
  const hook = setup();
  await waitFor(() => expect(identity(hook)?.status).toBe("ready"));
  source.presence.isFetching = true;
  hook.rerender();
  expect(identity(hook)).toEqual({ status: "stale", selection: null });
  source.presence.isFetching = false;
  source.presence.error = new Error("SYNTHETIC_PRIVATE_LOCK");
  hook.rerender();
  expect(identity(hook)).toEqual({ status: "stale", selection: null });
  expect(JSON.stringify(hook.result.current.orderedGroups)).not.toContain(
    "SYNTHETIC_PRIVATE_LOCK",
  );
});
it("drops cached selectors when the catalog read fails or is locked, with no retry/write", async () => {
  const hook = setup();
  await waitFor(() => expect(identity(hook)?.status).toBe("ready"));
  source.invoke.mockRejectedValue("journal");
  await act(async () => {
    await hook.client.invalidateQueries({
      queryKey: ["workbuddyNativeCaptures"],
    });
  });
  await waitFor(() =>
    expect(identity(hook)).toEqual({ status: "error", selection: null }),
  );
  expect(source.invoke.mock.calls.map(([name]) => name)).toEqual([
    "list_workbuddy_native_captures",
    "list_workbuddy_native_captures",
  ]);
});
it("does not invent a generation or native selector for an HTTP-only account", async () => {
  source.presence.data!.accounts = [
    {
      source: "workbuddy-inference",
      platform: "workbuddy",
      id: "synthetic-a",
      label: "Synthetic row",
      identity: "HTTP",
      inferenceState: "ready",
    },
  ];
  const hook = setup();
  expect(identity(hook)).toEqual({ status: "unsupported", selection: null });
  await act(async () => Promise.resolve());
  expect(source.invoke).not.toHaveBeenCalled();
});

it("does not block original overview refresh controls while a catalog read is pending", async () => {
  const pending = defer();
  source.invoke.mockImplementation(() => pending.promise);
  const hook = setup();
  await waitFor(() => expect(identity(hook)?.status).toBe("loading"));
  expect(hook.result.current.refreshing).toBe(false);
  await act(async () => hook.result.current.refresh());
  expect(source.presence.refetch).toHaveBeenCalledTimes(1);
  expect(source.invoke.mock.calls.map(([name]) => name)).toEqual([
    "list_workbuddy_native_captures",
  ]);
});
