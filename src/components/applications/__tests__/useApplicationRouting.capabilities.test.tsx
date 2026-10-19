import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import {
  applicationRoutingApi,
  type ApplicationRouting,
} from "@/lib/api/applicationRouting";
import { useApplicationRouting } from "../useApplicationRouting";
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));

const change = { selection: { providerId: "b", model: "model-b" } };
const review = { draftDigest: "a".repeat(64), revision: "b".repeat(64) };
const switched = {
  status: "switched" as const,
  providerName: "B",
  warnings: [],
  chatgptWasRunning: false,
  chatgptRelaunched: false,
};
const facts = (
  protocol: "native" | "legacy" | null = "legacy",
): ApplicationRouting => ({
  modeState: {
    status: protocol ? "ready" : "unknown",
    mode: protocol === "native" ? "proxy" : null,
    attached: protocol === "native" ? true : null,
    currentProviderId: "a",
    directProviderId: "a",
    publicationStarted: false,
    canWrite: Boolean(protocol),
    canRecheck: true,
    writeProtocol: protocol,
    blockedReason: protocol ? null : "upgrade.checkpoint_pending",
  },
  autoFailoverEnabled: false,
  routingActive: false,
  model: "model-a",
  modelOptions: [],
  chainIds: ["a", "b"],
  tiers: [],
});
let client: QueryClient;
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={client}>{children}</QueryClientProvider>
);
beforeEach(() => {
  client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  vi.spyOn(applicationRoutingApi, "get").mockResolvedValue(facts());
  vi.spyOn(applicationRoutingApi, "prepareRequest").mockResolvedValue({
    protocol: "legacy",
    review,
  });
  vi.spyOn(applicationRoutingApi, "apply").mockResolvedValue(switched);
  vi.spyOn(applicationRoutingApi, "queryRequest");
  vi.spyOn(applicationRoutingApi, "setFailover").mockResolvedValue(undefined);
});
afterEach(() => {
  client.clear();
  vi.restoreAllMocks();
});

describe("managed routing capability and explicit legacy review", () => {
  it("uses the reviewed legacy command without fabricating a native receipt", async () => {
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.writeBlocked).toBe(false));
    await act(async () => {
      await result.current.prepareRequest(change);
    });
    await act(async () => {
      expect(await result.current.apply(change)).toEqual(switched);
    });
    expect(applicationRoutingApi.apply).toHaveBeenCalledWith(
      "codex",
      change,
      undefined,
      undefined,
      review,
    );
    expect(applicationRoutingApi.queryRequest).not.toHaveBeenCalled();
    expect(result.current.originalResult).toBeNull();
  });
  it("does not replay or query a failed legacy write and needs a fresh read before a new action", async () => {
    const get = vi.mocked(applicationRoutingApi.get);
    vi.mocked(applicationRoutingApi.apply).mockRejectedValueOnce(
      new Error("reply lost"),
    );
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.writeBlocked).toBe(false));
    get.mockRejectedValue(new Error("read offline"));
    await act(async () => {
      await expect(result.current.apply(change)).rejects.toThrow("reply lost");
    });
    await waitFor(() => expect(result.current.writeBlocked).toBe(true));
    expect(result.current.originalResult).toBeNull();
    expect(applicationRoutingApi.queryRequest).not.toHaveBeenCalled();
    expect(applicationRoutingApi.apply).toHaveBeenCalledTimes(1);
    get.mockResolvedValue(facts());
    await act(async () => {
      await result.current.refetch();
    });
    await waitFor(() => expect(result.current.writeBlocked).toBe(false));
    expect(applicationRoutingApi.apply).toHaveBeenCalledTimes(1);
    await act(async () => {
      await result.current.apply(change);
    });
    expect(applicationRoutingApi.prepareRequest).toHaveBeenCalledTimes(2);
    expect(applicationRoutingApi.apply).toHaveBeenCalledTimes(2);
    expect(applicationRoutingApi.queryRequest).not.toHaveBeenCalled();
  });
  it.each(["claude", "codex", "gemini", "grokbuild"] as const)(
    "fails closed without modeState for managed %s",
    async (app) => {
      const data = facts();
      delete data.modeState;
      vi.mocked(applicationRoutingApi.get).mockResolvedValue(data);
      const { result } = renderHook(() => useApplicationRouting(app), {
        wrapper,
      });
      await waitFor(() => expect(result.current.data).toBeDefined());
      expect(result.current.writeBlocked).toBe(true);
      await expect(result.current.setFailover(true)).rejects.toThrow(
        "mode.verification_required",
      );
      expect(applicationRoutingApi.setFailover).not.toHaveBeenCalled();
    },
  );
  it("does not require managed routing capabilities for an independent app", async () => {
    const data = facts();
    delete data.modeState;
    vi.mocked(applicationRoutingApi.get).mockResolvedValue(data);
    const { result } = renderHook(() => useApplicationRouting("opencode"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(result.current.writeBlocked).toBe(false);
  });
  it.each([undefined, null, "invalid"])(
    "blocks a missing or invalid write protocol: %s",
    async (protocol) => {
      const data = facts();
      Object.assign(data.modeState!, { writeProtocol: protocol });
      vi.mocked(applicationRoutingApi.get).mockResolvedValue(data);
      const { result } = renderHook(() => useApplicationRouting("codex"), {
        wrapper,
      });
      await waitFor(() => expect(result.current.data).toBeDefined());
      expect(result.current.writeBlocked).toBe(true);
      await expect(result.current.prepareRequest(change)).rejects.toThrow(
        "mode.verification_required",
      );
      expect(applicationRoutingApi.prepareRequest).not.toHaveBeenCalled();
    },
  );
  it("blocks while loading, through checkpoint state, and unlocks only after a successful recovery read", async () => {
    let resolve!: (value: ApplicationRouting) => void;
    vi.mocked(applicationRoutingApi.get).mockReturnValueOnce(
      new Promise((r) => {
        resolve = r;
      }),
    );
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    expect(result.current.writeBlocked).toBe(true);
    await act(async () => {
      resolve(facts(null));
    });
    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(result.current.writeBlocked).toBe(true);
    expect(result.current.modeState?.blockedReason).toBe(
      "upgrade.checkpoint_pending",
    );
    await expect(result.current.apply(change)).rejects.toThrow(
      "mode.verification_required",
    );
    await act(async () => {
      await result.current.refetch();
    });
    await waitFor(() => expect(result.current.writeBlocked).toBe(false));
    expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
  });
  it("rejects a prepare protocol mismatch before any write", async () => {
    vi.mocked(applicationRoutingApi.prepareRequest).mockResolvedValue({
      protocol: "native",
      request: { id: "native", scope: "applicationRouting", ...review },
    });
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.writeBlocked).toBe(false));
    await act(async () => {
      await expect(result.current.apply(change)).rejects.toThrow(
        "mode.verification_required",
      );
    });
    expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
    expect(applicationRoutingApi.queryRequest).not.toHaveBeenCalled();
  });
  it("does not submit a second legacy write while the first reply is pending", async () => {
    let resolve!: (value: typeof switched) => void;
    vi.mocked(applicationRoutingApi.apply).mockReturnValueOnce(
      new Promise((r) => {
        resolve = r;
      }),
    );
    const { result } = renderHook(() => useApplicationRouting("codex"), {
      wrapper,
    });
    await waitFor(() => expect(result.current.writeBlocked).toBe(false));
    let pending!: Promise<unknown>;
    await act(async () => {
      pending = result.current.apply(change);
    });
    await waitFor(() =>
      expect(applicationRoutingApi.apply).toHaveBeenCalledTimes(1),
    );
    await expect(result.current.apply(change)).rejects.toThrow(
      "mode.verification_required",
    );
    await act(async () => {
      resolve(switched);
      await pending;
    });
    expect(applicationRoutingApi.apply).toHaveBeenCalledTimes(1);
  });
});
