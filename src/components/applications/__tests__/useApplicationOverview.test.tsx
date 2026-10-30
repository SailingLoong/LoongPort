import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import type { ApplicationConfiguration } from "@/lib/api/applicationOverview";
import { useApplicationOverview } from "../useApplicationOverview";
const mocks = vi.hoisted(() => ({
  get: vi.fn(),
  relay: vi.fn(),
  relayModel: vi.fn(),
  apply: vi.fn(),
  vendor: vi.fn(),
  prepare: vi.fn(),
  query: vi.fn(),
  error: vi.fn(),
  success: vi.fn(),
}));
vi.mock("@/lib/api/applicationOverview", () => ({
  applicationOverviewApi: { get: mocks.get },
}));
vi.mock("@/lib/api/relay", () => ({
  relayApi: { switchTier: mocks.relay, switchTierModel: mocks.relayModel },
}));
vi.mock("@/lib/api/applicationRouting", () => ({
  applicationRoutingApi: { apply: mocks.apply },
}));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: {
    switchPreparedConnection: mocks.vendor,
    prepareConnection: mocks.prepare,
    queryConnection: mocks.query,
  },
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("sonner", () => ({
  toast: { error: mocks.error, success: mocks.success, warning: vi.fn() },
}));
const target = {
  providerId: "relay-config",
  name: "Example",
  canSelect: true,
  selection: { kind: "relay" },
} as ApplicationConfiguration;
const providers = {};
function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
let client: QueryClient;
beforeEach(() => {
  vi.clearAllMocks();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  mocks.get.mockResolvedValue({
    configurations: [],
    recentProviderIds: [],
    isAdditive: false,
  });
});
describe("configuration selection", () => {
  it("serializes manual custom-provider switches like managed tier switches", async () => {
    let finish!: () => void;
    const switchProvider = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    const custom = {
      ...target,
      providerId: "custom",
      selection: { kind: "provider" },
    } as ApplicationConfiguration;
    const customProviders = { custom: { id: "custom", name: "Custom" } } as any;
    const { result } = renderHook(
      () => useApplicationOverview("codex", customProviders, switchProvider),
      { wrapper },
    );
    let first: Promise<void>;
    await act(async () => {
      first = result.current.select(custom);
    });
    expect(result.current.busy).toBe(true);
    await act(async () => {
      await result.current.select(custom);
    });
    expect(switchProvider).toHaveBeenCalledTimes(1);
    await act(async () => {
      finish();
      await first;
    });
    expect(result.current.busy).toBe(false);
  });

  it("keeps the backend confirmation boundary and cancellation writes nothing further", async () => {
    mocks.relay.mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Example",
    });
    const { result } = renderHook(
      () => useApplicationOverview("codex", providers, vi.fn()),
      { wrapper },
    );
    await act(async () => {
      await result.current.select(target);
    });
    expect(result.current.confirmation).toBe("Example");
    act(() => result.current.cancel());
    expect(mocks.relay).toHaveBeenCalledTimes(1);
    expect(mocks.success).not.toHaveBeenCalled();
    expect(result.current.confirmation).toBeNull();
  });
  it("replays the exact vendor plan only after the explicit confirmation", async () => {
    let operationId = "";
    const receipt = (status: "prepared" | "completed") => ({
      operationId,
      rowId: 3,
      plan: "standard",
      app: "codex",
      providerId: "relay-config",
      receiptRevision: 1,
      status,
      operationState: status,
      completedConfigurationRevision:
        status === "completed" ? "configured" : null,
    });
    mocks.query.mockImplementation(async (_app, operation) =>
      operation === null ? { status: "none" } : receipt("completed"),
    );
    mocks.prepare.mockImplementation(async (input) => {
      operationId = input.operationId;
      return receipt("prepared");
    });
    mocks.vendor
      .mockResolvedValueOnce({
        status: "confirmationRequired",
        targetName: "Example",
      })
      .mockResolvedValueOnce({
        status: "switched",
        warnings: [],
        chatgptWasRunning: false,
        chatgptRelaunched: false,
      });
    const { result } = renderHook(
      () => useApplicationOverview("codex", providers, vi.fn()),
      { wrapper },
    );
    await act(async () => {
      await result.current.select({
        ...target,
        selection: { kind: "vendor", rowId: 3, planId: "standard" },
      });
    });
    act(() => result.current.confirm(false));
    await waitFor(() =>
      expect(mocks.vendor).toHaveBeenLastCalledWith({
        rowId: 3,
        plan: "standard",
        app: "codex",
        providerId: "relay-config",
        quitChatgpt: false,
        operationId,
        expectedRevision: 1,
      }),
    );
    await waitFor(() => expect(result.current.busy).toBe(false));
    expect(mocks.success).toHaveBeenCalledTimes(1);
  });
  it("reports failure without manufacturing a successful selection", async () => {
    mocks.relay.mockRejectedValue(new Error("Unavailable"));
    const { result } = renderHook(
      () => useApplicationOverview("codex", providers, vi.fn()),
      { wrapper },
    );
    await act(async () => {
      await result.current.select(target);
    });
    expect(mocks.error).toHaveBeenCalled();
    expect(mocks.success).not.toHaveBeenCalled();
    expect(result.current.busy).toBe(false);
  });
  it("does not reopen a confirmation after leaving the application", async () => {
    let resolve!: (value: unknown) => void;
    mocks.relay.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const { result, unmount } = renderHook(
      () => useApplicationOverview("codex", providers, vi.fn()),
      { wrapper },
    );
    act(() => {
      void result.current.select(target);
    });
    unmount();
    await act(async () =>
      resolve({ status: "confirmationRequired", targetName: "Example" }),
    );
    expect(mocks.relay).toHaveBeenCalledTimes(1);
    expect(mocks.success).not.toHaveBeenCalled();
  });
});

it("retains the selected model and uses canonical routing for the catalog preview and final apply", async () => {
  const catalog = {
    ownership: "unclaimedExternal",
    choice: "keep",
    canManage: true,
    revision: "catalog-v1",
    beforePointer: "/external/models.json",
    afterPointer: "/external/models.json",
  };
  const decision = { choice: "manage" as const, revision: "catalog-v1" };
  mocks.relayModel.mockResolvedValueOnce({
    status: "confirmationRequired",
    targetName: "Example",
    catalog,
    quitRequired: false,
  });
  mocks.apply
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: "Example",
      catalog: {
        ...catalog,
        choice: "manage",
        afterPointer: "/managed/models.json",
      },
      quitRequired: false,
    })
    .mockResolvedValueOnce({
      status: "switched",
      warnings: [],
      chatgptWasRunning: false,
      chatgptRelaunched: false,
    });
  const { result } = renderHook(
    () => useApplicationOverview("codex", providers, vi.fn()),
    { wrapper },
  );
  await act(async () => {
    await result.current.select(target, undefined, "model-b");
  });
  expect(result.current.catalog).toEqual(catalog);
  expect(result.current.quitRequired).toBe(false);
  await act(async () => {
    await result.current.previewCatalog(decision);
  });
  expect(mocks.apply).toHaveBeenNthCalledWith(
    1,
    "codex",
    {
      selection: { providerId: target.providerId, model: "model-b" },
      catalogDecision: decision,
      catalogPreviewOnly: true,
    },
    undefined,
  );
  expect(mocks.success).not.toHaveBeenCalled();
  await act(async () => {
    await result.current.confirm(false, decision);
  });
  expect(mocks.apply).toHaveBeenNthCalledWith(
    2,
    "codex",
    {
      selection: { providerId: target.providerId, model: "model-b" },
      catalogDecision: decision,
      catalogPreviewOnly: false,
    },
    false,
  );
  expect(mocks.success).toHaveBeenCalledOnce();
  expect(result.current.confirmation).toBeNull();
});

it("drops a catalog confirmation when the application changes", async () => {
  const catalog = {
    ownership: "unclaimedExternal",
    choice: "keep",
    canManage: true,
    revision: "catalog-v1",
    beforePointer: "/external/models.json",
    afterPointer: "/external/models.json",
  };
  mocks.relay.mockResolvedValueOnce({
    status: "confirmationRequired",
    targetName: "Example",
    catalog,
  });
  const { result, rerender } = renderHook(
    ({ app }: { app: "codex" | "claude" }) =>
      useApplicationOverview(app, providers, vi.fn()),
    { wrapper, initialProps: { app: "codex" as "codex" | "claude" } },
  );
  await act(async () => {
    await result.current.select(target);
  });
  expect(result.current.catalog).toEqual(catalog);
  rerender({ app: "claude" });
  expect(result.current.confirmation).toBeNull();
  await act(async () => {
    await result.current.confirm(false, {
      choice: "keep",
      revision: "catalog-v1",
    });
  });
  expect(mocks.apply).not.toHaveBeenCalled();
});
