import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import {
  applicationRoutingApi,
  type ApplicationRouting,
} from "@/lib/api/applicationRouting";
import { useApplicationRouting } from "../useApplicationRouting";
import { failoverApi } from "@/lib/api/failover";
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
const facts = (): ApplicationRouting => ({
  autoFailoverEnabled: false,
  routingActive: false,
  model: null,
  modelOptions: [],
  chainIds: [],
  tiers: [],
});
const blocked = {
  status: "unknown" as const,
  mode: null,
  attached: null,
  currentProviderId: null,
  directProviderId: null,
  publicationStarted: false,
  canWrite: false,
  canRecheck: true,
  writeProtocol: null,
  blockedReason: "upgrade.checkpoint_pending",
};
let client: QueryClient;
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={client}>{children}</QueryClientProvider>
);
beforeEach(() => {
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.spyOn(applicationRoutingApi, "get").mockResolvedValue(facts());
  vi.spyOn(applicationRoutingApi, "setOrder").mockResolvedValue(undefined);
});
afterEach(() => {
  client.clear();
  vi.restoreAllMocks();
});
it.each(["claude-desktop", "opencode", "openclaw", "hermes", "pi"] as const)(
  "does not fabricate managed mode requirements for independent %s",
  async (appId) => {
    const { result } = renderHook(() => useApplicationRouting(appId), {
      wrapper,
    });
    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(result.current.writeBlocked).toBe(false);
    await act(async () => {
      await result.current.setOrder(["a"]);
    });
    expect(applicationRoutingApi.setOrder).toHaveBeenCalledWith(appId, ["a"]);
  },
);
it.each(["claude", "codex", "gemini", "grokbuild"] as const)(
  "still fails closed if managed %s is accidentally passed without mode facts",
  async (appId) => {
    const { result } = renderHook(() => useApplicationRouting(appId), {
      wrapper,
    });
    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(result.current.writeBlocked).toBe(true);
    await expect(result.current.setOrder(["a"])).rejects.toThrow(
      "mode.verification_required",
    );
    expect(applicationRoutingApi.setOrder).not.toHaveBeenCalled();
  },
);
it("blocks loading and checkpoint state until a successful independent-app read recovers", async () => {
  let resolve!: (value: ApplicationRouting) => void;
  vi.mocked(applicationRoutingApi.get).mockReturnValueOnce(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const { result } = renderHook(() => useApplicationRouting("claude-desktop"), {
    wrapper,
  });
  expect(result.current.writeBlocked).toBe(true);
  await act(async () => resolve({ ...facts(), modeState: blocked }));
  await waitFor(() =>
    expect(result.current.modeState?.blockedReason).toBe(
      "upgrade.checkpoint_pending",
    ),
  );
  await expect(result.current.setOrder(["a"])).rejects.toThrow(
    "mode.verification_required",
  );
  await act(async () => {
    await result.current.refetch();
  });
  await waitFor(() => expect(result.current.writeBlocked).toBe(false));
  expect(applicationRoutingApi.setOrder).not.toHaveBeenCalled();
});
it("checks the current source cache before writes even before React has rendered the new owner state", async () => {
  const { result } = renderHook(() => useApplicationRouting("opencode"), {
    wrapper,
  });
  await waitFor(() => expect(result.current.writeBlocked).toBe(false));
  const setOrder = result.current.setOrder;
  client.setQueryData(["applicationRouting", "opencode"], {
    ...facts(),
    modeState: blocked,
  });
  await expect(setOrder(["a"])).rejects.toThrow("mode.verification_required");
  expect(applicationRoutingApi.setOrder).not.toHaveBeenCalled();
});
it("surfaces a blocking owner from a failed read without replacing retained routing facts", async () => {
  const { result } = renderHook(() => useApplicationRouting("pi"), { wrapper });
  await waitFor(() => expect(result.current.data).toBeDefined());
  const retained = result.current.data;
  vi.mocked(applicationRoutingApi.get).mockRejectedValue({
    modeState: blocked,
  });
  await act(async () => {
    await result.current.refetch();
  });
  expect(result.current.data).toEqual(retained);
  await waitFor(() => expect(result.current.modeState).toEqual(blocked));
  expect(result.current.writeBlocked).toBe(true);
  await expect(result.current.setOrder(["a"])).rejects.toThrow(
    "mode.verification_required",
  );
  expect(applicationRoutingApi.setOrder).not.toHaveBeenCalled();
});

it("keeps original proxy block/reset actions guarded without restoring the old route writer", async () => {
  const writable = {
    ...blocked,
    status: "ready" as const,
    mode: "proxy" as const,
    attached: true,
    canWrite: true,
    writeProtocol: "native" as const,
    blockedReason: null,
  };
  vi.mocked(applicationRoutingApi.get).mockResolvedValue({
    ...facts(),
    modeState: writable,
  });
  const block = vi
    .spyOn(applicationRoutingApi, "setTierBlocked")
    .mockResolvedValue(undefined);
  const reset = vi
    .spyOn(failoverApi, "resetCircuitBreaker")
    .mockResolvedValue(undefined);
  const { result } = renderHook(() => useApplicationRouting("codex"), {
    wrapper,
  });
  await waitFor(() => expect(result.current.writeBlocked).toBe(false));
  await act(async () => {
    await result.current.blockTier({ providerId: "p", blocked: false });
  });
  await act(async () => {
    await result.current.resetTierErrors({ providerId: "p" });
  });
  expect(block).toHaveBeenCalledExactlyOnceWith("codex", "p", false);
  expect(reset).toHaveBeenCalledExactlyOnceWith("p", "codex");
  const actions = result.current;
  client.setQueryData(["applicationRouting", "codex"], {
    ...facts(),
    modeState: blocked,
  });
  await expect(
    actions.blockTier({ providerId: "p", blocked: true }),
  ).rejects.toThrow("mode.verification_required");
  await expect(actions.resetTierErrors({ providerId: "p" })).rejects.toThrow(
    "mode.verification_required",
  );
  expect(block).toHaveBeenCalledTimes(1);
  expect(reset).toHaveBeenCalledTimes(1);
  expect(actions).not.toHaveProperty("apply");
});
