import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { useApplicationRouting } from "../useApplicationRouting";
const mocks = vi.hoisted(() => ({
  get: vi.fn(),
  order: vi.fn(),
  apply: vi.fn(),
  failover: vi.fn(),
  status: vi.fn(),
  takeover: vi.fn(),
  start: vi.fn(),
  setTakeover: vi.fn(),
  error: vi.fn(),
  success: vi.fn(),
}));
vi.mock("@/lib/api/applicationRouting", () => ({
  applicationRoutingApi: {
    get: mocks.get,
    setOrder: mocks.order,
    apply: mocks.apply,
    setFailover: mocks.failover,
  },
}));
vi.mock("@/lib/api/proxy", () => ({
  proxyApi: {
    getProxyStatus: mocks.status,
    getProxyTakeoverStatus: mocks.takeover,
    startProxyServer: mocks.start,
    setProxyTakeoverForApp: mocks.setTakeover,
  },
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("sonner", () => ({
  toast: { error: mocks.error, success: mocks.success },
}));
let client: QueryClient;
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={client}>{children}</QueryClientProvider>
);
beforeEach(() => {
  vi.resetAllMocks();
  client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  mocks.get.mockResolvedValue({ autoFailoverEnabled: false, tiers: [] });
  mocks.status.mockResolvedValue({ running: false });
  mocks.takeover.mockResolvedValue({ codex: false });
  mocks.start.mockResolvedValue({});
  mocks.setTakeover.mockResolvedValue(undefined);
  mocks.failover.mockResolvedValue(undefined);
  mocks.order.mockResolvedValue(undefined);
});
describe("application routing controls", () => {
  it("reads the page without starting or reconfiguring routing", async () => {
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(mocks.start).not.toHaveBeenCalled();
    expect(mocks.setTakeover).not.toHaveBeenCalled();
    expect(mocks.failover).not.toHaveBeenCalled();
  });
  it("enables routing through the backend operation", async () => {
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.data).toBeDefined());
    await act(async () => {
      await result.current.setFailover(true);
    });
    expect(mocks.failover).toHaveBeenCalledWith("codex", true);
    expect(mocks.start).not.toHaveBeenCalled();
    expect(mocks.setTakeover).not.toHaveBeenCalled();
    expect(mocks.success).toHaveBeenCalledWith("applications.failoverEnabled");
  });
  it("disabling changes only fallback permission", async () => {
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.data).toBeDefined());
    await act(async () => {
      await result.current.setFailover(false);
    });
    expect(mocks.failover).toHaveBeenCalledWith("codex", false);
    expect(mocks.start).not.toHaveBeenCalled();
    expect(mocks.setTakeover).not.toHaveBeenCalled();
  });
  it("exposes backend activation failure without reporting success", async () => {
    mocks.failover.mockRejectedValue(new Error("configuration is busy"));
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.data).toBeDefined());
    await act(async () => {
      await expect(result.current.setFailover(true)).rejects.toThrow(
        "configuration is busy",
      );
    });
    expect(mocks.success).not.toHaveBeenCalled();
    expect(mocks.error).toHaveBeenCalled();
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ["applicationRouting", "codex"],
    });
  });
  it("saves the complete priority order without changing fallback or current tier", async () => {
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.data).toBeDefined());
    await act(async () => {
      await result.current.setOrder(["b", "a"]);
    });
    expect(mocks.order).toHaveBeenCalledWith("codex", ["b", "a"]);
    expect(mocks.failover).not.toHaveBeenCalled();
    expect(mocks.setTakeover).not.toHaveBeenCalled();
  });
  it("refreshes application state after success or failure but not while awaiting confirmation", async () => {
    mocks.apply
      .mockResolvedValueOnce({
        status: "confirmationRequired",
        targetName: "Example",
      })
      .mockResolvedValueOnce({
        status: "switched",
        providerName: "Example",
        warnings: [],
        chatgptWasRunning: false,
        chatgptRelaunched: false,
      })
      .mockRejectedValueOnce(new Error("configuration write failed"));
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.data).toBeDefined());
    const change = {
      order: { profileName: "Travel", providerIds: ["b"] },
      selection: { providerId: "b", model: "model-b" },
    };
    await act(async () => {
      await result.current.apply(change);
    });
    expect(invalidate).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.apply(change, false);
    });
    expect(mocks.apply).toHaveBeenLastCalledWith("codex", change, false);
    for (const owner of [
      "applicationRouting",
      "applicationOverview",
      "providers",
      "orderProfiles",
    ]) {
      expect(invalidate).toHaveBeenCalledWith({ queryKey: [owner, "codex"] });
    }
    expect(mocks.order).not.toHaveBeenCalled();
    invalidate.mockClear();
    await act(async () => {
      await expect(result.current.apply(change, false)).rejects.toThrow(
        "configuration write failed",
      );
    });
    for (const owner of [
      "applicationRouting",
      "applicationOverview",
      "providers",
      "orderProfiles",
    ]) {
      expect(invalidate).toHaveBeenCalledWith({ queryKey: [owner, "codex"] });
    }
  });
  it("keeps the last successful read and blocks writes when the owner requires verification", async () => {
    const last = {
      autoFailoverEnabled: true,
      routingActive: true,
      tiers: [{ providerId: "a" }],
    };
    mocks.get.mockResolvedValue(last);
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.data).toEqual(last));
    const changedAt = result.current.dataUpdatedAt;
    mocks.get.mockRejectedValue({
      code: "mode.verification_required",
      modeState: {
        status: "pending",
        mode: null,
        attached: null,
        currentProviderId: null,
        directProviderId: null,
        publicationStarted: true,
        canWrite: false,
        canRecheck: true,
      },
    });
    await act(async () => {
      await result.current.refetch();
    });
    expect(result.current.data).toEqual(last);
    expect(result.current.dataUpdatedAt).toBe(changedAt);
    await waitFor(() => expect(result.current.writeBlocked).toBe(true));
    expect(result.current.modeState?.status).toBe("pending");
    await act(async () => {
      await expect(
        result.current.apply({ selection: { providerId: "b" } }),
      ).rejects.toThrow("mode.verification_required");
      await expect(result.current.setFailover(true)).rejects.toThrow(
        "mode.verification_required",
      );
      await result.current.refetch();
    });
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(mocks.failover).not.toHaveBeenCalled();
    expect(result.current.data).toEqual(last);
  });
});
