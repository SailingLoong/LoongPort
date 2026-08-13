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
  provision: vi.fn(),
  status: vi.fn(),
  listSites: vi.fn(),
  listResults: vi.fn(),
  listHistory: vi.fn(),
  listModels: vi.fn(),
  start: vi.fn(),
  cancel: vi.fn(),
  onProgress: vi.fn(),
  list: vi.fn(),
  vendorProvision: vi.fn(),
}));
const vendorSupport = vi.hoisted(() => ({ enabled: false }));
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
  PROVIDER_SWITCHED: "provider-switched",
}));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: { list: api.list, provision: api.vendorProvision },
  vendorSupportsApp: () => vendorSupport.enabled,
}));
vi.mock("@/lib/api/modelVerification", () => ({
  modelVerificationApi: {
    listResults: api.listResults,
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
vi.mock("@/components/relay/RelayTierList", () => ({
  RelayTierList: (props: any) => (
    <div>
      {props.relays.flatMap((relay: any) =>
        relay.tiers.map((tier: any) => (
          <div key={tier.providerId}>
            <span data-testid={`verdict-${tier.providerId}`}>
              {props.verificationVerdictForTier(tier) ?? "none"}
            </span>
            {props.onVerifyTier && (
              <button type="button" onClick={() => props.onVerifyTier(tier)}>
                {props.isVerifyingTier(tier.providerId)
                  ? `reopen ${tier.providerId}`
                  : `verify ${tier.providerId}`}
              </button>
            )}
          </div>
        )),
      )}
    </div>
  ),
}));
vi.mock("@/components/relay/AddSiteDialog", () => ({
  AddSiteDialog: () => null,
}));
vi.mock("@/components/relay/ImageTabNotice", () => ({
  ImageTabNotice: () => null,
}));
vi.mock("@/components/relay/VendorBlock", () => ({ VendorBlock: () => null }));
vi.mock("@/components/ConfirmDialog", () => ({ ConfirmDialog: () => null }));
vi.mock("../SwitchTierConfirmDialog", () => ({
  SwitchTierConfirmDialog: () => null,
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

import { createTestQueryClient } from "../../../../tests/utils/testQueryClient";

import { RelaySection } from "../RelaySection";

/**
 * 各行的余额走 react-query（`useRowBalanceQuery`）⇒ 这一层要有 provider。
 * 余额不是这些闸关心的东西 —— `relayApi.balance` 没被 mock，那个 query 会 reject，
 * 行上渲染一个失败态的用量条，不影响任何模型验证的断言。
 */
function renderSection(appId: "codex" | "claude" | "codex-image" | "gemini") {
  const queryClient = createTestQueryClient();
  return render(
    <QueryClientProvider client={queryClient}>
      <RelaySection appId={appId} />
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
  rateMultiplier: null,
  isCurrent: false,
  userEdited: false,
  allowImageGeneration: false,
});
const relay = {
  id: 1,
  siteOrigin: "https://relay.example",
  siteName: "Relay",
  accountLabel: "account",
  loggedIn: true,
  sessionExpired: false,
  tiers: [tier("provider-a")],
};
const report = (providerId: string, verdict: string, model = "gpt-5") => ({
  target: { providerId, appType: "codex", model },
  verdict,
  evidenceLevel: "protocolBehavior",
  facts: [],
  rulesVersion: 1,
  checkedAt: 1,
});

let progressListener: ((event: any) => void) | undefined;

describe("RelaySection model verification ownership", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vendorSupport.enabled = false;
    eventHandlers.clear();
    progressListener = undefined;
    api.listRelays.mockResolvedValue([relay]);
    api.listTierRates.mockResolvedValue([]);
    api.checkSession.mockResolvedValue([]);
    api.provision.mockResolvedValue({
      tiers: [],
      failures: [],
      keysCreated: 0,
      mergedProviders: [],
    });
    api.status.mockResolvedValue({
      defaultSite: "",
      chatgptNeedsAttention: false,
    });
    api.listSites.mockResolvedValue([{}]);
    api.list.mockResolvedValue([]);
    api.vendorProvision.mockResolvedValue({
      providerId: "vendor-provider",
      keyCreated: false,
      platforms: ["codex"],
      mergedProviders: [],
    });
    api.listResults.mockResolvedValue([
      report("provider-a", "suspicious", "one"),
      report("provider-a", "anomaly", "two"),
    ]);
    api.listHistory.mockResolvedValue([]);
    api.listModels.mockResolvedValue(["gpt-5"]);
    api.start.mockResolvedValue({ runId: "run-1", state: "running" });
    api.cancel.mockResolvedValue(undefined);
    api.onProgress.mockImplementation(
      async (listener: (event: any) => void) => {
        progressListener = listener;
        return () => {};
      },
    );
  });

  it("reduces reports from the initial and refreshed relay fetch, with one dialog owner", async () => {
    renderSection("codex");
    await waitFor(() =>
      expect(api.listResults).toHaveBeenCalledWith(["provider-a"]),
    );
    expect(screen.getByTestId("verdict-provider-a")).toHaveTextContent(
      "anomaly",
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "loongport.refreshAll" }),
    );
    await waitFor(() => expect(api.listRelays).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(api.listResults).toHaveBeenCalledTimes(2));

    fireEvent.click(screen.getByRole("button", { name: "verify provider-a" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await screen.findByRole("combobox");
  });

  it("keeps a trusted tier visible until a more severe report supersedes it", async () => {
    api.listResults.mockResolvedValueOnce([
      report("provider-a", "trusted", "verified-model"),
    ]);

    renderSection("codex");

    await waitFor(() =>
      expect(screen.getByTestId("verdict-provider-a")).toHaveTextContent(
        "trusted",
      ),
    );

    api.listResults.mockResolvedValueOnce([
      report("provider-a", "trusted", "verified-model"),
      report("provider-a", "suspicious", "suspicious-model"),
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
    api.listResults.mockResolvedValueOnce([]);
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

    api.listResults.mockResolvedValueOnce([report("provider-a", "trusted")]);
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
    api.listResults.mockResolvedValue([
      report("provider-a", "suspicious", "model-a"),
      report("provider-a", "anomaly", "model-b"),
    ]);
    api.listModels.mockResolvedValue(["model-a"]);

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

    api.listResults.mockResolvedValue([
      report("provider-a", "trusted", "model-a"),
      report("provider-a", "anomaly", "model-b"),
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
      expect(api.listResults.mock.calls.length).toBeGreaterThan(1),
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
    vendorSupport.enabled = true;
    api.list.mockResolvedValue([
      {
        id: 9,
        vendorId: "deepseek",
        vendorName: "DeepSeek",
        accountLabel: "account",
        loggedIn: true,
        sessionExpired: false,
        keyReady: true,
        providerId: "vendor-provider",
        isCurrent: false,
        userEdited: false,
      },
    ]);
    const queryClient = createTestQueryClient();
    const invalidateQueries = vi.spyOn(queryClient, "invalidateQueries");
    render(
      <QueryClientProvider client={queryClient}>
        <RelaySection appId="codex" />
      </QueryClientProvider>,
    );

    await waitFor(() => expect(api.list).toHaveBeenCalled());
    fireEvent.click(
      screen.getByRole("button", { name: "loongport.refreshAll" }),
    );

    await waitFor(() => expect(api.provision).toHaveBeenCalledWith(1));
    expect(api.vendorProvision).toHaveBeenCalledWith(9);
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: ["rowBalance"],
    });
    expect(screen.queryByText("loongport.refreshAll")).not.toBeInTheDocument();
  });
});
