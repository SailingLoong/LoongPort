import type { ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useCodexOauthQuotaByAccountId } from "@/lib/query/subscription";
import type { SubscriptionQuota } from "@/types/subscription";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const good: SubscriptionQuota = {
  tool: "codex_oauth",
  credentialStatus: "valid",
  credentialMessage: null,
  success: true,
  tiers: [{ name: "five_hour", utilization: 25, resetsAt: null }],
  extraUsage: null,
  error: null,
  queriedAt: 1,
};

function setup(
  client = new QueryClient({
    defaultOptions: { queries: { retryDelay: 0, gcTime: 0 } },
  }),
) {
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return {
    client,
    ...renderHook(() => useCodexOauthQuotaByAccountId("synthetic-account"), {
      wrapper,
    }),
  };
}

describe("Codex OAuth quota typed failure contract", () => {
  afterEach(() => vi.restoreAllMocks());
  it.each(["network", "403", "429", "503", "IO", "mode.verification_required"])(
    "retains the previous quota when token retrieval rejects: %s",
    async (reason) => {
      invoke.mockReset().mockResolvedValue(good);
      const { result, unmount, client } = setup();
      await waitFor(() => expect(result.current.data?.success).toBe(true));
      invoke.mockRejectedValue(`Codex OAuth token unavailable: ${reason}`);
      await act(async () => {
        await result.current.refetch();
      });
      await waitFor(() => expect(result.current.isError).toBe(true));
      expect(result.current.data).toEqual(good);
      expect(invoke).toHaveBeenCalledWith("get_codex_oauth_quota", {
        accountId: "synthetic-account",
      });
      unmount();
      client.clear();
    },
  );

  it("does not hide a definitive refresh-token invalidation behind the last good quota", async () => {
    invoke.mockReset().mockResolvedValue(good);
    const { result, unmount, client } = setup();
    await waitFor(() => expect(result.current.data?.success).toBe(true));
    invoke.mockResolvedValue({
      ...good,
      success: false,
      credentialStatus: "expired",
      tiers: [],
      error: "Refresh Token 失效或已过期",
    });
    await act(async () => {
      await result.current.refetch();
    });
    await waitFor(() =>
      expect(result.current.data?.credentialStatus).toBe("expired"),
    );
    expect(result.current.data?.success).toBe(false);
    unmount();
    client.clear();
  });

  it("replaces old success with NotFound and does not revive it on a later rejection", async () => {
    invoke.mockReset().mockResolvedValue(good);
    const { result, unmount, client } = setup();
    await waitFor(() => expect(result.current.data?.success).toBe(true));
    const notFound: SubscriptionQuota = {
      ...good,
      credentialStatus: "not_found",
      success: false,
      tiers: [],
      queriedAt: null,
    };
    invoke.mockResolvedValue(notFound);
    await act(async () => {
      await result.current.refetch();
    });
    await waitFor(() => expect(result.current.data).toEqual(notFound));
    invoke.mockRejectedValue("Codex OAuth token unavailable: network");
    await act(async () => {
      await result.current.refetch();
    });
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.data).toEqual(notFound);
    unmount();
    client.clear();
  });

  it("expires rejected cached success at ten minutes on the next render, not on a new timer", async () => {
    let now = 1_800_000_000_000;
    vi.spyOn(Date, "now").mockImplementation(() => now);
    invoke.mockReset().mockResolvedValue(good);
    const { result, rerender, unmount, client } = setup();
    await waitFor(() => expect(result.current.data?.success).toBe(true));
    const successfulAt = result.current.dataUpdatedAt;
    invoke.mockRejectedValue("Codex OAuth token unavailable: network");
    await act(async () => {
      await result.current.refetch();
    });
    await waitFor(() => expect(result.current.isError).toBe(true));
    now = successfulAt + 10 * 60 * 1000 - 1;
    rerender();
    expect(result.current.data).toEqual(good);
    now += 1;
    rerender();
    expect(result.current.data).toMatchObject({
      success: false,
      credentialStatus: "valid",
      tiers: [],
    });
    expect(result.current.data?.error).toContain("network");
    expect(result.current.dataUpdatedAt).toBe(successfulAt);
    unmount();
    client.clear();
  });

  it.each([
    [10 * 60 * 1000 - 1, true],
    [10 * 60 * 1000, false],
    [10 * 60 * 1000 + 1, false],
  ])(
    "anchors a remount's rejected cached success to the original timestamp at age %i",
    async (age, visible) => {
      let now = 1_800_000_000_000;
      vi.spyOn(Date, "now").mockImplementation(() => now);
      invoke.mockReset().mockResolvedValue(good);
      const client = new QueryClient({
        defaultOptions: { queries: { retryDelay: 0, gcTime: Infinity } },
      });
      const first = setup(client);
      await waitFor(() =>
        expect(first.result.current.data?.success).toBe(true),
      );
      const successfulAt = first.result.current.dataUpdatedAt;
      invoke.mockRejectedValue("Codex OAuth token unavailable: network");
      await act(async () => {
        await first.result.current.refetch();
      });
      await waitFor(() => expect(first.result.current.isError).toBe(true));
      first.unmount();
      now = successfulAt + age;
      const second = setup(client);
      await waitFor(() => expect(second.result.current.isError).toBe(true));
      expect(second.result.current.data?.success).toBe(visible);
      expect(second.result.current.dataUpdatedAt).toBe(successfulAt);
      expect(second.result.current.data?.tiers).toEqual(
        visible ? good.tiers : [],
      );
      if (!visible)
        expect(second.result.current.data?.error).toContain("network");
      second.unmount();
      client.clear();
    },
  );
});
