import { useQoderCnCampaigns } from "@/components/providers/qoder/useQoderCnCampaigns";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import type { PropsWithChildren } from "react";
import { useQoderDailyClaim } from "@/components/providers/qoder/useQoderDailyClaim";
import { useQoderInferenceIdentity } from "@/components/providers/qoder/useQoderInferenceIdentity";
import { useQoderCnDeviceLogin } from "@/components/providers/qoder/useQoderCnDeviceLogin";
import { useQoderAutomaticClaim } from "@/components/providers/qoder/useQoderAutomaticClaim";

const campaigns = vi.hoisted(() => ({
  automaticContext: vi.fn(),
  setAutomatic: vi.fn(),
  read: vi.fn(),
  claim: vi.fn(),
}));
vi.mock("@/lib/api/qoderCnCampaigns", () => ({
  qoderCnCampaignsApi: campaigns,
}));
const api = vi.hoisted(() => ({
  context: vi.fn(),
  claim: vi.fn(),
  identity: vi.fn(),
  action: vi.fn(),
  start: vi.fn(),
  poll: vi.fn(),
  cancel: vi.fn(),
  automaticContext: vi.fn(),
  setAutomatic: vi.fn(),
  openExternal: vi.fn(),
}));
vi.mock("@/lib/api/qoderDaily", () => ({ qoderDailyApi: api }));
vi.mock("@/lib/api/qoderHttpAuth", () => ({
  qoderHttpAuthApi: { context: api.identity, action: api.action },
}));
vi.mock("@/lib/api/qoderCnDeviceLogin", () => ({ qoderCnDeviceLoginApi: api }));
vi.mock("@/lib/api/settings", () => ({
  settingsApi: { openExternal: api.openExternal },
}));
const context = {
  providerId: "fixture",
  credentialGeneration: "generation-a",
  deviceDependency: null,
};
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
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function wrapper({ children }: PropsWithChildren) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
beforeEach(() => {
  vi.resetAllMocks();
  api.context.mockResolvedValue(context);
  api.automaticContext.mockResolvedValue(policy);
  api.cancel.mockResolvedValue({ status: "cancelled" });
  api.identity.mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-a",
    source: "chinaQoderPat",
    status: "unestablished",
    expiresAtMs: null,
    allowedActions: ["establish"],
  });
});
it("daily command owns declaration, serializes same-turn claims, and drops old epoch results", async () => {
  const pending = deferred<{ Unavailable: string }>();
  api.claim.mockReturnValue(pending.promise);
  const { result, rerender } = renderHook(
    ({ providerId }) => useQoderDailyClaim(providerId, false),
    { initialProps: { providerId: "fixture" } },
  );
  expect(api.context).not.toHaveBeenCalled();
  act(() => result.current.setOpen(true));
  await waitFor(() => expect(result.current.context).toEqual(context));
  act(() => result.current.setDeclared(true));
  expect(result.current.ready).toBe(true);
  act(() => {
    void result.current.claim();
    void result.current.claim();
  });
  expect(api.claim).toHaveBeenCalledTimes(1);
  rerender({ providerId: "other" });
  await act(async () =>
    pending.resolve({ Unavailable: "AuthFamilyUnverified" }),
  );
  expect(result.current.result).toBeNull();
  expect(result.current.declared).toBe(false);
});
it("identity command owns permission checks and clears authorization after response loss without replay", async () => {
  api.action.mockRejectedValue(new Error("synthetic response loss"));
  const { result } = renderHook(
    () => useQoderInferenceIdentity({ providerId: "fixture" }),
    { wrapper },
  );
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.actions).toEqual(["establish"]));
  act(() => result.current.setDeclared(true));
  await act(async () => result.current.perform("rebind"));
  expect(api.action).not.toHaveBeenCalled();
  await act(async () => result.current.perform("establish"));
  expect(api.action).toHaveBeenCalledTimes(1);
  expect(result.current.canAct).toBe(false);
  expect(result.current.visibleContext).toBeNull();
  expect(result.current.feedback?.role).toBe("alert");
});
it("device command cancels a grant whose start arrives after close without opening browser or polling", async () => {
  const pending = deferred<unknown>();
  api.start.mockReturnValue(pending.promise);
  const { result } = renderHook(() => useQoderCnDeviceLogin({}), { wrapper });
  act(() => result.current.openFor(false));
  act(() => {
    void result.current.start();
  });
  act(() => result.current.close());
  await act(async () =>
    pending.resolve({
      status: "started",
      sessionId: "session-a",
      providerId: "fixture",
      credentialGeneration: "generation-a",
      authUrl: "https://qoder.cn/device/selectAccounts?session=synthetic",
      expiresAtMs: Date.now() + 60000,
      pollIntervalMs: 1000,
    }),
  );
  expect(result.current.open).toBe(false);
  expect(result.current.phase).toBe("idle");
  expect(api.cancel).toHaveBeenCalledTimes(1);
  expect(api.cancel).toHaveBeenCalledWith("session-a");
  expect(api.poll).not.toHaveBeenCalled();
  expect(api.openExternal).not.toHaveBeenCalled();
});
it("automatic owner retains uncertain write across closing and a different credential generation", async () => {
  api.setAutomatic.mockRejectedValue(new Error("synthetic response loss"));
  const { result, rerender } = renderHook(
    ({ open }) => useQoderAutomaticClaim("fixture", open, false),
    { initialProps: { open: true } },
  );
  await waitFor(() => expect(result.current.canDeclare).toBe(true));
  act(() => result.current.declare(true));
  await act(async () => result.current.setEnabled(true));
  expect(result.current.unknown).toBe(true);
  rerender({ open: false });
  api.automaticContext.mockResolvedValue({
    ...policy,
    credentialGeneration: "generation-b",
    revision: 100,
  });
  rerender({ open: true });
  await waitFor(() =>
    expect(result.current.context?.credentialGeneration).toBe("generation-b"),
  );
  expect(result.current.unknown).toBe(true);
  expect(result.current.canChange).toBe(false);
  await act(async () => result.current.setEnabled(true));
  expect(api.setAutomatic).toHaveBeenCalledTimes(1);
});

it("CN policy command rejects enabling when backend capability denies it", async () => {
  api.identity.mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-a",
    source: "chinaQoderPat",
    status: "ready",
    allowedActions: [],
    expiresAtMs: null,
  });
  campaigns.automaticContext.mockResolvedValue({
    ...policy,
    policyVersion: 2,
    canEnable: false,
  });
  const { result } = renderHook(() =>
    useQoderCnCampaigns({ providerId: "fixture" }),
  );
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.policy).not.toBeNull());
  expect(result.current.canSetAutomatic).toBe(false);
  await act(async () => result.current.setAutomatic());
  expect(campaigns.setAutomatic).not.toHaveBeenCalled();
});
it("CN policy response loss stays uncertain after reopen and a new generation cannot confirm an old write", async () => {
  api.identity.mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-a",
    source: "chinaQoderPat",
    status: "ready",
    allowedActions: [],
    expiresAtMs: null,
  });
  campaigns.automaticContext.mockResolvedValue({ ...policy, policyVersion: 2 });
  campaigns.setAutomatic.mockRejectedValue(
    new Error("synthetic response loss"),
  );
  const { result } = renderHook(() =>
    useQoderCnCampaigns({ providerId: "fixture" }),
  );
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.canSetAutomatic).toBe(true));
  await act(async () => result.current.setAutomatic());
  act(() => result.current.changeOpen(false));
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.policy).not.toBeNull());
  expect(result.current.canSetAutomatic).toBe(false);
  await act(async () => result.current.setAutomatic());
  expect(campaigns.setAutomatic).toHaveBeenCalledTimes(1);
  act(() => result.current.changeOpen(false));
  api.identity.mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-b",
    source: "chinaQoderPat",
    status: "ready",
    allowedActions: [],
    expiresAtMs: null,
  });
  campaigns.automaticContext.mockResolvedValue({
    ...policy,
    policyVersion: 2,
    credentialGeneration: "generation-b",
    revision: 100,
  });
  act(() => result.current.changeOpen(true));
  await waitFor(() =>
    expect(result.current.policy?.credentialGeneration).toBe("generation-b"),
  );
  expect(result.current.canSetAutomatic).toBe(false);
  act(() => result.current.changeOpen(false));
  api.identity.mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-a",
    source: "chinaQoderPat",
    status: "ready",
    allowedActions: [],
    expiresAtMs: null,
  });
  campaigns.automaticContext.mockResolvedValue({
    ...policy,
    policyVersion: 2,
    revision: 5,
    enabled: true,
  });
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.policy?.revision).toBe(5));
  expect(result.current.canSetAutomatic).toBe(true);
});
it("CN read and claim share their original single-flight boundary and reject stale source results", async () => {
  api.identity.mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-a",
    source: "chinaQoderPat",
    status: "ready",
    allowedActions: [],
    expiresAtMs: null,
  });
  campaigns.automaticContext.mockResolvedValue({ ...policy, policyVersion: 2 });
  const pending = deferred<unknown>();
  campaigns.claim.mockReturnValue(pending.promise);
  const { result, rerender } = renderHook(
    ({ providerId }) => useQoderCnCampaigns({ providerId }),
    { initialProps: { providerId: "fixture" } },
  );
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.canRead).toBe(true));
  act(() => {
    void result.current.claim();
    void result.current.claim();
    void result.current.read();
  });
  expect(campaigns.claim).toHaveBeenCalledTimes(1);
  expect(campaigns.read).not.toHaveBeenCalled();
  rerender({ providerId: "other" });
  await act(async () =>
    pending.resolve({
      status: "completed",
      providerId: "fixture",
      credentialGeneration: "generation-a",
      activities: [],
    }),
  );
  expect(result.current.benefits).toBeNull();
});

it("a retained daily command cannot dispatch after its provider epoch changed", async () => {
  const { result, rerender } = renderHook(
    ({ providerId }) => useQoderDailyClaim(providerId),
    { initialProps: { providerId: "fixture" } },
  );
  act(() => result.current.setOpen(true));
  await waitFor(() => expect(result.current.context).toEqual(context));
  act(() => result.current.setDeclared(true));
  const staleClaim = result.current.claim;
  rerender({ providerId: "other" });
  await act(async () => staleClaim());
  expect(api.claim).not.toHaveBeenCalled();
});
it("a retained identity command cannot dispatch after its source epoch changed", async () => {
  const { result, rerender } = renderHook(
    ({ sourceGeneration }) =>
      useQoderInferenceIdentity({ providerId: "fixture", sourceGeneration }),
    { initialProps: { sourceGeneration: "generation-a" }, wrapper },
  );
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.actions).toEqual(["establish"]));
  act(() => result.current.setDeclared(true));
  const stalePerform = result.current.perform;
  rerender({ sourceGeneration: "generation-b" });
  await act(async () => stalePerform("establish"));
  expect(api.action).not.toHaveBeenCalled();
});
it("a retained device start cannot dispatch after its dialog closes", async () => {
  const { result } = renderHook(() => useQoderCnDeviceLogin({}), { wrapper });
  act(() => result.current.openFor(false));
  const staleStart = result.current.start;
  act(() => result.current.close());
  await act(async () => staleStart());
  expect(api.start).not.toHaveBeenCalled();
});
it("a retained CN claim cannot dispatch after the provider epoch changes", async () => {
  api.identity.mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-a",
    source: "chinaQoderPat",
    status: "ready",
    allowedActions: [],
    expiresAtMs: null,
  });
  campaigns.automaticContext.mockResolvedValue({ ...policy, policyVersion: 2 });
  const { result, rerender } = renderHook(
    ({ providerId }) => useQoderCnCampaigns({ providerId }),
    { initialProps: { providerId: "fixture" } },
  );
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.canRead).toBe(true));
  const staleClaim = result.current.claim;
  rerender({ providerId: "other" });
  await act(async () => staleClaim());
  expect(campaigns.claim).not.toHaveBeenCalled();
});

it("a retained automatic setter cannot consume consent after close", async () => {
  const { result, rerender } = renderHook(
    ({ open }) => useQoderAutomaticClaim("fixture", open, false),
    { initialProps: { open: true } },
  );
  await waitFor(() => expect(result.current.canDeclare).toBe(true));
  act(() => result.current.declare(true));
  const staleSet = result.current.setEnabled;
  rerender({ open: false });
  await act(async () => staleSet(true));
  expect(api.setAutomatic).not.toHaveBeenCalled();
});

it.each([4, 5])(
  "CN policy read at revision %s cannot regress a late confirmed write",
  async (readRevision) => {
    api.identity.mockResolvedValue({
      providerId: "fixture",
      credentialGeneration: "generation-a",
      source: "chinaQoderPat",
      status: "ready",
      allowedActions: [],
      expiresAtMs: null,
    });
    campaigns.automaticContext.mockResolvedValue({
      ...policy,
      policyVersion: 2,
    });
    const write = deferred<unknown>();
    const read = deferred<unknown>();
    campaigns.setAutomatic.mockReturnValue(write.promise);
    const { result } = renderHook(() =>
      useQoderCnCampaigns({ providerId: "fixture" }),
    );
    act(() => result.current.changeOpen(true));
    await waitFor(() => expect(result.current.canSetAutomatic).toBe(true));
    act(() => {
      void result.current.setAutomatic();
    });
    act(() => result.current.changeOpen(false));
    campaigns.automaticContext.mockReturnValue(read.promise);
    act(() => result.current.changeOpen(true));
    await waitFor(() =>
      expect(campaigns.automaticContext).toHaveBeenCalledTimes(2),
    );
    await act(async () =>
      write.resolve({
        ...policy,
        policyVersion: 2,
        revision: 6,
        enabled: true,
      }),
    );
    await act(async () =>
      read.resolve({ ...policy, policyVersion: 2, revision: readRevision }),
    );
    expect(result.current.policy?.revision).toBe(6);
    expect(result.current.policy?.enabled).toBe(true);
  },
);

it("a disabled device VM cannot open authorization through its command", () => {
  const { result } = renderHook(
    () => useQoderCnDeviceLogin({ disabled: true }),
    { wrapper },
  );
  act(() => result.current.openFor(false));
  expect(result.current.open).toBe(false);
});
it("a retained device cancel cannot abandon a newer grant attempt", async () => {
  api.start.mockResolvedValue({
    status: "started",
    sessionId: "new-session",
    providerId: "fixture",
    credentialGeneration: "generation-a",
    authUrl: "https://qoder.cn/device/selectAccounts?session=synthetic",
    expiresAtMs: Date.now() + 60000,
    pollIntervalMs: 1000,
  });
  api.openExternal.mockResolvedValue(undefined);
  const { result } = renderHook(() => useQoderCnDeviceLogin({}), { wrapper });
  act(() => result.current.openFor(false));
  const staleCancel = result.current.cancel;
  act(() => result.current.close());
  act(() => result.current.openFor(false));
  await act(async () => result.current.start());
  expect(result.current.phase).toBe("waiting");
  act(() => staleCancel());
  expect(result.current.phase).toBe("waiting");
  expect(api.cancel).not.toHaveBeenCalled();
  act(() => result.current.cancel());
});

it.each(["close", "changeOpen"] as const)(
  "a retained device %s cannot abandon a newer grant attempt",
  async (command) => {
    api.start.mockResolvedValue({
      status: "started",
      sessionId: "new-session",
      providerId: "fixture",
      credentialGeneration: "generation-a",
      authUrl: "https://qoder.cn/device/selectAccounts?session=synthetic",
      expiresAtMs: Date.now() + 60000,
      pollIntervalMs: 1000,
    });
    api.openExternal.mockResolvedValue(undefined);
    const { result } = renderHook(() => useQoderCnDeviceLogin({}), { wrapper });
    act(() => result.current.openFor(false));
    const oldChangeOpen = result.current.changeOpen;
    const staleClose =
      command === "close" ? result.current.close : () => oldChangeOpen(false);
    act(() => result.current.close());
    act(() => result.current.openFor(false));
    await act(async () => result.current.start());
    act(() => staleClose());
    expect(result.current.phase).toBe("waiting");
    expect(result.current.open).toBe(true);
    expect(api.cancel).not.toHaveBeenCalled();
    act(() => result.current.close());
  },
);

it("CN late setter receipt cannot regress a higher revision confirmed by readback", async () => {
  api.identity.mockResolvedValue({
    providerId: "fixture",
    credentialGeneration: "generation-a",
    source: "chinaQoderPat",
    status: "ready",
    allowedActions: [],
    expiresAtMs: null,
  });
  campaigns.automaticContext.mockResolvedValue({ ...policy, policyVersion: 2 });
  const write = deferred<unknown>();
  campaigns.setAutomatic.mockReturnValue(write.promise);
  const { result } = renderHook(() =>
    useQoderCnCampaigns({ providerId: "fixture" }),
  );
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.canSetAutomatic).toBe(true));
  act(() => {
    void result.current.setAutomatic();
  });
  act(() => result.current.changeOpen(false));
  campaigns.automaticContext.mockResolvedValue({
    ...policy,
    policyVersion: 2,
    revision: 6,
  });
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.policy?.revision).toBe(6));
  await act(async () =>
    write.resolve({ ...policy, policyVersion: 2, revision: 5, enabled: true }),
  );
  act(() => result.current.changeOpen(false));
  campaigns.automaticContext.mockResolvedValue({ ...policy, policyVersion: 2 });
  act(() => result.current.changeOpen(true));
  await waitFor(() => expect(result.current.policy).not.toBeNull());
  expect(result.current.policy?.revision).toBe(6);
  expect(result.current.policy?.enabled).toBe(false);
});
