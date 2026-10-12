import type { ReactNode } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useUpdateProviderMutation } from "@/lib/query/mutations";
import { usageKeys } from "@/lib/query/usage";
import { toast } from "sonner";
import type { Provider } from "@/types";

const apiMocks = vi.hoisted(() => ({
  update: vi.fn(),
  confirmEdit: vi.fn(),
  queryEdit: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  providersApi: {
    update: (...args: unknown[]) => apiMocks.update(...args),
    confirmEdit: (...args: unknown[]) => apiMocks.confirmEdit(...args),
    queryEdit: (...args: unknown[]) => apiMocks.queryEdit(...args),
  },
  sessionsApi: {},
  settingsApi: {},
}));

vi.mock("@/hooks/useHermes", () => ({
  invalidateHermesProviderCaches: vi.fn(),
}));

vi.mock("@/hooks/useOpenClaw", () => ({
  openclawKeys: {
    health: ["openclaw", "health"],
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? _key,
  }),
}));

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const invalidateSpy = vi.spyOn(queryClient, "invalidateQueries");

  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );

  return { wrapper, invalidateSpy };
}

function createProvider(overrides: Partial<Provider> = {}): Provider {
  return {
    id: "provider-1",
    name: "Test Provider",
    settingsConfig: {},
    ...overrides,
  };
}

beforeEach(() => {
  apiMocks.update.mockReset().mockResolvedValue(true);
  apiMocks.confirmEdit.mockReset();
  apiMocks.queryEdit.mockReset();
  vi.mocked(toast.success).mockClear();
  vi.mocked(toast.error).mockClear();
});

describe("useUpdateProviderMutation", () => {
  it("invalidates the updated provider usage query", async () => {
    const { wrapper, invalidateSpy } = createWrapper();
    const provider = createProvider({ id: "provider-b" });
    const { result } = renderHook(() => useUpdateProviderMutation("codex"), {
      wrapper,
    });

    await act(async () => {
      await result.current.mutateAsync({ provider });
    });

    expect(apiMocks.update).toHaveBeenCalledWith(provider, "codex", undefined);
    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: ["providers", "codex"],
    });
    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: usageKeys.script("provider-b", "codex"),
    });
    expect(invalidateSpy).not.toHaveBeenCalledWith({
      queryKey: usageKeys.all,
    });
  });

  it("also invalidates the previous usage query when provider id changes", async () => {
    const { wrapper, invalidateSpy } = createWrapper();
    const provider = createProvider({ id: "provider-new" });
    const { result } = renderHook(() => useUpdateProviderMutation("openclaw"), {
      wrapper,
    });

    await act(async () => {
      await result.current.mutateAsync({
        provider,
        originalId: "provider-old",
      });
    });

    expect(apiMocks.update).toHaveBeenCalledWith(
      provider,
      "openclaw",
      "provider-old",
    );
    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: usageKeys.script("provider-new", "openclaw"),
    });
    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: usageKeys.script("provider-old", "openclaw"),
    });
    expect(invalidateSpy).not.toHaveBeenCalledWith({
      queryKey: usageKeys.all,
    });
  });

  it("refreshes Pi provider caches even when an update fails", async () => {
    apiMocks.update.mockRejectedValueOnce(new Error("conflict"));
    const { wrapper, invalidateSpy } = createWrapper();
    const provider = createProvider({ id: "pi-provider" });
    const { result } = renderHook(() => useUpdateProviderMutation("pi"), {
      wrapper,
    });

    await act(async () => {
      await expect(result.current.mutateAsync({ provider })).rejects.toThrow(
        "conflict",
      );
    });

    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: ["pi", "currentState"],
    });
    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: ["providers", "pi"],
    });
  });
});

const editRequest = {
  id: "44444444-4444-4444-8444-444444444444",
  providerId: "provider-1",
  draftDigest: "a".repeat(64),
  revision: "b".repeat(64),
};
describe("U02 original update mutation result", () => {
  it("returns the original pending result without success effects or a legacy write", async () => {
    const answer = { app: "claude", request: editRequest, status: "partial" };
    apiMocks.confirmEdit.mockResolvedValue(answer);
    const { wrapper, invalidateSpy } = createWrapper();
    const { result } = renderHook(() => useUpdateProviderMutation("claude"), {
      wrapper,
    });
    let received: unknown;
    await act(async () => {
      received = await result.current.mutateAsync({
        provider: createProvider(),
        edit: { request: editRequest, deleteCredential: false },
      });
    });
    expect(received).toEqual(answer);
    expect(apiMocks.update).not.toHaveBeenCalled();
    expect(invalidateSpy).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("keeps a completed save completed if cache refresh fails", async () => {
    const answer = { app: "claude", request: editRequest, status: "completed" };
    apiMocks.confirmEdit.mockResolvedValue(answer);
    const { wrapper, invalidateSpy } = createWrapper();
    invalidateSpy.mockRejectedValue(new Error("synthetic cache failure"));
    const { result } = renderHook(() => useUpdateProviderMutation("claude"), {
      wrapper,
    });
    let received: unknown;
    await act(async () => {
      received = await result.current.mutateAsync({
        provider: createProvider(),
        edit: { request: editRequest, deleteCredential: false },
      });
    });
    expect(received).toEqual(answer);
    expect(toast.success).not.toHaveBeenCalled(); // The live editor owns its success notice.
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("keeps a lost response bound to its original request without displaying raw errors", async () => {
    apiMocks.confirmEdit.mockRejectedValue(
      new Error("synthetic-private-error-fragment"),
    );
    const { wrapper, invalidateSpy } = createWrapper();
    const { result } = renderHook(() => useUpdateProviderMutation("claude"), {
      wrapper,
    });
    let received: unknown;
    await act(async () => {
      received = await result.current.mutateAsync({
        provider: createProvider(),
        edit: { request: editRequest, deleteCredential: false },
      });
    });
    expect(received).toEqual({
      app: "claude",
      request: editRequest,
      status: "unknown",
    });
    expect(apiMocks.confirmEdit).toHaveBeenCalledTimes(1);
    expect(apiMocks.update).not.toHaveBeenCalled();
    expect(invalidateSpy).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
  });
});

it("queries a lost original result through the same cache owner without another write", async () => {
  const answer = { app: "claude", request: editRequest, status: "completed" };
  apiMocks.queryEdit.mockResolvedValue(answer);
  const { wrapper, invalidateSpy } = createWrapper();
  const { result } = renderHook(() => useUpdateProviderMutation("claude"), {
    wrapper,
  });
  let received: unknown;
  await act(async () => {
    received = await result.current.mutateAsync({
      provider: createProvider(),
      edit: { request: editRequest, deleteCredential: false, queryOnly: true },
    });
  });
  expect(received).toEqual(answer);
  expect(apiMocks.queryEdit).toHaveBeenCalledWith("claude", editRequest);
  expect(apiMocks.confirmEdit).not.toHaveBeenCalled();
  expect(apiMocks.update).not.toHaveBeenCalled();
  expect(invalidateSpy).toHaveBeenCalledWith({
    queryKey: ["providers", "claude"],
  });
});

it.each(["app", "id", "providerId", "draftDigest", "revision"] as const)(
  "does not accept a completed receipt with mismatching %s",
  async (field) => {
    const answer = {
      app: "claude",
      request: { ...editRequest },
      status: "completed",
    };
    if (field === "app") answer.app = "codex";
    else answer.request[field] = "different";
    apiMocks.confirmEdit.mockResolvedValue(answer);
    const { wrapper, invalidateSpy } = createWrapper();
    const { result } = renderHook(() => useUpdateProviderMutation("claude"), {
      wrapper,
    });
    let received: unknown;
    await act(async () => {
      received = await result.current.mutateAsync({
        provider: createProvider(),
        edit: { request: editRequest, deleteCredential: false },
      });
    });
    expect(received).toEqual({
      app: "claude",
      request: editRequest,
      status: "unknown",
    });
    expect(invalidateSpy).not.toHaveBeenCalled();
    expect(apiMocks.update).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
  },
);

it("refreshes the original app cache if the active app changes before confirmation returns", async () => {
  let finish!: (value: unknown) => void;
  apiMocks.confirmEdit.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const { wrapper, invalidateSpy } = createWrapper();
  const { result, rerender } = renderHook(
    ({ app }: { app: "claude" | "codex" }) => useUpdateProviderMutation(app),
    { wrapper, initialProps: { app: "claude" } },
  );
  let pending!: Promise<unknown>;
  await act(async () => {
    pending = result.current.mutateAsync({
      provider: createProvider(),
      edit: { request: editRequest, deleteCredential: false },
    });
  });
  rerender({ app: "codex" });
  await act(async () => {
    finish({ app: "claude", request: editRequest, status: "completed" });
    await pending;
  });
  expect(invalidateSpy).toHaveBeenCalledWith({
    queryKey: ["providers", "claude"],
  });
  expect(invalidateSpy).not.toHaveBeenCalledWith({
    queryKey: ["providers", "codex"],
  });
});
