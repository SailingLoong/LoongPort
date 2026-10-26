import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { ApplicationRouteWorkspace } from "../ApplicationRouteWorkspace";
import {
  sharedRoutingApi,
  type SharedRoutingState,
} from "@/lib/api/sharedRouting";
import {
  serviceSourcesApi,
  type ServiceSource,
} from "@/lib/api/serviceSources";
import { applicationOverviewApi } from "@/lib/api/applicationOverview";
import { applicationModelSettingsApi } from "@/lib/api/applicationModelSettings";
import { failoverApi } from "@/lib/api/failover";
import { autoModeApi } from "@/lib/api/autoMode";
import { startupUpgradeApi } from "@/lib/api/startupUpgrade";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
vi.mock("@/lib/api/sharedRouting", () => ({
  sharedRoutingApi: {
    get: vi.fn(),
    prepareTarget: vi.fn(),
    applyTarget: vi.fn(),
    queryTarget: vi.fn(),
    recoverTarget: vi.fn(),
  },
}));
vi.mock("@/lib/api/serviceSources", () => ({
  serviceSourcesApi: { list: vi.fn() },
}));
vi.mock("@/lib/api/applicationRouting", async (original) => ({
  ...(await original<typeof import("@/lib/api/applicationRouting")>()),
  applicationRoutingApi: { get: vi.fn(), setTierBlocked: vi.fn() },
}));
vi.mock("@/lib/api/startupUpgrade", () => ({
  startupUpgradeApi: Object.fromEntries(
    [
      "query",
      "queryApp",
      "prepare",
      "publish",
      "complete",
      "continueRuntime",
      "recoverApp",
      "selectProvider",
      "selectMode",
    ].map((name) => [name, vi.fn()]),
  ),
}));
vi.mock("@/lib/api/applicationOverview", () => ({
  applicationOverviewApi: { get: vi.fn() },
}));
vi.mock("@/lib/api/applicationModelSettings", async (original) => ({
  ...(await original<typeof import("@/lib/api/applicationModelSettings")>()),
  applicationModelSettingsApi: {
    get: vi.fn(),
    preview: vi.fn(),
    confirm: vi.fn(),
    query: vi.fn(),
  },
}));
vi.mock("@/lib/api/autoMode", () => ({
  autoModeApi: { getStatus: vi.fn(), setStrategy: vi.fn() },
}));
vi.mock("@/lib/api/failover", () => ({
  failoverApi: { resetCircuitBreaker: vi.fn() },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, args?: Record<string, string>) =>
      args?.app ? `${key} ${args.app}` : key,
  }),
}));
const request = {
  id: "synthetic-receipt",
  scope: "applicationRouting" as const,
  draftDigest: "digest",
  revision: "revision",
};
let state: SharedRoutingState, sources: ServiceSource[];
beforeEach(() => {
  vi.clearAllMocks();
  state = {
    appId: "codex",
    binding: {
      revision: 1,
      target: {
        kind: "manual",
        sourceId: "missing-saved",
        model: "saved-model",
      },
    },
    groups: [
      {
        id: "daily",
        name: "Daily group",
        revision: 2,
        members: [
          { sourceId: "missing-member", model: "original-first" },
          { sourceId: "source-b", model: "model-b" },
        ],
      },
    ],
    pendingRequest: null,
    canApply: true,
    blockedReason: null,
  };
  sources = [
    {
      sourceId: "source-b",
      name: "Known native account",
      serviceName: null,
      accountLabel: null,
      configurationName: null,
      account: {
        kind: "nativeAccount",
        platform: "workbuddy",
        id: "native-string-id",
      },
      bindings: [
        {
          appId: "codex",
          model: "model-b",
          providerId: "p",
          usageState: "enabled",
          canSelect: true,
        },
      ],
    },
  ];
  vi.mocked(sharedRoutingApi.get).mockImplementation(async () => state);
  vi.mocked(serviceSourcesApi.list).mockImplementation(async () => sources);
  vi.mocked(applicationRoutingApi.get).mockResolvedValue({
    modeState: {
      status: "ready",
      mode: "proxy",
      attached: true,
      currentProviderId: null,
      directProviderId: null,
      publicationStarted: false,
      canWrite: true,
      writeProtocol: "native",
      blockedReason: null,
      canRecheck: true,
    },
    routingActive: false,
    autoFailoverEnabled: false,
    model: null,
    modelOptions: [],
    chainIds: [],
    tiers: [],
  });
  vi.mocked(sharedRoutingApi.prepareTarget).mockImplementation(
    async (_, target) => ({
      target,
      request,
      expectedBindingRevision: 1,
      groupRevision: 2,
    }),
  );
  vi.mocked(sharedRoutingApi.applyTarget).mockImplementation(
    async (_, review) => {
      state = { ...state, binding: { revision: 2, target: review.target } };
      return { request, status: "completed" };
    },
  );
});
function mount() {
  const onOpenSource = vi.fn(),
    onManageGroups = vi.fn(),
    onAdd = vi.fn();
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ApplicationRouteWorkspace
        appId="codex"
        onOpenSource={onOpenSource}
        onManageGroups={onManageGroups}
        onAdd={onAdd}
      />
    </QueryClientProvider>,
  );
  return { onOpenSource, onManageGroups, onAdd };
}
it("shows saved identity even when inventory cannot resolve it, and never claims actual requests succeeded", async () => {
  mount();
  await screen.findAllByText("saved-model");
  expect(screen.getAllByText("missing-saved").length).toBeGreaterThan(0);
  expect(screen.getByTestId("actual-request-fact")).toHaveTextContent(
    "routingWorkspace.requestUnknown",
  );
  expect(screen.queryByText(/balance|multiplier/i)).not.toBeInTheDocument();
});
it("keeps group selection local, preserves original member order and requires native review then confirmation", async () => {
  const user = userEvent.setup();
  mount();
  await screen.findAllByText("saved-model");
  await user.click(screen.getByLabelText("routingWorkspace.ordered"));
  await user.selectOptions(
    screen.getByLabelText("routingWorkspace.group"),
    "daily",
  );
  const rows = within(screen.getByTestId("shared-member-list")).getAllByRole(
    "listitem",
  );
  expect(rows[0]).toHaveTextContent("missing-member");
  expect(rows[1]).toHaveTextContent("Known native account");
  expect(
    screen.getAllByText("routingWorkspace.eligibilityUnknown"),
  ).toHaveLength(2);
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
  await user.click(
    screen.getByRole("button", { name: "routingWorkspace.preview" }),
  );
  await screen.findByRole("dialog");
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
  await user.click(
    screen.getByRole("button", { name: "routingWorkspace.confirm" }),
  );
  await waitFor(() =>
    expect(sharedRoutingApi.applyTarget).toHaveBeenCalledTimes(1),
  );
  expect(screen.getByTestId("actual-request-fact")).toHaveTextContent(
    "routingWorkspace.requestUnknown",
  );
});
it("does not infer localProvider eligibility from legacy bindings", async () => {
  sources = [
    {
      ...sources[0],
      sourceId: "local",
      name: "No native target fact",
      account: {
        kind: "localProvider",
        platform: "openrouter",
        id: "local-id",
        originApp: "codex",
        originProviderId: "provider-id",
      },
    },
  ];
  mount();
  await screen.findAllByText("saved-model");
  expect(
    screen.queryByRole("option", { name: /No native target fact/ }),
  ).not.toBeInTheDocument();
  expect(
    screen.getByText("routingWorkspace.noSelectableTargets"),
  ).toBeInTheDocument();
});
it("navigates known native identity without casting it to a legacy numeric account", async () => {
  const { onOpenSource } = mount();
  const user = userEvent.setup();
  await screen.findAllByText("saved-model");
  await user.click(screen.getByLabelText("routingWorkspace.ordered"));
  await user.selectOptions(
    screen.getByLabelText("routingWorkspace.group"),
    "daily",
  );
  await user.click(
    screen.getByRole("button", { name: "routingWorkspace.manageSource" }),
  );
  expect(onOpenSource).toHaveBeenCalledExactlyOnceWith(sources[0].account);
});
it("shows failed inventory as an error instead of an empty account list and cannot preview", async () => {
  vi.mocked(serviceSourcesApi.list).mockRejectedValue(
    new Error("private backend detail"),
  );
  mount();
  await screen.findByRole("alert");
  expect(screen.getByRole("alert")).toHaveTextContent(
    "routingWorkspace.readFailed",
  );
  expect(screen.queryByText("private backend detail")).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "routingWorkspace.preview" }),
  ).toBeDisabled();
});
it("only exposes original journal recovery, and labels it configuration recovery rather than request retry", async () => {
  state = {
    ...state,
    pendingRequest: request,
    canApply: false,
    blockedReason: "routing_groups.recovery_required",
  };
  vi.mocked(sharedRoutingApi.recoverTarget).mockResolvedValue({
    request,
    status: "verificationRequired",
  });
  const user = userEvent.setup();
  mount();
  await user.click(
    await screen.findByRole("button", {
      name: "routingWorkspace.recoverOperation",
    }),
  );
  expect(sharedRoutingApi.recoverTarget).toHaveBeenCalledExactlyOnceWith(
    "codex",
    request,
  );
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
  expect(screen.getByTestId("actual-request-fact")).toHaveTextContent(
    "routingWorkspace.requestUnknown",
  );
});

it("keeps a saved but no-longer-selectable target and offers its original service page without inventing a skip reason", async () => {
  state.binding = {
    revision: 1,
    target: { kind: "manual", sourceId: "source-b", model: "model-b" },
  };
  sources[0].bindings[0].canSelect = false;
  const { onOpenSource } = mount();
  const user = userEvent.setup();
  await screen.findByText("routingWorkspace.savedTargetNotSelectable");
  expect(screen.getAllByText("Known native account").length).toBeGreaterThan(0);
  expect(
    screen.getByRole("button", { name: "routingWorkspace.preview" }),
  ).toBeDisabled();
  await user.click(
    screen.getByRole("button", { name: "routingWorkspace.manageSource" }),
  );
  expect(onOpenSource).toHaveBeenCalledExactlyOnceWith(sources[0].account);
});

it.each(["pending", "failed"] as const)(
  "keeps %s saved-state reads unknown rather than displaying an empty or saved configuration",
  async (outcome) => {
    if (outcome === "pending")
      vi.mocked(sharedRoutingApi.get).mockImplementation(
        () => new Promise(() => {}),
      );
    else
      vi.mocked(sharedRoutingApi.get).mockRejectedValue(
        new Error("synthetic state read failure"),
      );
    mount();
    await screen.findAllByText("routingWorkspace.savedStateUnknown");
    if (outcome === "failed") await screen.findByRole("alert");
    expect(
      screen.queryByText("routingWorkspace.noSavedTarget"),
    ).not.toBeInTheDocument();
    expect(screen.getByTestId("route-edit-state")).toHaveTextContent(
      "routingWorkspace.savedStateUnknown",
    );
    expect(
      screen.getByRole("button", { name: "routingWorkspace.preview" }),
    ).toBeDisabled();
  },
);

it("renders native binding:null as explicit unconfigured rather than a failed read", async () => {
  state.binding = null;
  mount();
  await screen.findAllByText("routingWorkspace.noSavedTarget");
  await waitFor(() =>
    expect(screen.getByTestId("route-edit-state")).toHaveTextContent(
      "routingWorkspace.unconfigured",
    ),
  );
  expect(
    screen.queryByText("routingWorkspace.savedStateUnknown"),
  ).not.toBeInTheDocument();
});

const checkpointReview = {
  status: "database_verified",
  checkpointPresent: true,
  checkpointId: "synthetic-checkpoint",
  reviewToken: "synthetic-owner-token",
  canAuthenticate: false,
  canCheckAndBackup: false,
  canStartUpgrade: false,
  canCompleteUpgrade: false,
  archiveRetained: true,
  syncPaused: true,
};
function checkpointFault() {
  state.canApply = false;
  state.blockedReason = "upgrade.checkpoint_pending";
  vi.mocked(applicationRoutingApi.get).mockRejectedValue({
    code: "mode.verification_required",
    modeState: {
      status: "unknown",
      mode: null,
      attached: null,
      currentProviderId: null,
      directProviderId: null,
      publicationStarted: false,
      canWrite: false,
      writeProtocol: null,
      blockedReason: "upgrade.checkpoint_pending",
      canRecheck: true,
    },
  });
  vi.mocked(startupUpgradeApi.query).mockResolvedValue(checkpointReview);
  vi.mocked(startupUpgradeApi.queryApp).mockImplementation(
    async (_, appType) => ({
      appType,
      notUsed: true,
      revision: "synthetic-app-revision",
      savedMode: null,
      hasPendingOperation: null,
      pointerConsistent: null,
      liveStatus: "unknown",
      storedFieldsMatch: null,
      canRecoverOperation: false,
      defaultAction: "unknown",
      defaultTakeover: false,
      canCompleteApp: false,
      canStartUpgrade: false,
      directProviderResolution: "verification_required",
      retainedProviderId: null,
      keepFilesProviders: [],
      canChooseProvider: false,
      canChooseMode: false,
      modeRouteProviders: [],
    }),
  );
}
it("opens the original embedded startup review from the real checkpoint fault without automatic writes", async () => {
  checkpointFault();
  const user = userEvent.setup();
  mount();
  const review = await screen.findByRole("button", {
    name: "startupUpgrade.reviewApps",
  });
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(startupUpgradeApi.queryApp).not.toHaveBeenCalled();
  expect(
    screen.getByRole("button", { name: "routingWorkspace.preview" }),
  ).toBeDisabled();
  await user.click(review);
  await screen.findByRole("dialog");
  await waitFor(() =>
    expect(startupUpgradeApi.queryApp).toHaveBeenCalledWith(
      "synthetic-owner-token",
      "codex",
    ),
  );
  for (const method of [
    startupUpgradeApi.prepare,
    startupUpgradeApi.publish,
    startupUpgradeApi.complete,
    startupUpgradeApi.continueRuntime,
    startupUpgradeApi.recoverApp,
    sharedRoutingApi.applyTarget,
  ]) {
    expect(method).not.toHaveBeenCalled();
  }
  await user.keyboard("{Escape}");
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  expect(startupUpgradeApi.query).toHaveBeenCalledTimes(3);
});
it("does not query startup recovery in the normal C2 workspace", async () => {
  mount();
  await screen.findAllByText("saved-model");
  expect(startupUpgradeApi.query).not.toHaveBeenCalled();
  expect(
    screen.queryByRole("button", { name: "startupUpgrade.reviewApps" }),
  ).not.toBeInTheDocument();
});
it("keeps unknown checkpoint review read-only and rechecks only on explicit action", async () => {
  checkpointFault();
  vi.mocked(startupUpgradeApi.query).mockResolvedValue({
    ...checkpointReview,
    status: "verification_required",
  });
  mount();
  await screen.findByText("startupUpgrade.verificationRequired");
  await waitFor(() => expect(startupUpgradeApi.query).toHaveBeenCalledTimes(1));
  expect(
    screen.queryByRole("button", { name: "startupUpgrade.reviewApps" }),
  ).not.toBeInTheDocument();
  await userEvent
    .setup()
    .click(
      screen.getByRole("button", { name: "applications.recheckOperation" }),
    );
  await waitFor(() => expect(startupUpgradeApi.query).toHaveBeenCalledTimes(2));
  expect(startupUpgradeApi.queryApp).not.toHaveBeenCalled();
  expect(startupUpgradeApi.recoverApp).not.toHaveBeenCalled();
});
it("does not expose stale checkpoint authority after a failed read", async () => {
  checkpointFault();
  const user = userEvent.setup();
  mount();
  await screen.findByRole("button", { name: "startupUpgrade.reviewApps" });
  vi.mocked(startupUpgradeApi.query).mockRejectedValue(
    new Error("private native detail"),
  );
  await user.click(
    screen.getByRole("button", { name: "applications.recheckOperation" }),
  );
  await screen.findByText("startupUpgrade.queryFailed");
  expect(
    screen.queryByRole("button", { name: "startupUpgrade.reviewApps" }),
  ).not.toBeInTheDocument();
  expect(screen.queryByText("private native detail")).not.toBeInTheDocument();
  expect(startupUpgradeApi.queryApp).not.toHaveBeenCalled();
});

it("retains original global expiry and Codex model settings in the advanced area without a second write owner", async () => {
  vi.mocked(applicationOverviewApi.get).mockResolvedValue({
    configurations: [
      {
        providerId: "original-provider",
        name: "Original model configuration",
        source: "relay",
        account: null,
        serviceName: null,
        accountLabel: null,
        configurationName: null,
        model: "saved-model",
        presentation: { isCurrent: true } as never,
        selection: { kind: "relay" },
        canSelect: false,
      },
    ],
    recentProviderIds: [],
    isAdditive: false,
  });
  vi.mocked(autoModeApi.getStatus).mockResolvedValue({
    enabled: false,
    strategy: "cheapest",
    model: null,
    availableModels: [],
    hasCandidates: false,
    cliInstalled: true,
  });
  vi.mocked(applicationModelSettingsApi.get).mockRejectedValue(
    new Error("provider.modelSettings.readOnly"),
  );
  mount();
  await screen.findAllByText("saved-model");
  expect(autoModeApi.getStatus).not.toHaveBeenCalled();
  expect(applicationOverviewApi.get).not.toHaveBeenCalled();
  const user = userEvent.setup();
  await user.click(screen.getByText("routingWorkspace.advanced"));
  await screen.findByText("Original model configuration");
  expect(autoModeApi.getStatus).toHaveBeenCalledWith("codex");
  expect(applicationOverviewApi.get).toHaveBeenCalledWith("codex");
  await user.click(
    screen.getByRole("button", {
      name: "applications.modelSettings.open · Original model configuration",
    }),
  );
  await screen.findByRole("dialog");
  await waitFor(() =>
    expect(applicationModelSettingsApi.get).toHaveBeenCalledWith(
      "codex",
      "original-provider",
    ),
  );
  expect(applicationModelSettingsApi.preview).not.toHaveBeenCalled();
  expect(applicationModelSettingsApi.confirm).not.toHaveBeenCalled();
  expect(autoModeApi.setStrategy).not.toHaveBeenCalled();
});
it("does not expose stale advanced model controls when the original inventory read fails", async () => {
  vi.mocked(applicationOverviewApi.get).mockRejectedValue(
    new Error("private detail"),
  );
  vi.mocked(autoModeApi.getStatus).mockRejectedValue(
    new Error("private detail"),
  );
  mount();
  await screen.findAllByText("saved-model");
  await userEvent.setup().click(screen.getByText("routingWorkspace.advanced"));
  await screen.findByText("applications.loadFailed");
  expect(
    screen.queryByRole("button", { name: /applications.modelSettings.open/ }),
  ).not.toBeInTheDocument();
  expect(screen.queryByText("private detail")).not.toBeInTheDocument();
});

it("retains explicit original blocked-tier and error reset actions without claiming group eligibility or replaying routes", async () => {
  const connection = await applicationRoutingApi.get("codex");
  vi.mocked(applicationRoutingApi.get).mockResolvedValue({
    ...connection,
    tiers: [
      {
        providerId: "blocked-original",
        name: "Original blocked configuration",
        skipReason: "blocked",
        lastError: "not displayed raw",
        consecutiveFailures: 1,
      } as never,
    ],
  });
  vi.mocked(applicationOverviewApi.get).mockResolvedValue({
    configurations: [],
    recentProviderIds: [],
    isAdditive: false,
  });
  vi.mocked(autoModeApi.getStatus).mockResolvedValue({
    enabled: false,
    strategy: "cheapest",
    model: null,
    availableModels: [],
    hasCandidates: false,
    cliInstalled: true,
  });
  mount();
  const user = userEvent.setup();
  await screen.findAllByText("saved-model");
  await user.click(screen.getByText("routingWorkspace.advanced"));
  await user.click(
    await screen.findByRole("button", {
      name: "applications.unblockTier · Original blocked configuration",
    }),
  );
  await waitFor(() =>
    expect(applicationRoutingApi.setTierBlocked).toHaveBeenCalledWith(
      "codex",
      "blocked-original",
      false,
    ),
  );
  await user.click(
    await screen.findByRole("button", {
      name: "applications.resetTierErrors · Original blocked configuration",
    }),
  );
  await waitFor(() =>
    expect(failoverApi.resetCircuitBreaker).toHaveBeenCalledWith(
      "blocked-original",
      "codex",
    ),
  );
  expect(
    screen.getByRole("button", {
      name: "applications.unblockTier · Original blocked configuration",
    }),
  ).toBeInTheDocument();
  expect(screen.queryByText("not displayed raw")).not.toBeInTheDocument();
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
  expect(screen.getByTestId("actual-request-fact")).toHaveTextContent(
    "routingWorkspace.requestUnknown",
  );
});

it("does not treat a generic failed routing read or writable error payload as checkpoint authority", async () => {
  vi.mocked(applicationRoutingApi.get).mockRejectedValue({
    code: "mode.verification_required",
    modeState: {
      status: "pending",
      canWrite: true,
      blockedReason: "upgrade.checkpoint_pending",
    },
  });
  mount();
  await screen.findByText("routingWorkspace.readFailed");
  expect(
    screen.getByText("routingWorkspace.connectionUnknown"),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "startupUpgrade.reviewApps" }),
  ).not.toBeInTheDocument();
  expect(startupUpgradeApi.query).not.toHaveBeenCalled();
});

it("offers original pending request recovery despite the native blocked-read rejection, without replaying an apply", async () => {
  state = {
    ...state,
    pendingRequest: request,
    canApply: false,
    blockedReason: "routing_groups.recovery_required",
  };
  vi.mocked(applicationRoutingApi.get).mockRejectedValue({
    code: "mode.verification_required",
    modeState: {
      status: "pending",
      writeProtocol: null,
      blockedReason: "mode.verification_required",
      mode: null,
      attached: null,
      currentProviderId: null,
      directProviderId: null,
      publicationStarted: true,
      canWrite: false,
      canRecheck: true,
      legacyCommonConfigWritable: false,
    },
  });
  vi.mocked(sharedRoutingApi.recoverTarget).mockImplementation(async () => {
    state = { ...state, pendingRequest: null };
    return { request, status: "discarded" };
  });
  mount();
  const recover = await screen.findByRole("button", {
    name: "routingWorkspace.recoverOperation",
  });
  expect(sharedRoutingApi.recoverTarget).not.toHaveBeenCalled();
  await userEvent.setup().click(recover);
  await waitFor(() =>
    expect(sharedRoutingApi.recoverTarget).toHaveBeenCalledExactlyOnceWith(
      "codex",
      request,
    ),
  );
  expect(sharedRoutingApi.applyTarget).not.toHaveBeenCalled();
  expect(startupUpgradeApi.query).not.toHaveBeenCalled();
  expect(screen.getByTestId("actual-request-fact")).toHaveTextContent(
    "routingWorkspace.requestUnknown",
  );
});
