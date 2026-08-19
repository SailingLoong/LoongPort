import type { ReactNode } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAddProviderMutation } from "@/lib/query/mutations";
import type { Provider } from "@/types";

const apiMocks = vi.hoisted(() => ({
  add: vi.fn(),
  ensureClaudeDesktopOfficialProvider: vi.fn(),
  getAll: vi.fn(),
  updateTrayMenu: vi.fn(),
}));

const uuidMocks = vi.hoisted(() => ({
  generateUUID: vi.fn(),
}));

const toastMocks = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  providersApi: {
    add: (...args: unknown[]) => apiMocks.add(...args),
    ensureClaudeDesktopOfficialProvider: (...args: unknown[]) =>
      apiMocks.ensureClaudeDesktopOfficialProvider(...args),
    getAll: (...args: unknown[]) => apiMocks.getAll(...args),
    updateTrayMenu: (...args: unknown[]) => apiMocks.updateTrayMenu(...args),
  },
  sessionsApi: {},
  settingsApi: {},
}));

vi.mock("sonner", () => ({
  toast: toastMocks,
}));

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });

  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );

  return { wrapper };
}

beforeEach(() => {
  apiMocks.add.mockReset().mockResolvedValue(true);
  apiMocks.ensureClaudeDesktopOfficialProvider
    .mockReset()
    .mockResolvedValue(true);
  apiMocks.getAll.mockReset().mockResolvedValue({});
  apiMocks.updateTrayMenu.mockReset().mockResolvedValue(true);
  uuidMocks.generateUUID.mockReset().mockReturnValue("generated-uuid");
  toastMocks.success.mockReset();
  toastMocks.error.mockReset();
  toastMocks.warning.mockReset();
});

describe("useAddProviderMutation", () => {
  it("asks the backend to generate the duplicated Claude Desktop provider id", async () => {
    const { wrapper } = createWrapper();
    const { result } = renderHook(
      () => useAddProviderMutation("claude-desktop"),
      { wrapper },
    );

    const duplicatedProvider = await act(async () =>
      result.current.mutateAsync({
        name: "Claude Desktop Official copy",
        settingsConfig: { env: {} },
        category: "official",
      }),
    );

    expect(apiMocks.ensureClaudeDesktopOfficialProvider).not.toHaveBeenCalled();
    expect(apiMocks.add).toHaveBeenCalledTimes(1);
    expect(apiMocks.add).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "",
        name: "Claude Desktop Official copy",
        category: "official",
      }),
      "claude-desktop",
      undefined,
      undefined,
    );
    expect(duplicatedProvider.id).toBe("");
    expect(duplicatedProvider.id).not.toBe("claude-desktop-official");
  });

  it("returns the persisted seed row for the Claude Desktop official preset", async () => {
    const seedProvider: Provider = {
      id: "claude-desktop-official",
      name: "Claude Desktop Official",
      settingsConfig: { env: {} },
      websiteUrl: "https://claude.ai/download",
      category: "official",
      icon: "anthropic",
      iconColor: "#D4915D",
      createdAt: 123,
    };
    apiMocks.getAll.mockResolvedValueOnce({
      "claude-desktop-official": seedProvider,
    });
    const { wrapper } = createWrapper();
    const { result } = renderHook(
      () => useAddProviderMutation("claude-desktop"),
      { wrapper },
    );

    const persistedProvider = await act(async () =>
      result.current.mutateAsync({
        name: "Renamed by form",
        settingsConfig: { env: { ignored: true } },
        websiteUrl: "https://example.invalid",
        category: "official",
        icon: "custom-icon",
        ensureClaudeDesktopOfficialSeed: true,
      }),
    );

    expect(apiMocks.ensureClaudeDesktopOfficialProvider).toHaveBeenCalledTimes(
      1,
    );
    expect(apiMocks.getAll).toHaveBeenCalledWith("claude-desktop");
    expect(apiMocks.add).not.toHaveBeenCalled();
    expect(persistedProvider).toEqual(seedProvider);
  });

  it("adds a managed Codex account as a separate official card", async () => {
    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useAddProviderMutation("codex"), {
      wrapper,
    });

    const persistedProvider = await act(async () =>
      result.current.mutateAsync({
        name: "OpenAI Official",
        settingsConfig: { auth: {}, config: "" },
        category: "official",
        meta: {
          providerType: "codex_oauth",
          authBinding: {
            source: "managed_account",
            authProvider: "codex_oauth",
            accountId: "acct-managed",
          },
        },
      }),
    );

    expect(apiMocks.getAll).not.toHaveBeenCalled();
    // LoongPort 契约：provider id 由后端 `prepare_provider_for_add` 生成
    // （托管 id 前缀守卫也在这层），前端透传空 id。
    expect(apiMocks.add).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "",
        category: "official",
        meta: {
          providerType: "codex_oauth",
          authBinding: {
            source: "managed_account",
            authProvider: "codex_oauth",
            accountId: "acct-managed",
          },
        },
      }),
      "codex",
      undefined,
      undefined,
    );
    expect(persistedProvider).toEqual(
      expect.objectContaining({
        id: "",
        meta: expect.objectContaining({
          authBinding: expect.objectContaining({
            accountId: "acct-managed",
          }),
        }),
      }),
    );
  });

  it("adds every unbound Codex Official as an independent provider", async () => {
    uuidMocks.generateUUID
      .mockReset()
      .mockReturnValueOnce("unbound-official-1")
      .mockReturnValueOnce("unbound-official-2");
    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useAddProviderMutation("codex"), {
      wrapper,
    });

    const firstProvider = await act(async () =>
      result.current.mutateAsync({
        name: "OpenAI Official 1",
        settingsConfig: { auth: {}, config: "" },
        category: "official",
        meta: { providerType: "codex_oauth" },
      }),
    );
    const secondProvider = await act(async () =>
      result.current.mutateAsync({
        name: "OpenAI Official 2",
        settingsConfig: { auth: {}, config: "" },
        category: "official",
        meta: { providerType: "codex_oauth" },
      }),
    );

    expect(apiMocks.getAll).not.toHaveBeenCalled();
    // 同上：id 由后端生成，前端只负责把每个未绑定官方账号各提一次 add。
    expect(apiMocks.add).toHaveBeenNthCalledWith(
      1,
      expect.objectContaining({
        id: "",
        meta: { providerType: "codex_oauth" },
      }),
      "codex",
      undefined,
      undefined,
    );
    expect(apiMocks.add).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({
        id: "",
        meta: { providerType: "codex_oauth" },
      }),
      "codex",
      undefined,
      undefined,
    );
    expect(firstProvider.id).toBe("");
    expect(secondProvider.id).toBe("");
  });

  it("adds a Pi provider without a separate default-model command", async () => {
    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useAddProviderMutation("pi"), {
      wrapper,
    });

    const provider = await act(async () =>
      result.current.mutateAsync({
        name: "Pi Provider",
        providerKey: "pi-provider",
        settingsConfig: {
          api: "openai-responses",
          baseUrl: "https://example.com/v1",
          apiKey: "secret",
          models: [{ id: "model-a" }],
        },
      }),
    );

    // 本仓契约：ID 由后端生成 —— 前端传空 id，providerKey 作为第 4 参随行。
    expect(apiMocks.add).toHaveBeenCalledWith(
      expect.objectContaining({ id: "" }),
      "pi",
      undefined,
      "pi-provider",
    );
    expect(provider.id).toBe("");
  });

  it("reports a Pi provider add failure", async () => {
    apiMocks.add.mockRejectedValueOnce(new Error("provider add failed"));
    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useAddProviderMutation("pi"), {
      wrapper,
    });

    await act(async () => {
      await expect(
        result.current.mutateAsync({
          name: "Pi Provider",
          providerKey: "pi-provider",
          settingsConfig: { models: [{ id: "model-a" }] },
        }),
      ).rejects.toThrow("provider add failed");
    });

    // 本仓契约：ID 由后端生成 —— 前端传空 id，providerKey 作为第 4 参随行。
    expect(apiMocks.add).toHaveBeenCalledWith(
      expect.objectContaining({ id: "" }),
      "pi",
      undefined,
      "pi-provider",
    );
    expect(toastMocks.error).toHaveBeenCalled();
    expect(toastMocks.warning).not.toHaveBeenCalled();
  });
});
