import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Provider } from "@/types";
import { useCodexAccountProviderLinks } from "@/components/providers/forms/hooks/useCodexAccountProviderLinks";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const official = (id: string, accountId = "account-A"): Provider => ({
  id,
  name: "Repeated configuration",
  settingsConfig: {},
  meta: {
    authBinding: {
      source: "managed_account",
      authProvider: "codex_oauth",
      accountId,
    },
  },
  presentation: {
    isOfficial: true,
    isCurrent: false,
    isInConfig: false,
    isDefaultModel: false,
    isManaged: true,
    isReadOnly: false,
    canDelete: true,
    canEdit: true,
    canTestConnectivity: false,
    canConfigureUsage: false,
    usesOfficialSubscriptionUsage: true,
    canSetAsDefault: false,
    routingBadge: null,
    routingReason: null,
    switchBlockedReason: null,
  },
});
function wrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}
beforeEach(() => vi.mocked(invoke).mockReset());
it("associates only explicitly bound official Codex providers, retaining duplicate names by ID", async () => {
  const rows: Provider[] = [
    official("first"),
    official("second"),
    official("other-account", "account-B"),
    { ...official("missing-presentation"), presentation: undefined },
    {
      ...official("non-official"),
      presentation: { ...official("x").presentation!, isOfficial: false },
    },
    { ...official("default-fallback"), meta: undefined },
    {
      ...official("missing-account"),
      meta: {
        authBinding: { source: "managed_account", authProvider: "codex_oauth" },
      },
    },
    {
      ...official("provider-login"),
      meta: { authBinding: { source: "provider_config" } },
    },
    {
      ...official("key"),
      meta: {
        authBinding: { source: "vendor_api_key", accountId: "account-A" },
      },
    },
    {
      ...official("other-auth"),
      meta: {
        authBinding: {
          source: "managed_account",
          authProvider: "github_copilot",
          accountId: "account-A",
        },
      },
    },
  ];
  vi.mocked(invoke).mockResolvedValue(
    Object.fromEntries(rows.map((row) => [row.id, row])),
  );
  const { result } = renderHook(
    () => useCodexAccountProviderLinks("account-A", true),
    { wrapper: wrapper() },
  );
  await waitFor(() => expect(result.current.state).toBe("ready"));
  expect(result.current.providers.map((row) => row.id)).toEqual([
    "first",
    "second",
  ]);
  expect(result.current.providers[0]).toEqual({
    id: "first",
    name: "Repeated configuration",
    presentation: { isCurrent: false },
  });
  expect(invoke).toHaveBeenCalledExactlyOnceWith("get_providers", {
    app: "codex",
  });
});
it("reuses one Codex query across selected accounts rather than fetching per row", async () => {
  vi.mocked(invoke).mockResolvedValue({
    one: official("one"),
    two: official("two", "account-B"),
  });
  const { result, rerender } = renderHook(
    ({ id }) => useCodexAccountProviderLinks(id, true),
    { initialProps: { id: "account-A" }, wrapper: wrapper() },
  );
  await waitFor(() =>
    expect(result.current.providers.map((row) => row.id)).toEqual(["one"]),
  );
  rerender({ id: "account-B" });
  expect(result.current.providers.map((row) => row.id)).toEqual(["two"]);
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("does not read or retry while disabled", async () => {
  const { result } = renderHook(
    () => useCodexAccountProviderLinks("account-A", false),
    { wrapper: wrapper() },
  );
  await act(async () => {
    await result.current.retry();
  });
  expect(result.current.providers).toEqual([]);
  expect(invoke).not.toHaveBeenCalled();
});
it("removes stale associations on read failure and retries only the same readonly query", async () => {
  vi.mocked(invoke)
    .mockResolvedValueOnce({ one: official("one") })
    .mockRejectedValueOnce(new Error("private error payload"))
    .mockResolvedValueOnce({});
  const { result } = renderHook(
    () => useCodexAccountProviderLinks("account-A", true),
    { wrapper: wrapper() },
  );
  await waitFor(() => expect(result.current.providers).toHaveLength(1));
  await act(async () => {
    await result.current.retry();
  });
  await waitFor(() => expect(result.current.state).toBe("error"));
  expect(result.current.providers).toEqual([]);
  await act(async () => {
    await result.current.retry();
  });
  await waitFor(() => expect(result.current.state).toBe("ready"));
  expect(result.current.providers).toEqual([]);
  expect(vi.mocked(invoke).mock.calls).toEqual(
    Array.from({ length: 3 }, () => ["get_providers", { app: "codex" }]),
  );
});
