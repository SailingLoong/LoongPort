import { QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import type { PropsWithChildren } from "react";
import { describe, expect, it } from "vitest";

import { RELAY_DIRECTORY_UPDATED_EVENT } from "@/config/constants";
import { useRelayDirectoryCacheBridge } from "@/hooks/useRelayDirectoryCacheBridge";
import { relayDirectoryKeys } from "@/lib/query/relayDirectory";
import { emitTauriEvent } from "../msw/tauriMocks";
import { createTestQueryClient } from "../utils/testQueryClient";

describe("useRelayDirectoryCacheBridge", () => {
  it("invalidates an existing leaderboard while the directory page is closed", async () => {
    const client = createTestQueryClient();
    const key = relayDirectoryKeys.byKind("claude");
    client.setQueryData(key, {
      kind: "claude",
      items: [],
      syncedAt: 1,
    });
    const wrapper = ({ children }: PropsWithChildren) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    renderHook(() => useRelayDirectoryCacheBridge(), { wrapper });

    act(() => {
      emitTauriEvent(RELAY_DIRECTORY_UPDATED_EVENT, { kind: "claude" });
    });

    expect(client.getQueryState(key)?.isInvalidated).toBe(true);
  });
});
