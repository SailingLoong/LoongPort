import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  listRelays: vi.fn(),
  listTierRates: vi.fn(),
  checkSession: vi.fn(),
  refreshAll: vi.fn(),
  refresh: vi.fn(),
  removeSite: vi.fn(),
  status: vi.fn(),
  listSites: vi.fn(),
  listSummaries: vi.fn(),
  listHistory: vi.fn(),
  listModels: vi.fn(),
  start: vi.fn(),
  cancel: vi.fn(),
  onProgress: vi.fn(),
  list: vi.fn(),
  openLogin: vi.fn(),
  vendorRefresh: vi.fn(),
}));
const dialogState = vi.hoisted(() => ({
  onOpenChange: (_open: boolean) => {},
}));
const selectState = vi.hoisted(() => ({
  onValueChange: (_value: string) => {},
}));
const eventHandlers = vi.hoisted(
  () => new Map<string, (payload: any) => void>(),
);

vi.mock("@/lib/api", () => ({
  relayApi: api,
  PURCHASE_CLOSED: "purchase-closed",
  VENDOR_LOGIN_ERROR: "vendor-login-error",
  VENDOR_ACCOUNTS_CHANGED: "vendor-accounts-changed",
  PROVIDER_SWITCHED: "provider-switched",
  SITE_BALANCES_UPDATED: "site-balances-updated",
}));
vi.mock("@/lib/api/relay", () => ({ relayApi: api }));
vi.mock("@/lib/api/vendor", () => ({
  DEEPSEEK_VENDOR_ID: "deepseek",
  vendorApi: {
    list: api.list,
    openLogin: api.openLogin,
    refresh: api.vendorRefresh,
  },
}));
vi.mock("@/lib/api/modelVerification", () => ({
  modelVerificationApi: {
    listSummaries: api.listSummaries,
    listHistory: api.listHistory,
    listModels: api.listModels,
    start: api.start,
    cancel: api.cancel,
    onProgress: api.onProgress,
  },
}));
vi.mock("@/components/ui/dialog", () => ({
  Dialog: ({ open, onOpenChange, children }: any) => {
    dialogState.onOpenChange = onOpenChange;
    return open ? children : null;
  },
  DialogContent: ({ children }: any) => (
    <div role="dialog">
      {children}
      <button type="button" onClick={() => dialogState.onOpenChange(false)}>
        close verification dialog
      </button>
    </div>
  ),
  DialogDescription: ({ children }: any) => <p>{children}</p>,
  DialogFooter: ({ children }: any) => <div>{children}</div>,
  DialogHeader: ({ children }: any) => <div>{children}</div>,
  DialogTitle: ({ children }: any) => <h2>{children}</h2>,
}));
vi.mock("@/components/ui/select", () => ({
  Select: ({ onValueChange, children }: any) => {
    selectState.onValueChange = onValueChange;
    return <div>{children}</div>;
  },
  SelectTrigger: ({ children }: any) => (
    <button type="button" role="combobox">
      {children}
    </button>
  ),
  SelectValue: ({ placeholder }: any) => <span>{placeholder}</span>,
  SelectContent: ({ children }: any) => <div>{children}</div>,
  SelectItem: ({ value, children }: any) => (
    <button
      type="button"
      role="option"
      onClick={() => selectState.onValueChange(value)}
    >
      {children}
    </button>
  ),
}));
vi.mock("@/hooks/useStreamCheck", () => ({
  useStreamCheck: () => ({ checkProvider: vi.fn(), isChecking: () => false }),
}));
// 这里测的是模块启用时的宿主接线；「下线即全部不展示」的契约在
// model-verification/__tests__/offline.test.tsx 单独钉。
vi.mock("../model-verification/availability", () => ({
  MODEL_VERIFICATION_ENABLED: true,
}));
vi.mock("@/hooks/useTauriEvent", () => ({
  useTauriEvent: (event: string, handler: (payload: any) => void) => {
    eventHandlers.set(event, handler);
  },
}));
vi.mock("../useRowBusy", () => ({
  useRowBusy: () => ({
    busy: new Set(),
    isBusy: () => false,
    run: async (_key: string, callback: () => Promise<void>) => callback(),
  }),
}));
vi.mock("../useTierEditGuard", () => ({
  useTierEditGuard: () => ({ requestEdit: vi.fn(), editDialogs: null }),
}));
// RelayTierList 被 mock 掉后，验真的行内呈现改由模块自己的 context 驱动
// （与真实 RelayRow 的消费方式一致），stub 只做每档位一行的最小呈现。
vi.mock("@/components/relay/RelayTierList", async () => {
  const { useTierVerification } =
    await import("../model-verification/TierVerificationProvider");
  return {
    RelayTierList: (props: any) => {
      const { verdictFor, openVerification, isVerifying } =
        useTierVerification();
      return (
        <div>
          {props.relays.flatMap((relay: any) =>
            relay.tiers.map((tier: any) => (
              <div key={tier.providerId}>
                <span data-testid={`verdict-${tier.providerId}`}>
                  {verdictFor(tier.providerId) ?? "none"}
                </span>
                {tier.canVerifyModels && (
                  <button type="button" onClick={() => openVerification(tier)}>
                    {isVerifying(tier.providerId)
                      ? `reopen ${tier.providerId}`
                      : `verify ${tier.providerId}`}
                  </button>
                )}
              </div>
            )),
          )}
        </div>
      );
    },
  };
});
vi.mock("@/components/relay/ImageTabNotice", () => ({
  ImageTabNotice: () => null,
}));
vi.mock("@/components/relay/VendorBlock", () => ({
  VendorBlock: ({ vendor }: any) => (
    <output data-testid="vendor-labels">
      {vendor.accounts.map((account: any) => account.accountLabel).join(",")}
    </output>
  ),
}));
vi.mock("@/components/ConfirmDialog", () => ({
  ConfirmDialog: ({ isOpen, message }: any) =>
    isOpen ? <div role="alertdialog">{message}</div> : null,
}));
vi.mock("../SwitchTierConfirmDialog", () => ({
  SwitchTierConfirmDialog: () => null,
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: any) => options?.defaultValue ?? key,
    i18n: { language: "en" },
  }),
}));

import { createTestQueryClient } from "../../../../tests/utils/testQueryClient";

import { RelaySection } from "../RelaySection";
import { ServicesPage } from "../accounts/ServicesPage";
import { useAccountSessionStartup } from "../accounts/useAccountSessionStartup";
import { APP_IDS } from "@/config/appConfig";

/**
 * 各行的余额走 react-query（`useRowBalanceQuery`）⇒ 这一层要有 provider。
 * 余额不是这些闸关心的东西 —— `relayApi.balance` 没被 mock，那个 query 会 reject，
 * 行上渲染一个失败态的用量条，不影响任何模型验证的断言。
 */
function renderSection(appId: "codex" | "claude" | "codex-image" | "gemini") {
  const queryClient = createTestQueryClient();
  return render(
    <QueryClientProvider client={queryClient}>
      <RelaySection appId={appId} onOpenAddHub={vi.fn()} />
    </QueryClientProvider>,
  );
}

const tier = (
  providerId: string,
  appId: "codex" | "claude" | "gemini" | "codex-image" = "codex",
) => ({
  providerId,
  appId,
  groupName: providerId,
  displayName: providerId,
  model: "gpt-5",
  models: ["gpt-5"],
  rateMultiplier: null,
  isCurrent: false,
  canVerifyModels: appId === "codex" || appId === "claude",
  userEdited: false,
  allowImageGeneration: false,
  siteDeclaredOrigin: null,
});
const relay = {
  id: 1,
  siteOrigin: "https://relay.example",
  siteName: "Relay",
  accountLabel: "account",
  status: "ready" as const,
  isCurrent: false,
  canQueryBalance: true,
  canRefresh: true,
  canPurchase: false,
  canViewUsage: false,
  usageBlockers: [],
  removeConfirmation: "configured" as const,
  tiers: [tier("provider-a")],
};
const emptyRefreshResult = {
  summary: {
    notice: "none" as const,
    refreshedAccounts: 0,
    tiers: 0,
    keysCreated: 0,
    otherPlatformTiers: 0,
    mergedProviders: 0,
    failures: [],
  },
  balances: [],
};
const report = (providerId: string, verdict: string, model = "gpt-5") => ({
  target: { providerId, appType: "codex", model },
  verdict,
  evidenceLevel: "protocolBehavior",
  facts: [],
  rulesVersion: 1,
  checkedAt: 1,
});

const summary = (providerId: string, verdict: string, model = "gpt-5") => ({
  providerId,
  appType: "codex",
  badgeVerdict: verdict === "inconclusive" ? null : verdict,
  representativeReport: report(providerId, verdict, model),
});

let progressListener: ((event: any) => void) | undefined;

describe("RelaySection model verification ownership", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    eventHandlers.clear();
    progressListener = undefined;
    // 每次调用返回新数组引用：Provider 的 summaries 拉取由 providerIds 集合
    // 的身份变化驱动，而真实后端每次响应都是新对象；固定引用会让 mock
    // 与生产行为分叉（刷新后不重拉）。
    api.listRelays.mockImplementation(async () => [relay]);
    api.listTierRates.mockResolvedValue([]);
    api.checkSession.mockResolvedValue([]);
    api.refreshAll.mockResolvedValue(emptyRefreshResult);
    api.refresh.mockResolvedValue(emptyRefreshResult);
    api.status.mockResolvedValue({
      defaultSite: "",
      shouldPromptAddSite: false,
    });
    api.listSites.mockResolvedValue([{}]);
    api.list.mockResolvedValue({ supported: false, accounts: [] });
    api.openLogin.mockResolvedValue({ rowId: 9, refresh: emptyRefreshResult });
    api.vendorRefresh.mockResolvedValue(emptyRefreshResult);
    api.listSummaries.mockResolvedValue([
      summary("provider-a", "anomaly", "two"),
    ]);
    api.listHistory.mockResolvedValue([]);
    api.listModels.mockResolvedValue([
      { name: "gpt-5", fitness: "unknown" as const },
    ]);
    api.start.mockResolvedValue({ runId: "run-1", state: "running" });
    api.cancel.mockResolvedValue(undefined);
    api.onProgress.mockImplementation(
      async (listener: (event: any) => void) => {
        progressListener = listener;
        return () => {};
      },
    );
  });

  it("does not probe or configure accounts on mount", async () => {
    renderSection("codex");
    await screen.findByTestId("verdict-provider-a");
    expect(api.checkSession).not.toHaveBeenCalled();
    expect(api.refresh).not.toHaveBeenCalled();
  });

  it("configures the selected account only after an explicit click and warns about cross-app deletion", async () => {
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <RelaySection
          appId="codex"
          accountFilter={{ kind: "relay", id: 1 }}
          onOpenAddHub={vi.fn()}
        />
      </QueryClientProvider>,
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "One-click configuration" }),
    );
    await waitFor(() => expect(api.refresh).toHaveBeenCalledWith(1, "codex"));
    fireEvent.click(screen.getByRole("button", { name: "common.delete" }));
    expect(screen.getByRole("alertdialog")).toHaveTextContent(
      "all applications",
    );
    expect(screen.getByRole("alertdialog")).toHaveTextContent(
      "all its groups and tiers",
    );
    expect(screen.getByRole("alertdialog")).not.toHaveTextContent("1 tier");
  });

  it.each([false, true])(
    "refreshes mounted account action availability after startup expires a session (detail=%s)",
    async (detail) => {
      let finishProbe!: (ids: number[]) => void;
      api.checkSession.mockImplementation(
        () =>
          new Promise<number[]>((resolve) => {
            finishProbe = resolve;
          }),
      );
      api.listRelays.mockResolvedValue([{ ...relay, canQueryBalance: false }]);
      function Startup() {
        useAccountSessionStartup();
        return null;
      }
      render(
        <QueryClientProvider client={createTestQueryClient()}>
          <Startup />
          <ServicesPage
            appId="codex"
            onOpenAddHub={vi.fn()}
            onOpenApp={vi.fn()}
            account={detail ? { kind: "relay", id: 1 } : undefined}
            onSelectAccount={vi.fn()}
          />
        </QueryClientProvider>,
      );
      expect(
        await screen.findByRole("button", { name: "One-click configuration" }),
      ).toBeEnabled();
      api.listRelays.mockResolvedValue([
        {
          ...relay,
          status: "notLoggedIn",
          canRefresh: false,
          canQueryBalance: false,
        },
      ]);
      await act(async () => finishProbe([1]));
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "One-click configuration" }),
        ).toBeDisabled(),
      );
      expect(
        screen.getByRole("button", { name: "loongport.row.login" }),
      ).toBeInTheDocument();
      expect(api.checkSession).toHaveBeenCalledTimes(1);
    },
  );

  it("mounts one real lifecycle owner for a multi-account ServicesPage", async () => {
    api.listRelays.mockResolvedValue([
      { ...relay, canQueryBalance: false },
      { ...relay, id: 2, siteName: "Second relay", canQueryBalance: false },
    ]);
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <ServicesPage
          appId="codex"
          onOpenAddHub={vi.fn()}
          onOpenApp={vi.fn()}
        />
      </QueryClientProvider>,
    );
    expect(
      await screen.findAllByRole("button", { name: "One-click configuration" }),
    ).toHaveLength(2);
    // The public account query is the only snapshot owner.
    expect(api.listRelays).toHaveBeenCalledTimes(APP_IDS.length);
    expect(api.list).toHaveBeenCalledTimes(APP_IDS.length);
    expect(api.checkSession).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getAllByRole("button", { name: "One-click configuration" })[1],
    );
    await waitFor(() => expect(api.refresh).toHaveBeenCalledWith(2, "codex"));
    await waitFor(() =>
      expect(api.listRelays).toHaveBeenCalledTimes(APP_IDS.length * 2),
    );
    expect(api.list).toHaveBeenCalledTimes(APP_IDS.length * 2);
  });

  it("returns vendor login refresh to the public account owner", async () => {
    const changed = vi.fn();
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <RelaySection
          appId="codex"
          accountSnapshot={null}
          onAccountChanged={changed}
          onOpenAddHub={vi.fn()}
          renderAccounts={(renderActions) =>
            renderActions({
              kind: "vendor",
              appId: "claude",
              row: {
                id: 9,
                vendorId: "example",
                vendorName: "Example",
                accountLabel: "account",
                status: "ready",
                canQueryBalance: false,
                canRefresh: true,
                canDelete: true,
                plans: [],
              },
            })
          }
        />
      </QueryClientProvider>,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "loongport.row.reLogin" }),
    );
    await waitFor(() => expect(changed).toHaveBeenCalledTimes(1));
    expect(api.openLogin).toHaveBeenCalledWith("example", "claude");
    expect(api.list).not.toHaveBeenCalled();
    expect(api.listRelays).not.toHaveBeenCalled();
  });

  it("shares one controller across overview cards and uses each card's application", async () => {
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <RelaySection
          appId="codex"
          onOpenAddHub={vi.fn()}
          renderAccounts={(renderActions) => (
            <>
              {renderActions({ kind: "relay", row: relay, appId: "claude" })}
              {renderActions({
                kind: "relay",
                row: { ...relay, id: 2 },
                appId: "codex",
              })}
            </>
          )}
        />
      </QueryClientProvider>,
    );
    const buttons = await screen.findAllByRole("button", {
      name: "One-click configuration",
    });
    expect(api.listRelays).toHaveBeenCalledTimes(1);
    expect(api.list).toHaveBeenCalledTimes(1);
    expect(api.refresh).not.toHaveBeenCalled();
    fireEvent.click(buttons[0]);
    await waitFor(() => expect(api.refresh).toHaveBeenCalledWith(1, "claude"));
    fireEvent.click(buttons[1]);
    await waitFor(() => expect(api.refresh).toHaveBeenCalledWith(2, "codex"));
  });

  it("explains official sign-in and restart consequences for an in-use account", async () => {
    api.listRelays.mockResolvedValue([
      { ...relay, usageBlockers: [{ app: "claude", tierName: "Standard" }] },
    ]);
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <RelaySection
          appId="codex"
          accountFilter={{ kind: "relay", id: 1 }}
          onOpenAddHub={vi.fn()}
        />
      </QueryClientProvider>,
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "common.delete" }),
    );
    expect(screen.getByRole("alertdialog")).toHaveTextContent(
      "official sign-in",
    );
    expect(screen.getByRole("alertdialog")).toHaveTextContent("restart");
    expect(screen.getByRole("alertdialog")).toHaveTextContent("sign in again");
  });

  it("explains that deleting a vendor account retains the remote API key", async () => {
    api.list.mockResolvedValue({
      supported: true,
      accounts: [
        {
          id: 9,
          vendorId: "example",
          vendorName: "Example",
          status: "ready",
          canDelete: true,
          plans: [],
        },
      ],
    });
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <RelaySection
          appId="codex"
          accountFilter={{ kind: "vendor", id: 9 }}
          onOpenAddHub={vi.fn()}
        />
      </QueryClientProvider>,
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "common.delete" }),
    );
    expect(screen.getByRole("alertdialog")).toHaveTextContent(
      "API key on the provider website is not deleted",
    );
  });

  it("reloads repaired model inventory only for the visible app", async () => {
    renderSection("codex");
    await waitFor(() => expect(api.listRelays).toHaveBeenCalledTimes(1));
    act(() =>
      eventHandlers.get("provider-models-updated")?.({ appType: "claude" }),
    );
    expect(api.listRelays).toHaveBeenCalledTimes(1);
    act(() =>
      eventHandlers.get("provider-models-updated")?.({ appType: "codex" }),
    );
    await waitFor(() => expect(api.listRelays).toHaveBeenCalledTimes(2));
  });

  it("renders backend summaries from initial and refreshed relay fetches", async () => {
    renderSection("codex");
    await waitFor(() =>
      expect(api.listSummaries).toHaveBeenCalledWith(["provider-a"], "codex"),
    );
    expect(screen.getByTestId("verdict-provider-a")).toHaveTextContent(
      "anomaly",
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "loongport.refreshAll" }),
    );
    await waitFor(() => expect(api.listRelays).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(api.listSummaries).toHaveBeenCalledTimes(2));

    fireEvent.click(screen.getByRole("button", { name: "verify provider-a" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await screen.findByRole("combobox");
  });

  it("keeps a trusted tier visible until a more severe report supersedes it", async () => {
    api.listSummaries.mockResolvedValueOnce([
      summary("provider-a", "trusted", "verified-model"),
    ]);

    renderSection("codex");

    await waitFor(() =>
      expect(screen.getByTestId("verdict-provider-a")).toHaveTextContent(
        "trusted",
      ),
    );

    api.listSummaries.mockResolvedValueOnce([
      summary("provider-a", "suspicious", "suspicious-model"),
    ]);
    fireEvent.click(
      screen.getByRole("button", { name: "loongport.refreshAll" }),
    );

    await waitFor(() =>
      expect(screen.getByTestId("verdict-provider-a")).toHaveTextContent(
        "suspicious",
      ),
    );
  });

  it("clears a reset badge only after the matching backend change event", async () => {
    renderSection("codex");
    await waitFor(() =>
      expect(screen.getByTestId("verdict-provider-a")).toHaveTextContent(
        "anomaly",
      ),
    );
    api.listSummaries.mockResolvedValueOnce([]);
    eventHandlers.get("model-verification-changed")?.({
      providerId: "provider-a",
      appType: "codex",
    });
    await waitFor(() =>
      expect(screen.getByTestId("verdict-provider-a")).toHaveTextContent(
        "none",
      ),
    );
  });

  it("keeps one real run owner through close, terminal completion, and reopen", async () => {
    renderSection("codex");
    await waitFor(() =>
      screen.getByRole("button", { name: "verify provider-a" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "verify provider-a" }));
    expect(
      await screen.findByText("loongport.modelVerification.verdict.anomaly"),
    ).toBeInTheDocument();
    expect(api.start).not.toHaveBeenCalled();

    fireEvent.click(await screen.findByRole("option", { name: "gpt-5" }));
    fireEvent.click(
      screen.getByRole("button", {
        name: "loongport.modelVerification.actions.start",
      }),
    );
    await waitFor(() => expect(api.start).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "reopen provider-a" }),
      ).toBeInTheDocument(),
    );

    fireEvent.click(
      screen.getByRole("button", { name: "close verification dialog" }),
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    api.listSummaries.mockResolvedValueOnce([summary("provider-a", "trusted")]);
    await act(async () => {
      progressListener?.({
        runId: "run-1",
        providerId: "provider-a",
        appType: "codex",
        model: "gpt-5",
        state: "completed",
        completedChecks: 4,
        totalChecks: 4,
        failure: null,
      });
      eventHandlers.get("model-verification-changed")?.({
        providerId: "provider-a",
        appType: "codex",
      });
    });

    fireEvent.click(
      await screen.findByRole("button", { name: "verify provider-a" }),
    );
    expect(
      await screen.findByText("loongport.modelVerification.verdict.trusted"),
    ).toBeInTheDocument();
    expect(api.start).toHaveBeenCalledTimes(1);
  });

  it("keeps the prior persisted report visible when a rerun fails", async () => {
    renderSection("codex");
    await waitFor(() =>
      screen.getByRole("button", { name: "verify provider-a" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "verify provider-a" }));
    expect(
      await screen.findByText("loongport.modelVerification.verdict.anomaly"),
    ).toBeInTheDocument();

    fireEvent.click(await screen.findByRole("option", { name: "gpt-5" }));
    fireEvent.click(
      screen.getByRole("button", {
        name: "loongport.modelVerification.actions.start",
      }),
    );
    await waitFor(() => expect(api.start).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(progressListener).toBeDefined());

    act(() => {
      progressListener?.({
        runId: "run-1",
        providerId: "provider-a",
        appType: "codex",
        model: "gpt-5",
        state: "failed",
        completedChecks: 1,
        totalChecks: 4,
        failure: "authentication",
      });
    });

    expect(
      screen.getByText("loongport.modelVerification.verdict.anomaly"),
    ).toBeInTheDocument();
  });

  it("reopens with the persisted highest-severity model after another model completes", async () => {
    api.listSummaries.mockResolvedValue([
      summary("provider-a", "anomaly", "model-b"),
    ]);
    api.listModels.mockResolvedValue([
      { name: "model-a", fitness: "unknown" as const },
    ]);

    renderSection("codex");
    await waitFor(() =>
      screen.getByRole("button", { name: "verify provider-a" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "verify provider-a" }));
    expect(
      await screen.findByText("loongport.modelVerification.verdict.anomaly"),
    ).toBeInTheDocument();

    fireEvent.click(await screen.findByRole("option", { name: "model-a" }));
    fireEvent.click(
      screen.getByRole("button", {
        name: "loongport.modelVerification.actions.start",
      }),
    );
    await waitFor(() => expect(api.start).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(progressListener).toBeDefined());
    fireEvent.click(
      screen.getByRole("button", { name: "close verification dialog" }),
    );

    api.listSummaries.mockResolvedValue([
      summary("provider-a", "anomaly", "model-b"),
    ]);
    await act(async () => {
      progressListener?.({
        runId: "run-1",
        providerId: "provider-a",
        appType: "codex",
        model: "model-a",
        state: "completed",
        completedChecks: 4,
        totalChecks: 4,
        failure: null,
      });
      eventHandlers.get("model-verification-changed")?.({
        providerId: "provider-a",
        appType: "codex",
      });
    });
    await waitFor(() =>
      expect(api.listSummaries.mock.calls.length).toBeGreaterThan(1),
    );

    fireEvent.click(
      await screen.findByRole("button", { name: "verify provider-a" }),
    );
    expect(
      await screen.findByText("loongport.modelVerification.verdict.anomaly"),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("loongport.modelVerification.verdict.trusted"),
    ).not.toBeInTheDocument();
  });

  it.each([
    ["claude", true],
    ["codex-image", false],
    ["gemini", false],
  ] as const)("gates verification for %s tiers", async (appId, eligible) => {
    api.listRelays.mockResolvedValue([
      { ...relay, tiers: [tier("provider-a", appId)] },
    ]);
    renderSection(appId);
    await waitFor(() => screen.getByTestId("verdict-provider-a"));
    if (eligible) {
      expect(
        screen.queryByRole("button", { name: "verify provider-a" }),
      ).toBeInTheDocument();
    } else {
      expect(
        screen.queryByRole("button", { name: "verify provider-a" }),
      ).not.toBeInTheDocument();
    }
  });

  it("refreshes relays and official APIs from one page-level icon", async () => {
    api.list.mockResolvedValue({
      supported: true,
      accounts: [
        {
          id: 9,
          vendorId: "deepseek",
          vendorName: "DeepSeek",
          accountLabel: "account",
          status: "ready",
          canQueryBalance: true,
          canRefresh: true,
          canEditConfig: true,
          canSwitch: true,
          canDelete: true,
          providerId: "vendor-provider",
          isCurrent: false,
          userEdited: false,
        },
      ],
    });
    const queryClient = createTestQueryClient();
    const invalidateQueries = vi.spyOn(queryClient, "invalidateQueries");
    render(
      <QueryClientProvider client={queryClient}>
        <RelaySection appId="codex" onOpenAddHub={vi.fn()} />
      </QueryClientProvider>,
    );

    await waitFor(() => expect(api.list).toHaveBeenCalled());
    fireEvent.click(
      screen.getByRole("button", { name: "loongport.refreshAll" }),
    );

    await waitFor(() => expect(api.refreshAll).toHaveBeenCalledWith("codex"));
    expect(api.vendorRefresh).not.toHaveBeenCalled();
    expect(invalidateQueries).not.toHaveBeenCalledWith({
      queryKey: ["rowBalance"],
    });
    expect(screen.queryByText("loongport.refreshAll")).not.toBeInTheDocument();
  });

  it("keeps the newest backend vendor view when an older reload finishes late", async () => {
    let resolveOlder!: (value: any) => void;
    let resolveNewer!: (value: any) => void;
    const older = new Promise((resolve) => {
      resolveOlder = resolve;
    });
    const newer = new Promise((resolve) => {
      resolveNewer = resolve;
    });
    const account = (accountLabel: string) => ({
      id: 9,
      vendorId: "deepseek",
      vendorName: "DeepSeek",
      accountLabel,
      status: "ready",
      canQueryBalance: true,
      canRefresh: true,
      canEditConfig: true,
      canSwitch: true,
      canDelete: true,
      providerId: "vendor-provider",
      isCurrent: accountLabel === "new current",
      userEdited: false,
    });
    api.list
      .mockImplementationOnce(() => older)
      .mockImplementationOnce(() => newer);

    renderSection("codex");
    await waitFor(() => expect(api.list).toHaveBeenCalledTimes(1));

    eventHandlers.get("provider-switched")?.({ appType: "codex" });
    await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));

    resolveNewer({ supported: true, accounts: [account("new current")] });
    expect(await screen.findByTestId("vendor-labels")).toHaveTextContent(
      "new current",
    );

    resolveOlder({ supported: true, accounts: [account("stale current")] });
    await act(async () => {});
    expect(screen.getByTestId("vendor-labels")).toHaveTextContent(
      "new current",
    );
  });
});
