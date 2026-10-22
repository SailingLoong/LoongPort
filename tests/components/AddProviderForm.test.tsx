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
import { universalProvidersApi } from "@/lib/api";
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

vi.mock("@/components/universal/UniversalProviderFormModal", () => ({
  UniversalProviderFormModal: ({ onSave, isOpen, onClose }: any) => (
    <>
      {isOpen && <input aria-label="New universal draft" />}
      <button
        onClick={() => {
          onSave({ id: "synthetic-universal" });
          onClose();
        }}
      >
        Save universal test
      </button>
    </>
  ),
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
      meta: {},
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

it("announces universal save start before API work, including its independent save path", async () => {
  const order: string[] = [];
  const upsert = vi
    .spyOn(universalProvidersApi, "upsert")
    .mockImplementation(async () => {
      order.push("upsert");
      return true;
    });
  const sync = vi
    .spyOn(universalProvidersApi, "sync")
    .mockResolvedValue(undefined as never);
  const done = vi.fn();
  render(
    <AddProviderForm
      appId="codex"
      onSubmit={vi.fn()}
      onSaveStart={() => order.push("start")}
      onDone={done}
    />,
  );
  fireEvent.click(screen.getByText("Save universal test"));
  await waitFor(() => expect(done).toHaveBeenCalledWith("saved", 1));
  expect(order).toEqual(["start", "upsert"]);
  upsert.mockRestore();
  sync.mockRestore();
});

it("retains each universal save's own navigation epoch across overlapping requests", async () => {
  const resolves: Array<() => void> = [];
  const upsert = vi
    .spyOn(universalProvidersApi, "upsert")
    .mockImplementation(
      () =>
        new Promise<boolean>((resolve) => resolves.push(() => resolve(true))),
    );
  const sync = vi
    .spyOn(universalProvidersApi, "sync")
    .mockResolvedValue(undefined as never);
  let epoch = 10;
  const done = vi.fn();
  render(
    <AddProviderForm
      appId="codex"
      onSubmit={vi.fn()}
      onSaveStart={() => epoch++}
      onDone={done}
    />,
  );
  fireEvent.click(screen.getByText("Save universal test"));
  fireEvent.click(screen.getByText("Save universal test"));
  await act(async () => resolves[0]());
  expect(done).toHaveBeenLastCalledWith("saved", 10);
  await act(async () => resolves[1]());
  expect(done).toHaveBeenLastCalledWith("saved", 11);
  upsert.mockRestore();
  sync.mockRestore();
});

it("does not close or navigate away from a newer universal draft when an old save completes", async () => {
  let resolveSave!: () => void;
  const upsert = vi.spyOn(universalProvidersApi, "upsert").mockImplementation(
    () =>
      new Promise<boolean>((resolve) => {
        resolveSave = () => resolve(true);
      }),
  );
  const sync = vi
    .spyOn(universalProvidersApi, "sync")
    .mockResolvedValue(undefined as never);
  const done = vi.fn();
  render(<AddProviderForm appId="codex" onSubmit={vi.fn()} onDone={done} />);
  fireEvent.mouseDown(
    screen.getByRole("tab", { name: "provider.tabUniversal" }),
    { button: 0, ctrlKey: false },
  );
  fireEvent.click(
    screen.getByRole("button", { name: "universalProvider.add" }),
  );
  fireEvent.click(screen.getAllByText("Save universal test").at(-1)!);
  fireEvent.click(
    screen.getByRole("button", { name: "universalProvider.add" }),
  );
  fireEvent.change(screen.getByLabelText("New universal draft"), {
    target: { value: "keep my newer draft" },
  });
  await act(async () => resolveSave());
  expect(screen.getByLabelText("New universal draft")).toHaveValue(
    "keep my newer draft",
  );
  expect(done).not.toHaveBeenCalled();
  upsert.mockRestore();
  sync.mockRestore();
});

it("ignores an old universal save after switching to the app-specific draft", async () => {
  let resolveSave!: () => void;
  const upsert = vi.spyOn(universalProvidersApi, "upsert").mockImplementation(
    () =>
      new Promise<boolean>((resolve) => {
        resolveSave = () => resolve(true);
      }),
  );
  const sync = vi
    .spyOn(universalProvidersApi, "sync")
    .mockResolvedValue(undefined as never);
  const done = vi.fn();
  render(<AddProviderForm appId="codex" onSubmit={vi.fn()} onDone={done} />);
  fireEvent.mouseDown(
    screen.getByRole("tab", { name: "provider.tabUniversal" }),
    { button: 0, ctrlKey: false },
  );
  fireEvent.click(screen.getAllByText("Save universal test").at(-1)!);
  fireEvent.mouseDown(screen.getByRole("tab", { name: /apps.codex/ }), {
    button: 0,
    ctrlKey: false,
  });
  await act(async () => resolveSave());
  expect(done).not.toHaveBeenCalled();
  upsert.mockRestore();
  sync.mockRestore();
});

it("ignores an old app-specific save after opening a universal draft", async () => {
  mockFormReady = true;
  mockFormValues = {
    name: "Old draft",
    websiteUrl: "",
    settingsConfig: JSON.stringify({ auth: {}, config: "" }),
  };
  let resolveSave!: () => void;
  const submit = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        resolveSave = resolve;
      }),
  );
  const done = vi.fn();
  render(<AddProviderForm appId="codex" onSubmit={submit} onDone={done} />);
  fireEvent.click(screen.getByRole("button", { name: "common.add" }));
  await waitFor(() => expect(submit).toHaveBeenCalledTimes(1));
  fireEvent.mouseDown(
    screen.getByRole("tab", { name: "provider.tabUniversal" }),
    { button: 0, ctrlKey: false },
  );
  fireEvent.click(
    screen.getByRole("button", { name: "universalProvider.add" }),
  );
  fireEvent.change(screen.getByLabelText("New universal draft"), {
    target: { value: "keep universal draft" },
  });
  await act(async () => resolveSave());
  expect(done).not.toHaveBeenCalled();
  expect(screen.getByLabelText("New universal draft")).toHaveValue(
    "keep universal draft",
  );
});

it("passes the MiniMax Code native key and does not offer universal providers", async () => {
  mockFormReady = true;
  mockFormValues = {
    name: "MiniMax custom",
    websiteUrl: "",
    providerKey: "native-key",
    settingsConfig: JSON.stringify({
      kind: "custom",
      enabled: true,
      api: "openai-completions",
      options: {},
      models: {},
    }),
  };
  const onSubmit = vi.fn();
  render(
    <AddProviderForm appId="mcode" onSubmit={onSubmit} onDone={vi.fn()} />,
  );
  expect(
    screen.queryByRole("tab", { name: "provider.tabUniversal" }),
  ).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "common.add" }));
  await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
  expect(onSubmit.mock.calls[0][0].providerKey).toBe("native-key");
});
