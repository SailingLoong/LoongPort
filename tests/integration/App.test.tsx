import { Suspense } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  render,
  screen,
  waitFor,
  fireEvent,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, it, expect, beforeEach, vi } from "vitest";
import { http, HttpResponse } from "msw";
// 在收集阶段完成模块加载：若把 import 留在用例体内，超时后它仍可能续跑并越过 cleanup 再挂载 App。
import App from "@/App";
import type { ApplicationRouting } from "@/lib/api/applicationRouting";
import {
  LAST_APP_STORAGE_KEY,
  LAST_VIEW_STORAGE_KEY,
} from "@/config/constants";
import {
  getProviders,
  resetProviderState,
  setCurrentProviderId,
  setLiveProviderIds,
  setProviders,
} from "../msw/state";
import { emitTauriEvent } from "../msw/tauriMocks";
import { server } from "../msw/server";

const workspaceHarness = vi.hoisted(() => ({ real: false }));
const editResultMock = vi.hoisted(() => vi.fn());
const toastSuccessMock = vi.fn();
const toastErrorMock = vi.fn();
const toastInfoMock = vi.fn();
const skillsPanelMocks = vi.hoisted(() => ({
  checkUpdates: vi.fn(),
  openDiscovery: vi.fn(),
}));

vi.mock("sonner", () => ({
  toast: {
    success: (...args: unknown[]) => toastSuccessMock(...args),
    error: (...args: unknown[]) => toastErrorMock(...args),
    info: (...args: unknown[]) => toastInfoMock(...args),
  },
}));

// These integration cases exercise provider actions; workspace presentation has
// its own interaction tests. Keep the existing action harness immediately visible.
vi.mock(
  "@/components/applications/ApplicationWorkspace",
  async (importOriginal) => {
    const actual =
      await importOriginal<
        typeof import("@/components/applications/ApplicationWorkspace")
      >();
    return {
      ApplicationWorkspace: (props: any) =>
        workspaceHarness.real ? (
          <actual.ApplicationWorkspace {...props} />
        ) : (
          <>
            {typeof props.children === "function"
              ? props.children(false)
              : props.children}
            <button
              onClick={() => props.onOpenAccount({ kind: "relay", id: 1 })}
            >
              open-account
            </button>
          </>
        ),
    };
  },
);

vi.mock("@/components/providers/ProviderList", () => ({
  ProviderList: ({
    providers,
    onSwitch,
    onEdit,
    onDuplicate,
    onConfigureUsage,
    onOpenWebsite,
    onCreate,
    onDelete,
    onRemoveFromConfig,
  }: any) =>
    (() => {
      const currentProvider = Object.values(providers).find(
        (provider: any) => provider.presentation?.isCurrent,
      );
      return (
        <div>
          <div data-testid="provider-list">{JSON.stringify(providers)}</div>
          <div data-testid="current-provider">
            {(currentProvider as any)?.id}
          </div>
          <button onClick={() => onSwitch(currentProvider)}>switch</button>
          <button onClick={() => onEdit(currentProvider)}>edit</button>
          <button onClick={() => onDuplicate(currentProvider)}>
            duplicate
          </button>
          <button onClick={() => onConfigureUsage(currentProvider)}>
            usage
          </button>
          <button onClick={() => onOpenWebsite("https://example.com")}>
            open-website
          </button>
          <button onClick={() => onDelete(Object.values(providers)[0])}>
            delete
          </button>
          <button
            onClick={() => onRemoveFromConfig?.(Object.values(providers)[0])}
          >
            remove
          </button>
          <button onClick={() => onCreate?.()}>create</button>
        </div>
      );
    })(),
}));

vi.mock("@/components/relay/AddHubPage", () => ({
  // 只验证「+ 聚合页 → 提交 → 回供应商列表」这条数据链；
  // 标签结构与切换由 AddHubPage.test.tsx 自己测。
  AddHubPage: ({ onBack, onAddProvider, sourceAppId: appId }: any) => (
    <div data-testid="add-provider-dialog">
      <button
        onClick={() =>
          void (async () => {
            // 真实表单的时序：await 提交成功 → onDone → 聚合页返回。
            await onAddProvider({
              name: `New ${appId} Provider`,
              settingsConfig: {},
              category: "custom",
              sortIndex: 99,
            });
            onBack();
          })()
        }
      >
        confirm-add
      </button>
      <button onClick={() => onBack()}>close-add</button>
    </div>
  ),
}));

vi.mock("@/components/providers/EditProviderDialog", () => ({
  EditProviderDialog: ({
    open,
    provider,
    onSubmit,
    onOpenChange,
    mutationsDisabled,
  }: any) =>
    open ? (
      <div
        data-testid="edit-provider-dialog"
        data-mutations-disabled={String(!!mutationsDisabled)}
      >
        <button
          onClick={() =>
            onSubmit({
              provider: {
                ...provider,
                name: `${provider.name}-edited`,
              },
              originalId: provider.id,
            })
          }
        >
          confirm-edit
        </button>
        {[false, true].map((queryOnly) => (
          <button
            key={String(queryOnly)}
            onClick={() => {
              const request = {
                id: "094b4732-d3c9-43f4-a123-009293a85273",
                providerId: provider.id,
                draftDigest: "a".repeat(64),
                revision: "b".repeat(64),
              };
              void Promise.resolve(
                onSubmit({
                  provider,
                  originalId: provider.id,
                  edit: {
                    request,
                    deleteCredential: false,
                    ...(queryOnly ? { queryOnly: true } : {}),
                  },
                }),
              ).then(editResultMock);
            }}
          >
            {queryOnly ? "query-original-edit" : "confirm-bound-edit"}
          </button>
        ))}
        <button onClick={() => onOpenChange(false)}>close-edit</button>
      </div>
    ) : null,
}));

vi.mock("@/components/UsageScriptModal", () => ({
  default: ({ isOpen, provider, onSave, onClose }: any) =>
    isOpen ? (
      <div data-testid="usage-modal">
        <span data-testid="usage-provider">{provider?.id}</span>
        <button onClick={() => onSave("script-code")}>save-script</button>
        <button onClick={() => onClose()}>close-usage</button>
      </div>
    ) : null,
}));

vi.mock("@/components/ConfirmDialog", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@/components/ConfirmDialog")>();
  return {
    ConfirmDialog: (props: any) =>
      workspaceHarness.real ? (
        <actual.ConfirmDialog {...props} />
      ) : props.isOpen ? (
        <div data-testid="confirm-dialog">
          <div data-testid="confirm-message">{props.message}</div>
          <button onClick={() => props.onConfirm()}>confirm-delete</button>
          <button onClick={() => props.onCancel()}>cancel-delete</button>
        </div>
      ) : null,
  };
});

vi.mock("@/components/skills/UnifiedSkillsPanel", async () => {
  const React = await import("react");
  const MockUnifiedSkillsPanel = React.forwardRef(
    ({ onCheckUpdatesStateChange }: any, ref) => {
      React.useEffect(() => {
        onCheckUpdatesStateChange?.({ isChecking: false, hasSkills: true });
        return () =>
          onCheckUpdatesStateChange?.({
            isChecking: false,
            hasSkills: false,
          });
      }, [onCheckUpdatesStateChange]);
      React.useImperativeHandle(ref, () => ({
        openDiscovery: skillsPanelMocks.openDiscovery,
        openImport: vi.fn(),
        openInstallFromZip: vi.fn(),
        openRestoreFromBackup: vi.fn(),
        checkUpdates: skillsPanelMocks.checkUpdates,
      }));
      return <div data-testid="unified-skills-panel" />;
    },
  );
  MockUnifiedSkillsPanel.displayName = "MockUnifiedSkillsPanel";
  return { default: MockUnifiedSkillsPanel };
});

vi.mock("@/components/UpdateBadge", () => ({
  UpdateBadge: ({ onClick }: any) => (
    <button onClick={onClick}>update-badge</button>
  ),
}));

vi.mock("@/components/mcp/McpPanel", () => ({
  default: ({ open, onOpenChange }: any) =>
    open ? (
      <div data-testid="mcp-panel">
        <button onClick={() => onOpenChange(false)}>close-mcp</button>
      </div>
    ) : (
      <button onClick={() => onOpenChange(true)}>open-mcp</button>
    ),
}));

const renderApp = (client = new QueryClient()) => {
  // This harness mocks the workspace; seed its successful read in the shared cache.
  for (const app of workspaceHarness.real
    ? []
    : ["claude", "codex", "gemini", "grok"]) {
    client.setQueryData(["applicationRouting", app], {
      autoFailoverEnabled: false,
      routingActive: false,
    });
  }
  return render(
    <QueryClientProvider client={client}>
      <Suspense fallback={<div data-testid="loading">loading</div>}>
        <App />
      </Suspense>
    </QueryClientProvider>,
  );
};

async function selectApplication(name: string) {
  const user = userEvent.setup();
  if (!screen.queryByRole("button", { name })) {
    await user.click(screen.getByRole("button", { name: "appSwitcher.add" }));
  }
  await user.click(await screen.findByRole("button", { name }));
}

async function renderRoutingDraft(client = new QueryClient()) {
  workspaceHarness.real = true;
  const apply = vi.fn();
  server.use(
    http.post(
      "http://tauri.local/get_application_routing",
      async ({ request }) => {
        const { appType } = (await request.json()) as {
          appType: Parameters<typeof getProviders>[0];
        };
        const ids = Object.keys(getProviders(appType));
        return HttpResponse.json({
          autoFailoverEnabled: true,
          routingActive: true,
          chainIds: ids,
          model: null,
          modelOptions: [],
          tiers: ids.map((id, index) => ({
            providerId: id,
            position: index,
            isCurrent: index === 0,
            canFailover: true,
            canVerifyModels: false,
            skipReason: null,
            models: [],
            subscriptionWindows: [],
          })),
        });
      },
    ),
    http.post("http://tauri.local/get_order_profiles", () =>
      HttpResponse.json({ profiles: [], current: "default" }),
    ),
    http.post("http://tauri.local/get_model_verification_summaries", () =>
      HttpResponse.json([]),
    ),
    http.post("http://tauri.local/apply_application_routing", () => {
      apply();
      return HttpResponse.json({ status: "switched", warnings: [] });
    }),
  );
  renderApp(client);
  await userEvent.type(await screen.findByRole("searchbox"), "Custom");
  expect(await screen.findByText("applications.pendingChanges")).toBeVisible();
  return apply;
}

describe("App integration with MSW", () => {
  beforeEach(() => {
    resetProviderState();
    workspaceHarness.real = false;
    editResultMock.mockReset();
    toastSuccessMock.mockReset();
    toastErrorMock.mockReset();
    toastInfoMock.mockReset();
    // Start each independent flow from the persisted application view.
    localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
    localStorage.setItem(LAST_APP_STORAGE_KEY, "claude");
  });

  it("asks before leaving a real routing draft through the sidebar", async () => {
    const apply = await renderRoutingDraft();
    await userEvent.click(
      screen.getByRole("button", { name: "client.services" }),
    );
    expect(
      screen.getByRole("dialog", { name: "applications.leaveDraftTitle" }),
    ).toBeVisible();
    expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");
    expect(apply).not.toHaveBeenCalled();
  });

  it.each(["continue", "Escape", "close"] as const)(
    "keeps the real draft and forgets the deferred destination after %s",
    async (dismiss) => {
      const apply = await renderRoutingDraft();
      await userEvent.click(
        screen.getByRole("button", { name: "client.services" }),
      );
      const dialog = screen.getByRole("dialog", {
        name: "applications.leaveDraftTitle",
      });
      if (dismiss === "Escape") await userEvent.keyboard("{Escape}");
      else
        await userEvent.click(
          within(dialog).getByRole("button", {
            name:
              dismiss === "close"
                ? "common.close"
                : "loongport.tier.editConfirmButton",
          }),
        );
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");
      expect(screen.getByRole("searchbox")).toHaveValue("Custom");
      expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe("claude");
      expect(screen.getByText("applications.pendingChanges")).toBeVisible();
      await selectApplication("Codex");
      await userEvent.click(
        screen.getByRole("button", { name: "applications.discardAndLeave" }),
      );
      await waitFor(() =>
        expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe("codex"),
      );
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");
      expect(apply).not.toHaveBeenCalled();
    },
  );

  it("keeps the displayed app and restore key on cancel, then discards only the local draft on confirmed app switch", async () => {
    const apply = await renderRoutingDraft();
    const claudeTab = screen.getByRole("button", { name: "Claude Code" });
    await selectApplication("Codex");
    expect(claudeTab).toHaveAttribute("aria-pressed", "true");
    expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe("claude");
    await userEvent.click(
      screen.getByRole("button", { name: "loongport.tier.editConfirmButton" }),
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(claudeTab).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Codex" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe("claude");
    expect(screen.getByRole("searchbox")).toHaveValue("Custom");
    expect(screen.getByText("applications.pendingChanges")).toBeVisible();
    await selectApplication("Codex");
    await userEvent.click(
      screen.getByRole("button", { name: "applications.discardAndLeave" }),
    );
    await waitFor(() =>
      expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe("codex"),
    );
    expect(screen.getByRole("button", { name: "Codex" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await selectApplication("Claude Code");
    await waitFor(() =>
      expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe("claude"),
    );
    expect(await screen.findByRole("searchbox")).toHaveValue("");
    expect(
      screen.queryByText("applications.pendingChanges"),
    ).not.toBeInTheDocument();
    expect(apply).not.toHaveBeenCalled();
  });

  it("keeps the first pending destination and consumes repeated confirmation only once", async () => {
    const apply = await renderRoutingDraft();
    const services = screen.getByRole("button", { name: "client.services" });
    const resources = screen.getByRole("button", { name: "client.resources" });
    act(() => {
      fireEvent.click(services);
      fireEvent.click(resources);
    });
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    const discard = screen.getByRole("button", {
      name: "applications.discardAndLeave",
    });
    act(() => {
      fireEvent.click(discard);
      fireEvent.click(discard);
    });
    await waitFor(() =>
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("services"),
    );
    expect(
      screen.queryByRole("button", { name: "common.back" }),
    ).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "client.applications" }),
    );
    expect(await screen.findByRole("searchbox")).toHaveValue("");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(apply).not.toHaveBeenCalled();
  });

  it("does not prompt for the same destination or a clean workspace", async () => {
    const apply = await renderRoutingDraft();
    await userEvent.click(
      screen.getByRole("button", { name: "client.applications" }),
    );
    await selectApplication("Claude Code");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("searchbox")).toHaveValue("Custom");
    await userEvent.click(
      screen.getByRole("button", { name: "applications.discardOrder" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "client.services" }),
    );
    await waitFor(() =>
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("services"),
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(apply).not.toHaveBeenCalled();
  });

  it("keeps an in-flight apply on its workspace and only discards unsubmitted changes after it settles", async () => {
    const apply = await renderRoutingDraft();
    let settle!: (response: Response) => void;
    const response = new Promise<Response>((resolve) => {
      settle = resolve;
    });
    server.use(
      http.post("http://tauri.local/apply_application_routing", () => {
        apply();
        return response;
      }),
    );
    try {
      await userEvent.click(
        screen.getByRole("button", { name: /applications.applyOrder/ }),
      );
      await userEvent.click(
        within(
          screen.getByRole("dialog", { name: "applications.reviewChanges" }),
        ).getByRole("button", { name: "common.confirm" }),
      );
      await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
      const services = screen.getByRole("button", { name: "client.services" });
      await userEvent.click(services);
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      await selectApplication("Codex");
      await userEvent.click(services);
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "Claude Code" }),
      ).toHaveAttribute("aria-pressed", "true");
      expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe("claude");
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");
      expect(screen.getByRole("searchbox")).toHaveValue("Custom");
      expect(
        screen.getByRole("button", { name: "applications.discardOrder" }),
      ).toBeDisabled();
      expect(toastInfoMock).toHaveBeenCalledWith("common.saving");
      expect(apply).toHaveBeenCalledTimes(1);
      settle(
        HttpResponse.json(
          { message: "The application result is unavailable" },
          { status: 500 },
        ),
      );
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "applications.discardOrder" }),
        ).toBeEnabled(),
      );
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");
      await userEvent.click(services);
      await userEvent.click(
        screen.getByRole("button", {
          name: "loongport.tier.editConfirmButton",
        }),
      );
      expect(screen.getByRole("searchbox")).toHaveValue("Custom");
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");
      await userEvent.click(services);
      await userEvent.click(
        screen.getByRole("button", { name: "applications.discardAndLeave" }),
      );
      await waitFor(() =>
        expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("services"),
      );
      expect(apply).toHaveBeenCalledTimes(1);
    } finally {
      settle(
        HttpResponse.json(
          { message: "The application result is unavailable" },
          { status: 500 },
        ),
      );
    }
  });

  it("does not confuse a read-only routing refresh with an in-flight write", async () => {
    const client = new QueryClient();
    const apply = await renderRoutingDraft(client);
    const key = ["applicationRouting", "claude"];
    const facts = client.getQueryData<ApplicationRouting>(key);
    let finish!: (response: Response) => void;
    const response = new Promise<Response>((resolve) => {
      finish = resolve;
    });
    server.use(
      http.post("http://tauri.local/get_application_routing", () => response),
    );
    try {
      void client.invalidateQueries({ queryKey: key });
      await waitFor(() => expect(client.isFetching({ queryKey: key })).toBe(1));
      await userEvent.click(
        screen.getByRole("button", { name: "client.services" }),
      );
      expect(
        screen.getByRole("dialog", { name: "applications.leaveDraftTitle" }),
      ).toBeVisible();
      await userEvent.click(
        screen.getByRole("button", {
          name: "loongport.tier.editConfirmButton",
        }),
      );
      expect(screen.getByRole("searchbox")).toHaveValue("Custom");
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");
      expect(toastInfoMock).not.toHaveBeenCalled();
      expect(apply).not.toHaveBeenCalled();
    } finally {
      finish(HttpResponse.json(facts));
      await waitFor(() => expect(client.isFetching({ queryKey: key })).toBe(0));
    }
  });

  it("guards the existing settings shortcut and cancels without triggering Back", async () => {
    const apply = await renderRoutingDraft();
    fireEvent.keyDown(window, { key: ",", metaKey: true });
    expect(
      screen.getByRole("dialog", { name: "applications.leaveDraftTitle" }),
    ).toBeVisible();
    await userEvent.keyboard("{Escape}");
    expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");
    expect(screen.getByRole("searchbox")).toHaveValue("Custom");
    fireEvent.keyDown(window, { key: ",", metaKey: true });
    await userEvent.click(
      screen.getByRole("button", { name: "applications.discardAndLeave" }),
    );
    await waitFor(() =>
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("settings"),
    );
    expect(apply).not.toHaveBeenCalled();
  });

  it("connects the compact page header and sidebar skip link to the main content", async () => {
    renderApp();
    await screen.findByRole("navigation", { name: "client.navigation" });
    expect(screen.getByRole("banner")).toHaveStyle({ height: "52px" });
    expect(screen.getByRole("main")).toHaveAttribute("id", "main-content");
    expect(screen.getByRole("main")).toHaveAttribute("tabindex", "-1");
  });

  it("opens ZCode without using provider-store or environment commands", async () => {
    localStorage.setItem(LAST_APP_STORAGE_KEY, "zcode");
    const unsupported = vi.fn();
    server.use(
      http.post("http://tauri.local/get_zcode_config", () =>
        HttpResponse.json({ revision: "missing", providers: [] }),
      ),
      http.post("http://tauri.local/get_providers", async ({ request }) => {
        const body = (await request.json()) as { app?: string };
        if (body.app === "zcode") unsupported("get_providers");
        return HttpResponse.json({});
      }),
      http.post(
        "http://tauri.local/check_env_conflicts",
        async ({ request }) => {
          const body = (await request.json()) as { app?: string };
          if (body.app === "zcode") unsupported("check_env_conflicts");
          return HttpResponse.json([]);
        },
      ),
    );
    renderApp();
    expect(
      await screen.findByRole("tab", { name: "Sign-in accounts" }),
    ).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(screen.getByRole("tab", { name: "API configuration" }), {
      key: "Enter",
    });
    expect(
      await screen.findByText("No personal providers yet"),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("provider-list")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "loongport.addEntry.title" }),
    ).not.toBeInTheDocument();
    expect(unsupported).not.toHaveBeenCalled();
  });

  it.each(["save", "remove"] as const)(
    "keeps a pending ZCode %s and its error visible across the settings shortcut",
    async (operation) => {
      localStorage.setItem(LAST_APP_STORAGE_KEY, "zcode");
      let finish!: () => void;
      const pending = new Promise<void>((resolve) => {
        finish = resolve;
      });
      const started = vi.fn();
      server.use(
        http.post("http://tauri.local/get_zcode_config", () =>
          HttpResponse.json({
            revision: "fixture-revision",
            providers: [
              {
                id: "loongport-test",
                name: "Native fixture",
                apiType: "openai-responses",
                baseUrl: "https://api.example/v1",
                models: ["example-model"],
                hasApiKey: true,
                managed: true,
              },
            ],
          }),
        ),
        http.post(
          `http://tauri.local/${operation}_zcode_provider`,
          async () => {
            started();
            await pending;
            return HttpResponse.text(
              "Configuration changed; refresh before writing",
              { status: 409 },
            );
          },
        ),
      );
      renderApp();
      fireEvent.keyDown(
        await screen.findByRole("tab", { name: "API configuration" }),
        { key: "Enter" },
      );
      await screen.findByText("Native fixture");
      fireEvent.click(
        screen.getByRole("button", {
          name: operation === "save" ? "Edit" : "Remove",
        }),
      );
      fireEvent.click(
        operation === "save"
          ? within(screen.getByRole("dialog")).getByRole("button", {
              name: "Save",
            })
          : screen.getByText("confirm-delete"),
      );
      await waitFor(() => expect(started).toHaveBeenCalledTimes(1));
      fireEvent.keyDown(window, { key: ",", ctrlKey: true });
      const keptView = localStorage.getItem(LAST_VIEW_STORAGE_KEY);
      finish();
      expect(keptView).toBe("providers");
      expect(
        await screen.findByText(
          "Configuration changed; refresh before writing",
        ),
      ).toBeInTheDocument();
      fireEvent.click(
        operation === "save"
          ? within(screen.getByRole("dialog")).getByRole("button", {
              name: "Cancel",
            })
          : screen.getByText("cancel-delete"),
      );
      fireEvent.keyDown(window, { key: ",", metaKey: true });
      await waitFor(() =>
        expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("settings"),
      );
    },
  );

  it("opens service onboarding from a saved native app without a blank page", async () => {
    localStorage.setItem(LAST_APP_STORAGE_KEY, "zcode");
    server.use(
      http.post("http://tauri.local/get_zcode_config", () =>
        HttpResponse.json({ revision: "missing", providers: [] }),
      ),
      http.post("http://tauri.local/service_onboarding_status", () =>
        HttpResponse.json({
          shouldPrompt: true,
          completed: false,
          plazaVisible: false,
        }),
      ),
    );
    renderApp();
    expect(
      await screen.findByTestId("add-provider-dialog"),
    ).toBeInTheDocument();
    expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe("codex");
  });

  it.fails(
    "cleans the rendered App when the test times out",
    async () => {
      renderApp();
      // 这条必须真实撞穿自己的 10ms 超时，下一条才是在验证超时后的隔离性。
      await new Promise((resolve) => setTimeout(resolve, 50));
    },
    10,
  );

  it("mounts exactly one App after the timeout cleanup", async () => {
    renderApp();

    expect(
      await screen.findAllByRole("button", { name: "client.image" }),
    ).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "Claude Code" })).toHaveLength(
      1,
    );
  });

  it("switches primary pages through the sidebar without Back or Escape history", async () => {
    renderApp();
    await waitFor(() =>
      expect(screen.getByTestId("provider-list")).toHaveTextContent("claude-1"),
    );
    fireEvent.click(screen.getByRole("button", { name: "client.image" }));
    await waitFor(() =>
      expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe("codex-image"),
    );
    expect(
      screen.getByRole("button", { name: "client.image" }),
    ).toHaveAttribute("aria-current", "page");
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "Claude Code" }),
      ).not.toBeInTheDocument(),
    );
    expect(
      screen.queryByRole("button", { name: "common.back" }),
    ).not.toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("image");
    fireEvent.click(
      screen.getByRole("button", { name: "client.applications" }),
    );
    await waitFor(() =>
      expect(screen.getByTestId("provider-list")).toHaveTextContent("codex-1"),
    );
    expect(
      screen.queryByRole("button", { name: "common.back" }),
    ).not.toBeInTheDocument();
  });

  it("keeps Back and Escape within subordinate resource navigation", async () => {
    renderApp();
    fireEvent.click(
      await screen.findByRole("button", { name: "client.resources" }),
    );
    expect(
      screen.queryByRole("button", { name: "common.back" }),
    ).not.toBeInTheDocument();
    fireEvent.click(
      await screen.findByRole("button", { name: /client.features.skills/ }),
    );
    expect(
      await screen.findByTestId("unified-skills-panel"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "common.back" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "client.resources" }),
    ).toHaveAttribute("aria-current", "page");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("resources");
    expect(
      screen.queryByRole("button", { name: "common.back" }),
    ).not.toBeInTheDocument();
  });

  it("renders one account-detail Back and clears detail through sidebar navigation", async () => {
    renderApp();
    fireEvent.click(
      await screen.findByRole("button", { name: "open-account" }),
    );
    expect(
      await screen.findByRole("heading", { name: "loongport.accounts.detail" }),
    ).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "common.back" })).toHaveLength(
      1,
    );
    fireEvent.click(screen.getByRole("button", { name: "common.back" }));
    fireEvent.click(
      await screen.findByRole("button", { name: "open-account" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "client.resources" }));
    fireEvent.click(screen.getByRole("button", { name: "client.services" }));
    expect(
      screen.queryByRole("heading", { name: "loongport.accounts.detail" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "common.back" }),
    ).not.toBeInTheDocument();
  });

  it("covers basic provider flows via real hooks", async () => {
    renderApp();

    await waitFor(() =>
      expect(screen.getByTestId("provider-list").textContent).toContain(
        "claude-1",
      ),
    );

    await selectApplication("Codex");
    await waitFor(() =>
      expect(screen.getByTestId("provider-list").textContent).toContain(
        "codex-1",
      ),
    );

    fireEvent.click(screen.getByText("usage"));
    expect(screen.getByTestId("usage-modal")).toBeInTheDocument();
    fireEvent.click(screen.getByText("save-script"));
    fireEvent.click(screen.getByText("close-usage"));

    fireEvent.click(screen.getByText("create"));
    expect(
      await screen.findByTestId("add-provider-dialog"),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByText("confirm-add"));
    await waitFor(() =>
      expect(screen.getByTestId("provider-list").textContent).toMatch(
        /New codex Provider/,
      ),
    );

    fireEvent.click(screen.getByText("edit"));
    expect(screen.getByTestId("edit-provider-dialog")).toBeInTheDocument();
    fireEvent.click(screen.getByText("confirm-edit"));
    await waitFor(() =>
      expect(screen.getByTestId("provider-list").textContent).toMatch(
        /-edited/,
      ),
    );

    fireEvent.click(screen.getByText("switch"));
    fireEvent.click(screen.getByText("duplicate"));
    await waitFor(() =>
      expect(screen.getByTestId("provider-list").textContent).toMatch(/copy/),
    );

    fireEvent.click(screen.getByText("open-website"));

    emitTauriEvent("provider-switched", {
      appType: "codex",
      providerId: "codex-2",
    });

    expect(toastErrorMock).not.toHaveBeenCalled();
    expect(toastSuccessMock).toHaveBeenCalled();
  }, 10_000);

  it("shows toast when auto sync fails in background", async () => {
    renderApp();

    await waitFor(() =>
      expect(screen.getByTestId("provider-list").textContent).toContain(
        "claude-1",
      ),
    );

    expect(() => {
      emitTauriEvent("webdav-sync-status-updated", null);
    }).not.toThrow();
    expect(toastErrorMock).not.toHaveBeenCalled();

    emitTauriEvent("webdav-sync-status-updated", {
      source: "auto",
      status: "error",
      error: "network timeout",
    });

    await waitFor(() => {
      expect(toastErrorMock).toHaveBeenCalled();
    });

    toastErrorMock.mockReset();
    toastInfoMock.mockReset();
    expect(() => {
      emitTauriEvent("s3-sync-status-updated", null);
    }).not.toThrow();
    expect(toastErrorMock).not.toHaveBeenCalled();

    emitTauriEvent("s3-sync-status-updated", {
      source: "auto",
      status: "error",
      error: "s3 timeout",
    });

    await waitFor(() => {
      expect(toastErrorMock).toHaveBeenCalled();
    });
  });

  it("duplicates openclaw providers with a generated key that avoids live-only ids", async () => {
    setProviders("openclaw", {
      deepseek: {
        id: "deepseek",
        name: "DeepSeek",
        settingsConfig: {
          baseUrl: "https://api.deepseek.com",
          apiKey: "test-key",
          api: "openai-completions",
          models: [],
        },
        category: "custom",
        sortIndex: 0,
        createdAt: Date.now(),
      },
    });
    setCurrentProviderId("openclaw", "deepseek");
    setLiveProviderIds("openclaw", ["deepseek-copy"]);

    renderApp();

    await selectApplication("OpenClaw");

    await waitFor(() =>
      expect(screen.getByTestId("provider-list").textContent).toContain(
        "deepseek",
      ),
    );

    fireEvent.click(screen.getByText("duplicate"));

    await waitFor(() => {
      const providerList = screen.getByTestId("provider-list").textContent;
      expect(providerList).toContain("deepseek-copy-2");
      expect(providerList).toContain("DeepSeek copy");
    });

    expect(toastErrorMock).not.toHaveBeenCalledWith(
      expect.stringContaining("Provider key is required for openclaw"),
    );
  });

  it("warns without blocking when removing Pi's global default provider", async () => {
    localStorage.setItem(LAST_APP_STORAGE_KEY, "pi");
    setProviders("pi", {
      custom: {
        id: "custom",
        name: "Custom Pi",
        settingsConfig: {
          baseUrl: "https://api.example.com/v1",
          apiKey: "test-key",
          api: "openai-completions",
          models: [{ id: "model-a" }],
        },
        category: "custom",
        sortIndex: 0,
        createdAt: Date.now(),
      },
    });
    server.use(
      http.post("http://tauri.local/get_pi_current_state", () =>
        HttpResponse.json({
          enabledProviderIds: ["custom"],
          defaultProviderId: "custom",
        }),
      ),
    );

    renderApp();

    await waitFor(() =>
      expect(screen.getByTestId("provider-list").textContent).toContain(
        "Custom Pi",
      ),
    );
    fireEvent.click(screen.getByText("remove"));

    expect(screen.getByTestId("confirm-message")).toHaveTextContent(
      "confirm.piDefaultProviderWarning",
    );
    fireEvent.click(screen.getByText("confirm-delete"));
    await waitFor(() =>
      expect(screen.queryByTestId("confirm-dialog")).not.toBeInTheDocument(),
    );
  });

  it("opens Skills through extension resources and hosts its check-update action", async () => {
    renderApp();
    fireEvent.click(
      await screen.findByRole("button", { name: "client.resources" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /client.features.skills/ }),
    );

    expect(
      await screen.findByTestId("unified-skills-panel"),
    ).toBeInTheDocument();
    const checkUpdatesButton = await screen.findByRole("button", {
      name: "skills.checkUpdates",
    });
    await waitFor(() => expect(checkUpdatesButton).toBeEnabled());

    fireEvent.click(checkUpdatesButton);
    expect(skillsPanelMocks.checkUpdates).toHaveBeenCalledTimes(1);
  });

  it("routes the Skills discover toolbar action through the panel guard", async () => {
    localStorage.setItem(LAST_VIEW_STORAGE_KEY, "skills");
    renderApp();

    expect(
      await screen.findByTestId("unified-skills-panel"),
    ).toBeInTheDocument();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "skills.discover",
      }),
    );

    expect(skillsPanelMocks.openDiscovery).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("unified-skills-panel")).toBeInTheDocument();
  });
});

describe("U02 App original result callback", () => {
  it("returns the original outcome and permits a read-only query while application writes are blocked", async () => {
    localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
    localStorage.setItem(LAST_APP_STORAGE_KEY, "claude");
    const row = {
      id: "u02-app-row",
      name: "Synthetic App row",
      settingsConfig: {},
    };
    setProviders("claude", { [row.id]: row });
    setCurrentProviderId("claude", row.id);
    const confirms = vi.fn();
    const queries = vi.fn();
    server.use(
      http.post(
        "http://tauri.local/confirm_provider_edit",
        async ({ request }) => {
          const input = (await request.json()) as any;
          confirms();
          return HttpResponse.json({
            app: input.app,
            request: input.request,
            status: "unknown",
          });
        },
      ),
      http.post(
        "http://tauri.local/query_provider_edit",
        async ({ request }) => {
          const input = (await request.json()) as any;
          queries();
          return HttpResponse.json({
            app: input.app,
            request: input.request,
            status: "completed",
          });
        },
      ),
    );
    editResultMock.mockReset();
    const client = new QueryClient();
    const view = renderApp(client);
    try {
      await waitFor(() =>
        expect(screen.getByTestId("provider-list").textContent).toContain(
          row.id,
        ),
      );
      fireEvent.click(screen.getByText("edit"));
      await screen.findByTestId("edit-provider-dialog");
      fireEvent.click(screen.getByText("confirm-bound-edit"));
      await waitFor(() =>
        expect(editResultMock).toHaveBeenCalledWith(
          expect.objectContaining({ status: "unknown" }),
        ),
      );
      expect(screen.getByTestId("edit-provider-dialog")).toBeInTheDocument();
      act(() => {
        client.setQueryData(["applicationRouting", "claude"], {
          autoFailoverEnabled: false,
          routingActive: false,
          modeState: { canWrite: false, status: "pending" },
        });
      });
      await waitFor(() =>
        expect(screen.getByTestId("edit-provider-dialog")).toHaveAttribute(
          "data-mutations-disabled",
          "true",
        ),
      );
      fireEvent.click(screen.getByText("confirm-bound-edit"));
      await waitFor(() =>
        expect(editResultMock).toHaveBeenCalledWith(
          expect.objectContaining({ status: "blocked" }),
        ),
      );
      fireEvent.click(screen.getByText("query-original-edit"));
      await waitFor(() =>
        expect(editResultMock).toHaveBeenCalledWith(
          expect.objectContaining({ status: "completed" }),
        ),
      );
      expect(confirms).toHaveBeenCalledTimes(1);
      expect(queries).toHaveBeenCalledTimes(1);
      // The actual dialog, not App's callback, decides whether its draft/session can close.
      expect(screen.getByTestId("edit-provider-dialog")).toBeInTheDocument();
    } finally {
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });
});
