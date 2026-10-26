import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { useApplicationRouting } from "../useApplicationRouting";
import {
  applicationRoutingApi,
  type ApplicationRouting,
} from "@/lib/api/applicationRouting";
import { failoverApi } from "@/lib/api/failover";
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
let client: QueryClient;
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={client}>{children}</QueryClientProvider>
);
const facts = (): ApplicationRouting => ({
  autoFailoverEnabled: false,
  routingActive: false,
  tiers: [],
  model: null,
  modelOptions: [],
  chainIds: ["a", "b"],
});
beforeEach(() => {
  client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  vi.spyOn(applicationRoutingApi, "get").mockResolvedValue(facts());
  vi.spyOn(applicationRoutingApi, "setOrder").mockResolvedValue(undefined);
  vi.spyOn(applicationRoutingApi, "setTierBlocked").mockResolvedValue(
    undefined,
  );
  vi.spyOn(applicationRoutingApi, "setFailover").mockResolvedValue(undefined);
  vi.spyOn(applicationRoutingApi, "apply");
  vi.spyOn(failoverApi, "resetCircuitBreaker").mockResolvedValue(undefined);
});
afterEach(() => {
  client.clear();
  vi.restoreAllMocks();
});
function mount() {
  return renderHook(() => useApplicationRouting("opencode"), { wrapper });
}
describe("non-proxy routing facts and immediate actions", () => {
  it("reads without starting routing, changing fallback, applying a target, or writing order", async () => {
    const { result } = mount();
    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(applicationRoutingApi.setFailover).not.toHaveBeenCalled();
    expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
    expect(applicationRoutingApi.setOrder).not.toHaveBeenCalled();
  });
  it("saves native display order immediately and refreshes its source facts", async () => {
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const { result } = mount();
    await waitFor(() => expect(result.current.data).toBeDefined());
    await act(async () => {
      await result.current.setOrder(["b", "a"]);
    });
    expect(applicationRoutingApi.setOrder).toHaveBeenCalledExactlyOnceWith(
      "opencode",
      ["b", "a"],
    );
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ["applicationRouting", "opencode"],
    });
    expect(applicationRoutingApi.setFailover).not.toHaveBeenCalled();
    expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
  });
  it("retains explicit unblock and error reset actions through their existing owners", async () => {
    const { result } = mount();
    await waitFor(() => expect(result.current.data).toBeDefined());
    await act(async () => {
      await result.current.blockTier({ providerId: "a", blocked: false });
    });
    await act(async () => {
      await result.current.resetTierErrors({ providerId: "b" });
    });
    expect(
      applicationRoutingApi.setTierBlocked,
    ).toHaveBeenCalledExactlyOnceWith("opencode", "a", false);
    expect(failoverApi.resetCircuitBreaker).toHaveBeenCalledExactlyOnceWith(
      "b",
      "opencode",
    );
    expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
  });
  it("exposes a rejected immediate write without replaying it on refresh", async () => {
    vi.mocked(applicationRoutingApi.setOrder).mockRejectedValue(
      new Error("native order rejected"),
    );
    const { result } = mount();
    await waitFor(() => expect(result.current.data).toBeDefined());
    await act(async () => {
      await expect(result.current.setOrder(["b", "a"])).rejects.toThrow(
        "native order rejected",
      );
    });
    await act(async () => {
      await result.current.refetch();
    });
    expect(applicationRoutingApi.setOrder).toHaveBeenCalledTimes(1);
    expect(result.current.data?.chainIds).toEqual(["a", "b"]);
  });
  it("keeps the last read, blocks every write on read failure, and retries only reads", async () => {
    const { result } = mount();
    await waitFor(() => expect(result.current.data).toBeDefined());
    const previous = result.current.data;
    const updated = result.current.dataUpdatedAt;
    vi.mocked(applicationRoutingApi.get).mockRejectedValue(
      new Error("synthetic read unavailable"),
    );
    await act(async () => {
      await result.current.refetch();
    });
    expect(result.current.data).toEqual(previous);
    expect(result.current.dataUpdatedAt).toBe(updated);
    await waitFor(() => expect(result.current.writeBlocked).toBe(true));
    await expect(result.current.setOrder(["b", "a"])).rejects.toThrow(
      "mode.verification_required",
    );
    await expect(
      result.current.blockTier({ providerId: "a", blocked: false }),
    ).rejects.toThrow("mode.verification_required");
    await expect(
      result.current.resetTierErrors({ providerId: "a" }),
    ).rejects.toThrow("mode.verification_required");
    vi.mocked(applicationRoutingApi.get).mockResolvedValue(facts());
    await act(async () => {
      await result.current.refetch();
    });
    await waitFor(() => expect(result.current.writeBlocked).toBe(false));
    expect(applicationRoutingApi.setOrder).not.toHaveBeenCalled();
    expect(applicationRoutingApi.setTierBlocked).not.toHaveBeenCalled();
    expect(failoverApi.resetCircuitBreaker).not.toHaveBeenCalled();
  });
});
