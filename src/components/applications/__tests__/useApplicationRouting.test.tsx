import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { useApplicationRouting } from "../useApplicationRouting";
const mocks = vi.hoisted(() => ({
  get: vi.fn(),
  order: vi.fn(),
  failover: vi.fn(),
  status: vi.fn(),
  takeover: vi.fn(),
  start: vi.fn(),
  setTakeover: vi.fn(),
  error: vi.fn(),
}));
vi.mock("@/lib/api/applicationRouting", () => ({
  applicationRoutingApi: {
    get: mocks.get,
    setOrder: mocks.order,
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
vi.mock("sonner", () => ({ toast: { error: mocks.error } }));
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
  it("starts and takes over on explicit enable before enabling fallback", async () => {
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await act(async () => {
      await result.current.setFailover(true);
    });
    expect(mocks.start).toHaveBeenCalledTimes(1);
    expect(mocks.setTakeover).toHaveBeenCalledWith("codex", true);
    expect(mocks.failover).toHaveBeenCalledWith("codex", true);
    expect(mocks.start.mock.invocationCallOrder[0]).toBeLessThan(
      mocks.setTakeover.mock.invocationCallOrder[0],
    );
    expect(mocks.setTakeover.mock.invocationCallOrder[0]).toBeLessThan(
      mocks.failover.mock.invocationCallOrder[0],
    );
  });
  it("disabling changes only fallback permission", async () => {
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await act(async () => {
      await result.current.setFailover(false);
    });
    expect(mocks.failover).toHaveBeenCalledWith("codex", false);
    expect(mocks.start).not.toHaveBeenCalled();
    expect(mocks.setTakeover).not.toHaveBeenCalled();
  });
  it("does not enable fallback when takeover fails and exposes failure to the caller", async () => {
    mocks.setTakeover.mockRejectedValue(new Error("configuration is busy"));
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await act(async () => {
      await expect(result.current.setFailover(true)).rejects.toThrow(
        "configuration is busy",
      );
    });
    expect(mocks.failover).not.toHaveBeenCalled();
    expect(mocks.error).toHaveBeenCalled();
  });
  it("saves the complete priority order without changing fallback or current tier", async () => {
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await act(async () => {
      await result.current.setOrder(["b", "a"]);
    });
    expect(mocks.order).toHaveBeenCalledWith("codex", ["b", "a"]);
    expect(mocks.failover).not.toHaveBeenCalled();
    expect(mocks.setTakeover).not.toHaveBeenCalled();
  });
});
