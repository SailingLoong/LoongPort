import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";
import { expect, it, vi } from "vitest";
import { providersApi } from "@/lib/api/providers";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
import { useSwitchProviderMutation } from "@/lib/query/mutations";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

it("carries catalog approval and preview-only intent through provider mutation and invoke", async () => {
  invoke.mockResolvedValue({
    status: "confirmationRequired",
    targetName: "Example",
  });
  const client = new QueryClient();
  const { result } = renderHook(() => useSwitchProviderMutation("codex"), {
    wrapper: ({ children }: { children: ReactNode }) =>
      createElement(QueryClientProvider, { client }, children),
  });
  const catalogDecision = { choice: "manage" as const, revision: "catalog-v1" };
  await act(async () => {
    await result.current.mutateAsync({
      providerId: "p",
      catalogDecision,
      catalogPreviewOnly: true,
    });
  });
  expect(invoke).toHaveBeenLastCalledWith("switch_provider", {
    id: "p",
    app: "codex",
    quitChatgpt: null,
    catalogDecision,
    catalogPreviewOnly: true,
  });
  await providersApi.switch("p", "codex", false, catalogDecision, false);
  expect(invoke).toHaveBeenLastCalledWith("switch_provider", {
    id: "p",
    app: "codex",
    quitChatgpt: false,
    catalogDecision,
    catalogPreviewOnly: false,
  });
});

it("carries catalog approval in the existing routing change without losing order or model", async () => {
  const change = {
    selection: { providerId: "p", model: "model-b" },
    order: { profileName: "Work", providerIds: ["p"] },
    catalogDecision: { choice: "manage" as const, revision: "catalog-v1" },
    catalogPreviewOnly: true,
  };
  await applicationRoutingApi.apply("codex", change);
  expect(invoke).toHaveBeenLastCalledWith("apply_application_routing", {
    appType: "codex",
    change,
    quitChatgpt: undefined,
  });
});
