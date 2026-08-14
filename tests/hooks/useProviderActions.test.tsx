import type { ReactNode } from "react";
import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { useProviderActions } from "@/hooks/useProviderActions";
import type { Provider, UsageScript } from "@/types";

const toastSuccessMock = vi.fn();
const toastErrorMock = vi.fn();
const toastInfoMock = vi.fn();
const toastWarningMock = vi.fn();

vi.mock("sonner", () => ({
  toast: {
    success: (...args: unknown[]) => toastSuccessMock(...args),
    error: (...args: unknown[]) => toastErrorMock(...args),
    info: (...args: unknown[]) => toastInfoMock(...args),
    warning: (...args: unknown[]) => toastWarningMock(...args),
  },
}));

const addProviderMutateAsync = vi.fn();
const updateProviderMutateAsync = vi.fn();
const deleteProviderMutateAsync = vi.fn();
const switchProviderMutateAsync = vi.fn();

const addProviderMutation = {
  mutateAsync: addProviderMutateAsync,
  isPending: false,
};
const updateProviderMutation = {
  mutateAsync: updateProviderMutateAsync,
  isPending: false,
};
const deleteProviderMutation = {
  mutateAsync: deleteProviderMutateAsync,
  isPending: false,
};
const switchProviderMutation = {
  mutateAsync: switchProviderMutateAsync,
  isPending: false,
};

const useAddProviderMutationMock = vi.fn(() => addProviderMutation);
const useUpdateProviderMutationMock = vi.fn(() => updateProviderMutation);
const useDeleteProviderMutationMock = vi.fn(() => deleteProviderMutation);
const useSwitchProviderMutationMock = vi.fn(() => switchProviderMutation);

vi.mock("@/lib/query", () => ({
  useAddProviderMutation: () => useAddProviderMutationMock(),
  useUpdateProviderMutation: () => useUpdateProviderMutationMock(),
  useDeleteProviderMutation: () => useDeleteProviderMutationMock(),
  useSwitchProviderMutation: () => useSwitchProviderMutationMock(),
}));

const providersApiUpdateMock = vi.fn();
const providersApiUpdateTrayMenuMock = vi.fn();
const piApiUpdateProviderUsageScriptMock = vi.fn();
const openclawApiGetModelCatalogMock = vi.fn();
const openclawApiGetDefaultModelMock = vi.fn();
const openclawApiSetDefaultModelMock = vi.fn();

vi.mock("@/lib/api", () => ({
  piApi: {
    updateProviderUsageScript: (...args: unknown[]) =>
      piApiUpdateProviderUsageScriptMock(...args),
  },
  providersApi: {
    update: (...args: unknown[]) => providersApiUpdateMock(...args),
    updateTrayMenu: (...args: unknown[]) =>
      providersApiUpdateTrayMenuMock(...args),
  },
  openclawApi: {
    getModelCatalog: (...args: unknown[]) =>
      openclawApiGetModelCatalogMock(...args),
    getDefaultModel: (...args: unknown[]) =>
      openclawApiGetDefaultModelMock(...args),
    setDefaultModel: (...args: unknown[]) =>
      openclawApiSetDefaultModelMock(...args),
  },
}));

interface WrapperProps {
  children: ReactNode;
}

function createWrapper() {
  const queryClient = new QueryClient();

  const wrapper = ({ children }: WrapperProps) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );

  return { wrapper, queryClient };
}

function createProvider(overrides: Partial<Provider> = {}): Provider {
  return {
    id: "provider-1",
    name: "Test Provider",
    settingsConfig: {},
    category: "official",
    ...overrides,
  };
}

beforeEach(() => {
  addProviderMutateAsync.mockReset();
  updateProviderMutateAsync.mockReset();
  deleteProviderMutateAsync.mockReset();
  switchProviderMutateAsync.mockReset();
  switchProviderMutateAsync.mockResolvedValue({
    status: "switched",
    warnings: [],
    routingNotice: null,
  });
  providersApiUpdateMock.mockReset();
  providersApiUpdateTrayMenuMock.mockReset();
  piApiUpdateProviderUsageScriptMock.mockReset();
  openclawApiGetModelCatalogMock.mockReset();
  openclawApiGetDefaultModelMock.mockReset();
  openclawApiSetDefaultModelMock.mockReset();
  toastSuccessMock.mockReset();
  toastErrorMock.mockReset();
  toastInfoMock.mockReset();
  toastWarningMock.mockReset();

  addProviderMutation.isPending = false;
  updateProviderMutation.isPending = false;
  deleteProviderMutation.isPending = false;
  switchProviderMutation.isPending = false;

  useAddProviderMutationMock.mockClear();
  useUpdateProviderMutationMock.mockClear();
  useDeleteProviderMutationMock.mockClear();
  useSwitchProviderMutationMock.mockClear();
});

describe("useProviderActions", () => {
  it("should trigger mutation when calling addProvider", async () => {
    addProviderMutateAsync.mockResolvedValueOnce(undefined);
    const { wrapper } = createWrapper();
    const providerInput = {
      name: "New Provider",
      settingsConfig: { token: "abc" },
    } as Omit<Provider, "id">;

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await act(async () => {
      await result.current.addProvider(providerInput);
    });

    expect(addProviderMutateAsync).toHaveBeenCalledTimes(1);
    expect(addProviderMutateAsync).toHaveBeenCalledWith(providerInput);
  });

  it("should update tray menu when calling updateProvider", async () => {
    updateProviderMutateAsync.mockResolvedValueOnce(undefined);
    providersApiUpdateTrayMenuMock.mockResolvedValueOnce(true);
    const { wrapper } = createWrapper();
    const provider = createProvider();

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await act(async () => {
      await result.current.updateProvider(provider);
    });

    expect(updateProviderMutateAsync).toHaveBeenCalledWith({
      provider,
      originalId: undefined,
    });
    expect(providersApiUpdateTrayMenuMock).toHaveBeenCalledTimes(1);
  });

  it("submits only the requested provider switch", async () => {
    switchProviderMutateAsync.mockResolvedValueOnce({
      status: "switched",
      warnings: [],
      routingNotice: null,
    });
    const { wrapper } = createWrapper();
    const provider = createProvider({ category: "custom" });

    const { result } = renderHook(() => useProviderActions("codex"), {
      wrapper,
    });

    await act(async () => {
      await result.current.switchProvider(provider);
    });

    expect(switchProviderMutateAsync).toHaveBeenCalledWith({
      providerId: provider.id,
    });
    expect(toastSuccessMock).toHaveBeenCalledWith(
      "切换成功，请重启客户端以生效",
      { closeButton: true },
    );
  });

  it("presents each provider switch warning exactly as returned by the backend", async () => {
    switchProviderMutateAsync.mockResolvedValueOnce({
      status: "switched",
      warnings: [
        "Failed to quit ChatGPT before switching",
        "Failed to backfill the previous provider config",
      ],
      routingNotice: null,
    });
    const { wrapper } = createWrapper();
    const provider = createProvider({ category: "custom" });

    const { result } = renderHook(() => useProviderActions("codex"), {
      wrapper,
    });

    await act(async () => {
      await result.current.switchProvider(provider, true);
    });

    expect(toastWarningMock.mock.calls).toEqual([
      ["Failed to quit ChatGPT before switching"],
      ["Failed to backfill the previous provider config"],
    ]);
    expect(toastSuccessMock).toHaveBeenCalledWith(
      "切换成功，请重启客户端以生效",
      { closeButton: true },
    );
  });

  it("presents the backend routing notice without deriving provider capabilities", async () => {
    switchProviderMutateAsync.mockResolvedValueOnce({
      status: "switched",
      warnings: [],
      routingNotice: "managedOAuth",
    });
    const { wrapper } = createWrapper();
    const provider = createProvider({
      category: "custom",
      meta: undefined,
      settingsConfig: {},
    });

    const { result } = renderHook(() => useProviderActions("codex"), {
      wrapper,
    });

    await act(async () => {
      await result.current.switchProvider(provider);
    });

    expect(toastWarningMock).toHaveBeenCalledWith(
      expect.stringContaining("托管 OAuth"),
    );
    expect(toastSuccessMock).not.toHaveBeenCalled();
  });

  it.each([
    ["githubCopilot", "GitHub Copilot"],
    ["managedOAuth", "托管 OAuth"],
    ["openAiChat", "OpenAI Chat"],
    ["openAiResponses", "OpenAI Responses"],
    ["anthropicMessages", "Anthropic Messages"],
    ["claudeDesktop", "Claude Desktop 本地路由模式"],
    ["fullUrl", "完整 URL"],
    ["routingRequired", "本地路由"],
  ] as const)(
    "maps backend routing notice %s to user-facing copy",
    async (routingNotice, expectedText) => {
      switchProviderMutateAsync.mockResolvedValueOnce({
        status: "switched",
        warnings: [],
        routingNotice,
      });
      const { wrapper } = createWrapper();
      const provider = createProvider({ category: "custom" });

      const { result } = renderHook(() => useProviderActions("claude"), {
        wrapper,
      });

      await act(async () => {
        await result.current.switchProvider(provider);
      });

      expect(toastWarningMock).toHaveBeenCalledWith(
        expect.stringContaining(expectedText),
      );
      expect(switchProviderMutateAsync).toHaveBeenCalledWith({
        providerId: provider.id,
      });
    },
  );

  it("does not present a routing warning when the backend returns none", async () => {
    const { wrapper } = createWrapper();
    const provider = createProvider({ category: "custom" });

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await act(async () => {
      await result.current.switchProvider(provider);
    });

    expect(toastWarningMock).not.toHaveBeenCalled();
    expect(switchProviderMutateAsync).toHaveBeenCalledWith({
      providerId: provider.id,
    });
  });

  it("propagates updateProvider errors", async () => {
    updateProviderMutateAsync.mockRejectedValueOnce(new Error("update failed"));
    const { wrapper } = createWrapper();
    const provider = createProvider();

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await expect(
      act(async () => {
        await result.current.updateProvider(provider);
      }),
    ).rejects.toThrow("update failed");
  });

  it("handles switch mutation errors", async () => {
    switchProviderMutateAsync.mockRejectedValueOnce(new Error("switch failed"));
    const { wrapper } = createWrapper();
    const provider = createProvider();

    const { result } = renderHook(() => useProviderActions("codex"), {
      wrapper,
    });

    await expect(
      result.current.switchProvider(provider),
    ).resolves.toBeUndefined();
  });

  it("should call delete mutation when calling deleteProvider", async () => {
    deleteProviderMutateAsync.mockResolvedValueOnce(undefined);
    const { wrapper } = createWrapper();

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await act(async () => {
      await result.current.deleteProvider("provider-2");
    });

    expect(deleteProviderMutateAsync).toHaveBeenCalledWith("provider-2");
  });

  it("should update provider and refresh cache when saveUsageScript succeeds", async () => {
    providersApiUpdateMock.mockResolvedValueOnce(true);
    const { wrapper, queryClient } = createWrapper();
    const invalidateSpy = vi.spyOn(queryClient, "invalidateQueries");

    const provider = createProvider({
      meta: {
        usage_script: {
          enabled: false,
          language: "javascript",
          code: "",
        },
      },
    });

    const script: UsageScript = {
      enabled: true,
      language: "javascript",
      code: "return { success: true };",
      timeout: 5,
    };

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await act(async () => {
      await result.current.saveUsageScript(provider, script);
    });

    expect(providersApiUpdateMock).toHaveBeenCalledWith(
      {
        ...provider,
        meta: {
          ...provider.meta,
          usage_script: script,
        },
      },
      "claude",
    );
    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: ["providers", "claude"],
    });
    expect(toastSuccessMock).toHaveBeenCalledTimes(1);
  });

  it("should show error toast when saveUsageScript fails with error message", async () => {
    providersApiUpdateMock.mockRejectedValueOnce(new Error("Save failed"));
    const { wrapper } = createWrapper();
    const provider = createProvider();
    const script: UsageScript = {
      enabled: true,
      language: "javascript",
      code: "return {}",
    };

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await act(async () => {
      await result.current.saveUsageScript(provider, script);
    });

    expect(toastErrorMock).toHaveBeenCalledTimes(1);
    expect(toastErrorMock.mock.calls[0]?.[0]).toBe("Save failed");
  });

  it("saves Pi usage metadata without updating the native provider config", async () => {
    piApiUpdateProviderUsageScriptMock.mockResolvedValueOnce(true);
    const { wrapper } = createWrapper();
    const provider = createProvider({
      settingsConfig: {
        name: "Pi provider",
        futureField: { preserve: true },
      },
    });
    const script: UsageScript = {
      enabled: true,
      language: "javascript",
      code: "return {}",
    };

    const { result } = renderHook(() => useProviderActions("pi"), {
      wrapper,
    });

    await act(async () => {
      await result.current.saveUsageScript(provider, script);
    });

    expect(piApiUpdateProviderUsageScriptMock).toHaveBeenCalledWith(
      provider.id,
      script,
    );
    expect(providersApiUpdateMock).not.toHaveBeenCalled();
  });

  it("should use default error message when saveUsageScript fails without error message", async () => {
    providersApiUpdateMock.mockRejectedValueOnce(new Error(""));
    const { wrapper } = createWrapper();
    const provider = createProvider();
    const script: UsageScript = {
      enabled: true,
      language: "javascript",
      code: "return {}",
    };

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await act(async () => {
      await result.current.saveUsageScript(provider, script);
    });

    expect(toastErrorMock).toHaveBeenCalledTimes(1);
    expect(toastErrorMock.mock.calls[0]?.[0]).toBe("用量查询配置保存失败");
  });

  it("propagates addProvider errors to caller", async () => {
    addProviderMutateAsync.mockRejectedValueOnce(new Error("add failed"));
    const { wrapper } = createWrapper();

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await expect(
      act(async () => {
        await result.current.addProvider({
          name: "temp",
          settingsConfig: {},
        } as Omit<Provider, "id">);
      }),
    ).rejects.toThrow("add failed");
  });

  it("propagates deleteProvider errors to caller", async () => {
    deleteProviderMutateAsync.mockRejectedValueOnce(new Error("delete failed"));
    const { wrapper } = createWrapper();

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await expect(
      act(async () => {
        await result.current.deleteProvider("provider-2");
      }),
    ).rejects.toThrow("delete failed");
  });

  it("handles switch mutation errors silently", async () => {
    switchProviderMutateAsync.mockRejectedValueOnce(new Error("switch failed"));
    const { wrapper } = createWrapper();
    const provider = createProvider();

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await result.current.switchProvider(provider);
  });

  it("should track pending state of all mutations in isLoading", () => {
    addProviderMutation.isPending = true;
    const { wrapper } = createWrapper();

    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    expect(result.current.isLoading).toBe(true);
  });

  it("sets the first OpenClaw model without inventing a fallback chain", async () => {
    openclawApiSetDefaultModelMock.mockResolvedValueOnce({
      backupPath: "/tmp/openclaw-backup.json5",
      warnings: [],
    });

    const { wrapper } = createWrapper();
    const provider = createProvider({
      settingsConfig: {
        models: [{ id: "gpt-4.1" }, { id: "gpt-4.1-mini" }],
      },
    });

    const { result } = renderHook(() => useProviderActions("openclaw"), {
      wrapper,
    });

    await act(async () => {
      await result.current.setAsDefaultModel(provider);
    });

    expect(openclawApiSetDefaultModelMock).toHaveBeenCalledWith({
      primary: "provider-1/gpt-4.1",
    });
    expect(toastSuccessMock).toHaveBeenCalledTimes(1);
    expect(toastSuccessMock.mock.calls[0]?.[1]).toEqual({ closeButton: true });
  });

  it("sets the explicitly selected OpenClaw model and preserves existing fallbacks", async () => {
    openclawApiGetDefaultModelMock.mockResolvedValueOnce({
      primary: "other/old-primary",
      fallbacks: ["provider-1/gpt-4.1-mini", "other/fallback"],
      customPolicy: "preserve-me",
    });
    openclawApiSetDefaultModelMock.mockResolvedValueOnce({
      warnings: [],
    });

    const { wrapper } = createWrapper();
    const provider = createProvider({
      settingsConfig: {
        models: [{ id: "gpt-4.1" }, { id: "gpt-4.1-mini" }],
      },
    });

    const { result } = renderHook(() => useProviderActions("openclaw"), {
      wrapper,
    });

    await act(async () => {
      await result.current.setAsDefaultModel(provider, "gpt-4.1-mini");
    });

    expect(openclawApiSetDefaultModelMock).toHaveBeenCalledWith({
      primary: "provider-1/gpt-4.1-mini",
      fallbacks: ["other/fallback"],
      customPolicy: "preserve-me",
    });
  });
});
it("clears loading flag when all mutations idle", () => {
  addProviderMutation.isPending = false;
  updateProviderMutation.isPending = false;
  deleteProviderMutation.isPending = false;
  switchProviderMutation.isPending = false;

  const { wrapper } = createWrapper();
  const { result } = renderHook(() => useProviderActions("claude"), {
    wrapper,
  });

  expect(result.current.isLoading).toBe(false);
});
