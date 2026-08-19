import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { useEffect } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AddProviderForm } from "@/components/providers/AddProviderForm";
import type { ProviderFormValues } from "@/components/providers/forms/ProviderForm";
import { codexProviderPresets } from "@/config/codexProviderPresets";

vi.mock("@/components/ui/dialog", () => ({
  Dialog: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogHeader: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogTitle: ({ children }: { children: React.ReactNode }) => (
    <h1>{children}</h1>
  ),
  DialogDescription: ({ children }: { children: React.ReactNode }) => (
    <p>{children}</p>
  ),
  DialogFooter: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}));

let mockFormValues: ProviderFormValues;
let mockFormReady = true;
let submitReadyCallbacks: Array<(isReady: boolean) => void> = [];

vi.mock("@/components/providers/forms/ProviderForm", () => ({
  ProviderForm: ({
    onSubmit,
    onSubmitReadyChange,
    onManageAuthAccounts,
  }: {
    onSubmit: (values: ProviderFormValues) => void;
    onSubmitReadyChange?: (isReady: boolean) => void;
    onManageAuthAccounts?: (target: "codex_oauth") => void;
  }) => {
    useEffect(() => {
      if (onSubmitReadyChange) {
        submitReadyCallbacks.push(onSubmitReadyChange);
        onSubmitReadyChange(mockFormReady);
      }
    }, [onSubmitReadyChange]);
    return (
      <form
        id="provider-form"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(mockFormValues);
        }}
      >
        <button
          type="button"
          onClick={() => onManageAuthAccounts?.("codex_oauth")}
        >
          manage-auth
        </button>
      </form>
    );
  },
}));

vi.mock("@/components/providers/AuthSettingsPanel", () => ({
  AuthSettingsPanel: ({ target }: { target: string | null }) =>
    target ? <div data-testid="auth-settings-panel">{target}</div> : null,
}));

describe("AddProviderForm", () => {
  beforeEach(() => {
    mockFormReady = true;
    submitReadyCallbacks = [];
    mockFormValues = {
      name: "Test Provider",
      websiteUrl: "https://provider.example.com",
      settingsConfig: JSON.stringify({ env: {}, config: {} }),
      meta: {
        custom_endpoints: {
          "https://api.new-endpoint.com": {
            url: "https://api.new-endpoint.com",
            addedAt: 1,
          },
        },
      },
    };
  });

  it("使用 ProviderForm 返回的自定义端点", async () => {
    const handleSubmit = vi.fn().mockResolvedValue(undefined);
    const handleOpenChange = vi.fn();

    render(
      <AddProviderForm
        onDone={handleOpenChange}
        appId="claude"
        onSubmit={handleSubmit}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", {
        name: "common.add",
      }),
    );

    await waitFor(() => expect(handleSubmit).toHaveBeenCalledTimes(1));

    const submitted = handleSubmit.mock.calls[0][0];
    expect(submitted.meta?.custom_endpoints).toEqual(
      mockFormValues.meta?.custom_endpoints,
    );
    expect(handleOpenChange).toHaveBeenCalledTimes(1);
  });

  it("在缺少自定义端点时回退到配置中的 baseUrl", async () => {
    const handleSubmit = vi.fn().mockResolvedValue(undefined);

    mockFormValues = {
      name: "Base URL Provider",
      websiteUrl: "",
      settingsConfig: JSON.stringify({
        env: { ANTHROPIC_BASE_URL: "https://claude.base" },
        config: {},
      }),
    };

    render(
      <AddProviderForm
        onDone={vi.fn()}
        appId="claude"
        onSubmit={handleSubmit}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", {
        name: "common.add",
      }),
    );

    await waitFor(() => expect(handleSubmit).toHaveBeenCalledTimes(1));

    const submitted = handleSubmit.mock.calls[0][0];
    expect(submitted.meta?.custom_endpoints).toEqual({
      "https://claude.base": {
        url: "https://claude.base",
        addedAt: expect.any(Number),
        lastUsed: undefined,
      },
    });
  });

  it("submits the optional managed account from the Codex Official preset", async () => {
    const handleSubmit = vi.fn().mockResolvedValue(undefined);
    const officialPresetIndex = codexProviderPresets.findIndex(
      (preset) =>
        preset.category === "official" && preset.providerType === "codex_oauth",
    );
    expect(officialPresetIndex).toBeGreaterThanOrEqual(0);

    mockFormValues = {
      name: "OpenAI Official",
      websiteUrl: "https://chatgpt.com/codex",
      settingsConfig: JSON.stringify({ auth: {}, config: "" }),
      presetId: `codex-${officialPresetIndex}`,
      presetCategory: "official",
      meta: {
        providerType: "codex_oauth",
        authBinding: {
          source: "managed_account",
          authProvider: "codex_oauth",
          accountId: "acct-managed",
        },
      },
    };

    render(
      <AddProviderForm
        appId="codex"
        onSubmit={handleSubmit}
        onDone={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "common.add" }));

    await waitFor(() => expect(handleSubmit).toHaveBeenCalledTimes(1));
    expect(handleSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        category: "official",
        meta: expect.objectContaining({
          authBinding: {
            source: "managed_account",
            authProvider: "codex_oauth",
            accountId: "acct-managed",
          },
        }),
      }),
    );
    expect(handleSubmit.mock.calls[0][0]).not.toHaveProperty(
      "ensureCodexOfficialSeed",
    );
  });

  it("切换 app 标签时收起嵌套的托管账号面板（聚合页形态）", async () => {
    const props = {
      onDone: vi.fn(),
      onSubmit: vi.fn(),
    };
    const { rerender } = render(<AddProviderForm appId="codex" {...props} />);

    fireEvent.click(screen.getByRole("button", { name: "manage-auth" }));
    expect(screen.getByTestId("auth-settings-panel")).toHaveTextContent(
      "codex_oauth",
    );

    // 聚合页没有 open 翻转：等价的复位时机是切换 app 标签。
    rerender(<AddProviderForm appId="gemini" {...props} />);
    rerender(<AddProviderForm appId="codex" {...props} />);

    await waitFor(() => {
      expect(
        screen.queryByTestId("auth-settings-panel"),
      ).not.toBeInTheDocument();
    });
  });

  it("新建 Grok Build 自定义供应商时不补默认 Grok 图标", async () => {
    const handleSubmit = vi.fn().mockResolvedValue(undefined);

    mockFormValues = {
      name: "tes 1",
      websiteUrl: "",
      icon: "",
      iconColor: "",
      settingsConfig: JSON.stringify({
        config: `[models]
default = "grok-4.5"

[model."grok-4.5"]
model = "grok-4.5"
base_url = "https://grok.example.com/v1"
name = "tes 1"
api_key = "secret"
api_backend = "responses"
context_window = 500000
`,
      }),
    };

    render(
      <AddProviderForm
        onDone={vi.fn()}
        appId="grokbuild"
        onSubmit={handleSubmit}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "common.add" }));

    await waitFor(() => expect(handleSubmit).toHaveBeenCalledTimes(1));

    const submitted = handleSubmit.mock.calls[0][0];
    expect(submitted.icon).toBeUndefined();
    expect(submitted.iconColor).toBeUndefined();
  });

  it("Pi 添加供应商时仅提交供应商目录", async () => {
    const handleSubmit = vi.fn().mockResolvedValue(undefined);
    mockFormValues = {
      name: "Pi Provider",
      providerKey: "pi-provider",
      websiteUrl: "",
      settingsConfig: JSON.stringify({
        baseUrl: "https://api.example.com/v1",
        models: [
          { id: "selected-model", name: "Selected" },
          { id: "other-model", name: "Other" },
        ],
      }),
      meta: {
        isPartner: true,
        endpointAutoSelect: true,
        custom_endpoints: {
          "https://failover.example.com/v1": {
            url: "https://failover.example.com/v1",
            addedAt: 1,
          },
        },
      },
    };

    render(
      <AddProviderForm onDone={vi.fn()} appId="pi" onSubmit={handleSubmit} />,
    );

    fireEvent.click(screen.getByRole("button", { name: "common.add" }));
    await waitFor(() => expect(handleSubmit).toHaveBeenCalledTimes(1));
    expect(handleSubmit.mock.calls[0][0]).toMatchObject({
      providerKey: "pi-provider",
      meta: { isPartner: true },
    });
    expect(handleSubmit.mock.calls[0][0]).not.toHaveProperty(
      "piActivateModelId",
    );
  });

  it("重新打开 Pi 表单后忽略上一轮的就绪回调", async () => {
    const props = {
      onDone: vi.fn(),
      appId: "pi" as const,
      onSubmit: vi.fn(),
    };
    const { rerender } = render(<AddProviderForm key="round-1" {...props} />);

    const addButton = await screen.findByRole("button", { name: "common.add" });
    await waitFor(() => expect(addButton).toBeEnabled());
    const staleCallback = submitReadyCallbacks.at(-1);
    expect(staleCallback).toBeDefined();

    mockFormReady = false;
    rerender(<AddProviderForm key="round-2" {...props} />);
    const reopenedButton = await screen.findByRole("button", {
      name: "common.add",
    });
    await waitFor(() => expect(reopenedButton).toBeDisabled());

    act(() => staleCallback?.(true));
    expect(reopenedButton).toBeDisabled();
  });
});
