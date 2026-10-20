import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  applicationOverviewApi,
  type ApplicationConfiguration,
} from "@/lib/api/applicationOverview";
import { relayApi } from "@/lib/api/relay";
import { useApplicationOverview } from "../useApplicationOverview";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
let client: QueryClient;
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={client}>{children}</QueryClientProvider>
);
const target = {
  providerId: "b",
  name: "B",
  canSelect: true,
  selection: { kind: "relay" },
} as ApplicationConfiguration;
const decision = { choice: "manage" as const, revision: "catalog-v1" };
const catalog = {
  ownership: "unclaimedExternal" as const,
  choice: "manage" as const,
  canManage: true,
  revision: "catalog-v1",
  beforePointer: "/synthetic/external",
  afterPointer: "/synthetic/managed",
};
const confirmation = {
  status: "confirmationRequired" as const,
  targetName: "B",
  catalog,
  quitRequired: false,
};
beforeEach(() => {
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.spyOn(applicationOverviewApi, "get").mockResolvedValue({
    configurations: [target],
    recentProviderIds: [],
    isAdditive: false,
  });
  vi.spyOn(relayApi, "switchTierModel").mockResolvedValue(confirmation);
  vi.mocked(invoke).mockReset();
});
afterEach(() => {
  client.clear();
  vi.restoreAllMocks();
});
// This checks the frontend transport union. The backend rejects legacy
// catalog mutation; its acceptance is deliberately not modeled by this spy.
it.each(["legacy", "native"] as const)(
  "bridges the real consumer to synthetic %s IPC envelopes without preparing a preview",
  async (protocol) => {
    const review = {
      draftDigest: "reviewed-digest",
      revision: "reviewed-revision",
    };
    const native = { ...review, id: "native-id", scope: "applicationRouting" };
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "prepare_application_routing_request")
        return protocol === "legacy"
          ? { protocol, review }
          : { protocol, request: native };
      if (command === "apply_application_routing")
        return (args as any).change.catalogPreviewOnly
          ? confirmation
          : {
              status: "switched",
              providerName: "B",
              warnings: [],
              chatgptWasRunning: false,
              chatgptRelaunched: false,
            };
      throw new Error(`Unexpected command ${command}`);
    });
    const { result } = renderHook(
      () => useApplicationOverview("codex", {}, vi.fn()),
      { wrapper },
    );
    await act(async () => {
      await result.current.select(target, undefined, "model-b");
    });
    expect(result.current.catalog).toEqual(catalog);
    await act(async () => {
      await result.current.previewCatalog(decision);
    });
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
      "apply_application_routing",
    ]);
    expect(result.current.confirmation).toBe("B");
    await act(async () => {
      await result.current.confirm(false, decision);
    });
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
      "apply_application_routing",
      "prepare_application_routing_request",
      "apply_application_routing",
    ]);
    expect(invoke).toHaveBeenLastCalledWith("apply_application_routing", {
      appType: "codex",
      change: {
        selection: { providerId: "b", model: "model-b" },
        catalogDecision: decision,
        catalogPreviewOnly: false,
      },
      quitChatgpt: false,
      ...(protocol === "legacy"
        ? { legacyReview: review }
        : { request: native }),
    });
    expect(result.current.confirmation).toBeNull();
  },
  // Three act-wrapped round trips through the real hook and query client;
  // contended CI runners have exceeded the 15s default without stalling.
  30_000,
);

it("keeps a backend-rejected legacy catalog confirmation failed without replay or model change", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "prepare_application_routing_request")
      throw new Error("mode.verification_required");
    throw new Error(`Unexpected write or query ${command}`);
  });
  const { result } = renderHook(
    () => useApplicationOverview("codex", {}, vi.fn()),
    { wrapper },
  );
  await act(async () => {
    await result.current.select(target, undefined, "model-b");
  });
  expect(result.current.confirmation).toBe("B");
  await act(async () => {
    await result.current.confirm(false, decision);
  });
  expect(result.current.confirmation).toBeNull();
  expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
    "prepare_application_routing_request",
  ]);
  expect(relayApi.switchTierModel).toHaveBeenCalledTimes(1);
  expect(result.current.data?.configurations).toEqual([target]);
});
