import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { useSharedRouteWorkspace } from "../useSharedRouteWorkspace";
import {
  sharedRoutingApi,
  type SharedRoutingState,
  type PreparedRouteTarget,
  type RouteTarget,
} from "@/lib/api/sharedRouting";
import { serviceSourcesApi } from "@/lib/api/serviceSources";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
vi.mock("@/lib/api/sharedRouting", () => ({
  sharedRoutingApi: {
    get: vi.fn(),
    prepareTarget: vi.fn(),
    applyTarget: vi.fn(),
    queryTarget: vi.fn(),
    recoverTarget: vi.fn(),
  },
}));
vi.mock("@/lib/api/serviceSources", () => ({
  serviceSourcesApi: { list: vi.fn() },
}));
vi.mock("@/lib/api/applicationRouting", () => ({
  applicationRoutingApi: { get: vi.fn() },
}));
const request = {
  id: "synthetic-native-receipt",
  scope: "applicationRouting" as const,
  draftDigest: "native-draft",
  revision: "native-revision",
};
const target: RouteTarget = { kind: "orderedGroup", groupId: "daily" };
const prepared: PreparedRouteTarget = {
  target,
  request,
  expectedBindingRevision: 2,
  groupRevision: 3,
};
let state: SharedRoutingState;
beforeEach(() => {
  vi.clearAllMocks();
  state = {
    appId: "codex",
    binding: {
      revision: 2,
      target: { kind: "manual", sourceId: "saved-source", model: "model-a" },
    },
    groups: [
      {
        id: "daily",
        name: "Daily",
        revision: 3,
        members: [
          { sourceId: "missing-source", model: "model-z" },
          { sourceId: "saved-source", model: "model-a" },
        ],
      },
    ],
    pendingRequest: null,
    canApply: true,
    blockedReason: null,
  };
  vi.mocked(sharedRoutingApi.get).mockImplementation(async () => state);
  vi.mocked(serviceSourcesApi.list).mockResolvedValue([
    {
      sourceId: "saved-source",
      name: "Saved source",
      account: { kind: "vendor", id: 1 },
      serviceName: null,
      accountLabel: null,
      configurationName: null,
      bindings: [
        {
          appId: "codex",
          providerId: "saved-provider",
          model: "model-a",
          usageState: "current",
          canSelect: true,
        },
      ],
    },
  ]);
  vi.mocked(applicationRoutingApi.get).mockResolvedValue({
    modeState: {
      status: "ready",
      mode: "proxy",
      attached: true,
      currentProviderId: "saved-provider",
      directProviderId: null,
      publicationStarted: false,
      canWrite: true,
      writeProtocol: "native",
      blockedReason: null,
      canRecheck: true,
    },
    autoFailoverEnabled: false,
    routingActive: false,
    model: "model-a",
    modelOptions: [],
    chainIds: [],
    tiers: [],
  });
  vi.mocked(sharedRoutingApi.prepareTarget).mockResolvedValue(prepared);
  vi.mocked(sharedRoutingApi.applyTarget).mockImplementation(async () => {
    state = { ...state, binding: { revision: 3, target } };
    return { request, status: "completed" };
  });
  vi.mocked(sharedRoutingApi.queryTarget).mockResolvedValue({
    request,
    status: "unknown",
  });
});
function mount(app = "codex") {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const hook = renderHook(
    ({ appId }) => useSharedRouteWorkspace(appId as "codex"),
    {
      initialProps: { appId: app },
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  return { client, ...hook };
}
it("loads authoritative saved facts and keeps selection as an unsaved local draft", async () => {
  const { result } = mount();
  await waitFor(() => expect(result.current.state?.binding?.revision).toBe(2));
  expect(result.current.draft).toBeNull();
  expect(result.current.hasPendingChanges).toBe(false);
  act(() => result.current.setDraft(target));
  expect(result.current.state?.binding?.target.kind).toBe("manual");
  expect(result.current.draft).toEqual(target);
  expect(result.current.hasPendingChanges).toBe(true);
  expect(sharedRoutingApi.prepareTarget).not.toHaveBeenCalled();
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
  expect(
    result.current.state?.groups[0].members.map((m) => m.sourceId),
  ).toEqual(["missing-source", "saved-source"]);
});
it("prepares and explicitly confirms the exact native receipt without changing actual-request facts", async () => {
  const { result } = mount();
  await waitFor(() => expect(result.current.state).toBeDefined());
  act(() => result.current.setDraft(target));
  await act(() => result.current.prepare());
  expect(sharedRoutingApi.prepareTarget).toHaveBeenCalledWith("codex", target);
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
  expect(result.current.prepared).toEqual(prepared);
  await act(() => result.current.confirm());
  expect(sharedRoutingApi.applyTarget).toHaveBeenCalledExactlyOnceWith(
    "codex",
    prepared,
  );
  expect(result.current.draft).toBeNull();
  expect(result.current.notice).toBe("saved");
  expect(result.current.connection?.routingActive).toBe(false);
});
it.each([false, "true"])(
  "does not prepare when canApply is not exact true (%j)",
  async (flag) => {
    state = { ...state, canApply: flag as boolean };
    const { result } = mount();
    await waitFor(() => expect(result.current.state).toBeDefined());
    act(() => result.current.setDraft(target));
    await act(() => result.current.prepare());
    expect(sharedRoutingApi.prepareTarget).not.toHaveBeenCalled();
  },
);
it("does not permit writes after failed source/state reads or fabricate empty inventory", async () => {
  vi.mocked(serviceSourcesApi.list).mockRejectedValue(
    new Error("synthetic inventory failure"),
  );
  const { result } = mount();
  await waitFor(() => expect(result.current.error).toBe(true));
  expect(result.current.sources).toBeUndefined();
  act(() => result.current.setDraft(target));
  await act(() => result.current.prepare());
  expect(sharedRoutingApi.prepareTarget).not.toHaveBeenCalled();
});
it("invalidates a prepared review when the user changes the draft", async () => {
  const { result } = mount();
  await waitFor(() => expect(result.current.state).toBeDefined());
  act(() => result.current.setDraft(target));
  await act(() => result.current.prepare());
  act(() =>
    result.current.setDraft({
      kind: "manual",
      sourceId: "saved-source",
      model: "model-b",
    }),
  );
  expect(result.current.prepared).toBeNull();
  await act(() => result.current.confirm());
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
});
it("checks a lost apply reply once and never automatically resubmits or recovers", async () => {
  vi.mocked(sharedRoutingApi.applyTarget).mockRejectedValue(
    new Error("synthetic lost reply"),
  );
  const { result } = mount();
  await waitFor(() => expect(result.current.state).toBeDefined());
  act(() => result.current.setDraft(target));
  await act(() => result.current.prepare());
  await act(() => result.current.confirm());
  expect(sharedRoutingApi.applyTarget).toHaveBeenCalledTimes(1);
  expect(sharedRoutingApi.queryTarget).toHaveBeenCalledExactlyOnceWith(
    "codex",
    request,
  );
  expect(sharedRoutingApi.recoverTarget).not.toHaveBeenCalled();
  expect(result.current.notice).toBe("unknown");
  expect(result.current.hasPendingChanges).toBe(true);
  await act(() => result.current.confirm());
  expect(sharedRoutingApi.applyTarget).toHaveBeenCalledTimes(1);
});
it.each(["pending", "verificationRequired", "unknown", "conflict"] as const)(
  "never reports %s as saved or clears the draft",
  async (status) => {
    vi.mocked(sharedRoutingApi.applyTarget).mockResolvedValue({
      request,
      status,
    });
    const { result } = mount();
    await waitFor(() => expect(result.current.state).toBeDefined());
    act(() => result.current.setDraft(target));
    await act(() => result.current.prepare());
    await act(() => result.current.confirm());
    expect(result.current.notice).not.toBe("saved");
    expect(result.current.draft).toEqual(target);
  },
);
it("refuses a result belonging to another request and preserves the original receipt", async () => {
  vi.mocked(sharedRoutingApi.applyTarget).mockResolvedValue({
    request: { ...request, id: "different" },
    status: "completed",
  });
  const { result } = mount();
  await waitFor(() => expect(result.current.state).toBeDefined());
  act(() => result.current.setDraft(target));
  await act(() => result.current.prepare());
  await act(() => result.current.confirm());
  expect(result.current.notice).toBe("unknown");
  expect(result.current.requestResult?.request).toEqual(request);
  expect(result.current.draft).toEqual(target);
});
it("drops late preparation after the draft or app changed", async () => {
  let resolve!: (value: PreparedRouteTarget) => void;
  vi.mocked(sharedRoutingApi.prepareTarget).mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const { result, rerender } = mount();
  await waitFor(() => expect(result.current.state).toBeDefined());
  act(() => result.current.setDraft(target));
  let operation!: Promise<void>;
  act(() => {
    operation = result.current.prepare();
  });
  act(() =>
    result.current.setDraft({
      kind: "manual",
      sourceId: "saved-source",
      model: "changed",
    }),
  );
  await act(async () => {
    resolve(prepared);
    await operation;
  });
  expect(result.current.prepared).toBeNull();
  rerender({ appId: "claude" });
  await waitFor(() => expect(result.current.draft).toBeNull());
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
});
it("uses only the original native pending receipt for explicit recovery", async () => {
  state = {
    ...state,
    pendingRequest: request,
    canApply: false,
    blockedReason: "routing_groups.recovery_required",
  };
  vi.mocked(sharedRoutingApi.recoverTarget).mockResolvedValue({
    request,
    status: "completed",
  });
  const { result } = mount();
  await waitFor(() => expect(result.current.pendingRequest).toEqual(request));
  await act(() => result.current.recoverRequest());
  expect(sharedRoutingApi.recoverTarget).toHaveBeenCalledExactlyOnceWith(
    "codex",
    request,
  );
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
  expect(sharedRoutingApi.prepareTarget).not.toHaveBeenCalled();
});
it("waits for all authoritative reads before even imperative preparation", async () => {
  vi.mocked(serviceSourcesApi.list).mockImplementation(
    () => new Promise(() => {}),
  );
  const { result } = mount();
  await waitFor(() => expect(result.current.state).toBeDefined());
  act(() => result.current.setDraft(target));
  await act(() => result.current.prepare());
  expect(sharedRoutingApi.prepareTarget).not.toHaveBeenCalled();
});
it("exposes stale review and never applies across a changed saved revision", async () => {
  const { result, client } = mount();
  await waitFor(() => expect(result.current.state).toBeDefined());
  act(() => result.current.setDraft(target));
  await act(() => result.current.prepare());
  expect(result.current.canConfirm).toBe(true);
  act(() =>
    client.setQueryData(["sharedRouting", "codex"], {
      ...state,
      binding: { ...state.binding, revision: 7 },
    }),
  );
  await waitFor(() => expect(result.current.canConfirm).toBe(false));
  await act(() => result.current.confirm());
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
  expect(result.current.notice).toBe("changed");
});
it.each(["notRecorded", "discarded", "abandoned"] as const)(
  "distinguishes known non-applied %s from an ambiguous outcome",
  async (status) => {
    vi.mocked(sharedRoutingApi.applyTarget).mockResolvedValue({
      request,
      status,
    });
    const { result } = mount();
    await waitFor(() => expect(result.current.state).toBeDefined());
    act(() => result.current.setDraft(target));
    await act(() => result.current.prepare());
    await act(() => result.current.confirm());
    expect(result.current.notice).toBe("notSaved");
    expect(result.current.draft).toEqual(target);
    expect(result.current.pendingRequest).toBeNull();
    expect(result.current.canPreview).toBe(true);
  },
);

it.each([
  new Error("generic read failure"),
  {
    modeState: {
      status: "unknown",
      canWrite: false,
      blockedReason: "mode.verification_required",
    },
  },
])(
  "does not recover an original journal from an ordinary or unknown connection error",
  async (error) => {
    state = {
      ...state,
      pendingRequest: request,
      canApply: false,
      blockedReason: "routing_groups.recovery_required",
    };
    vi.mocked(applicationRoutingApi.get).mockRejectedValue(error);
    const { result } = mount();
    await waitFor(() => expect(result.current.error).toBe(true));
    expect(result.current.canRecover).toBe(false);
    await act(() => result.current.recoverRequest());
    expect(sharedRoutingApi.recoverTarget).not.toHaveBeenCalled();
  },
);
it("rechecks original pending authority at invocation and never uses a stale recovery handler", async () => {
  state = {
    ...state,
    pendingRequest: request,
    canApply: false,
    blockedReason: "routing_groups.recovery_required",
  };
  const { result, client } = mount();
  await waitFor(() => expect(result.current.canRecover).toBe(true));
  const staleRecover = result.current.recoverRequest;
  act(() =>
    client.setQueryData(["sharedRouting", "codex"], {
      ...state,
      pendingRequest: { ...request, id: "replacement" },
    }),
  );
  await act(staleRecover);
  expect(sharedRoutingApi.recoverTarget).not.toHaveBeenCalled();
});
