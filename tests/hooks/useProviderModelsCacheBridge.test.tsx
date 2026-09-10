import { QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import type { PropsWithChildren } from "react";
import { expect, it } from "vitest";
import { useProviderModelsCacheBridge } from "@/hooks/useProviderModelsCacheBridge";
import { emitTauriEvent } from "../msw/tauriMocks";
import { createTestQueryClient } from "../utils/testQueryClient";

it("refreshes affected inventory views after background repair without marking another app stale", () => {
  const client = createTestQueryClient();
  const affected = [
    ["providers", "codex"],
    ["applicationOverview", "codex"],
    ["easyModeTierBoard", "codex"],
    ["serviceAccounts"],
  ];
  for (const key of [...affected, ["providers", "claude"]])
    client.setQueryData(key, []);
  const wrapper = ({ children }: PropsWithChildren) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  renderHook(() => useProviderModelsCacheBridge(), { wrapper });
  act(() => {
    emitTauriEvent("provider-models-updated", { appType: "codex" });
  });
  for (const key of affected)
    expect(client.getQueryState(key)?.isInvalidated).toBe(true);
  expect(client.getQueryState(["providers", "claude"])?.isInvalidated).toBe(
    false,
  );
});
