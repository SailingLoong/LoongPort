import workspaceSource from "../ApplicationWorkspace.tsx?raw";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
import { useApplicationRouting } from "../useApplicationRouting";

vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
afterEach(() => vi.restoreAllMocks());

it("keeps the remaining non-proxy workspace out of the retired route-draft consumer graph", () => {
  for (const consumer of [
    "ApplicationInlineSelection",
    "OrderProfilesMenu",
    "useApplicationRoutingDraft",
    "ApplicationChangeConfirmDialog",
  ]) {
    expect(workspaceSource).not.toContain(`from "./${consumer}"`);
  }
});

it("retains immediate non-proxy actions without exposing another native route-request writer", async () => {
  vi.spyOn(applicationRoutingApi, "get").mockResolvedValue({
    autoFailoverEnabled: false,
    routingActive: false,
    model: null,
    modelOptions: [],
    chainIds: [],
    tiers: [],
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const { result, unmount } = renderHook(
    () => useApplicationRouting("opencode"),
    {
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  try {
    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(result.current.writeBlocked).toBe(false);
    for (const action of [
      "setOrder",
      "blockTier",
      "resetTierErrors",
    ] as const) {
      expect(result.current[action]).toBeTypeOf("function");
    }
    for (const retired of [
      "prepareRequest",
      "apply",
      "originalResult",
      "recheckRequest",
      "startNewRequest",
      "setFailover",
    ]) {
      expect(result.current).not.toHaveProperty(retired);
    }
  } finally {
    unmount();
    client.clear();
  }
});
