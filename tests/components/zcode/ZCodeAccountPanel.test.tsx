import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
  cleanup,
} from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ZCodeAccountPanel } from "@/components/zcode/ZCodeAccountPanel";
import {
  zcodeAccountsApi,
  type CatalogStatus,
  type RecoveryStatus,
  type SessionCheckDisplay,
} from "@/lib/api/zcodeAccounts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@/lib/api/zcodeAccounts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/zcodeAccounts")>()),
  zcodeAccountsApi: {
    library: vi.fn(),
    setLabel: vi.fn(),
    readCurrentIdentity: vi.fn(),
    checkConnections: vi.fn(),
    cancelConnectionCheck: vi.fn(),
    queryLastOperation: vi.fn().mockResolvedValue(null),
    openForLogin: vi.fn(),
    latestVersion: vi.fn(),
    discover: vi.fn(),
    inspect: vi.fn(),
    status: vi.fn(),
    previewCapture: vi.fn(),
    commitCapture: vi.fn(),
    cancelCapture: vi.fn(),
    switch: vi.fn(),
    recoveryStatus: vi.fn(),
    archive: vi.fn(),
    confirmRecovery: vi.fn(),
    recapture: vi.fn(),
    deleteRecovery: vi.fn(),
  },
}));
const source = {
  installPath: "/Applications/ZCode.app",
  dataRoot: "/example/.zcode/v2",
  keyMode: "standard" as const,
};
const context = {
  contextId: "context-one",
  contextRevision: "context-revision",
  dataRoot: source.dataRoot,
  family: "zai" as const,
  version: "1.0",
  build: "fixture",
};
const catalog: CatalogStatus = {
  revision: "catalog-one",
  profiles: [
    {
      id: "opaque-zai",
      family: "zai",
      label: "Personal Z.ai",
      sourceVerified: true,
      identitySource: "officialLogin",
      officialLabel: "p…",
      capabilities: null,
      canActivate: true,
      activationBlockedReason: null,
      needsKey: false,
      canCompleteCoding: false,
      completeCodingBlockedReason: null,
      canCheckConnections: true,
      checkConnectionsBlockedReason: null,
    },
    {
      id: "opaque-bigmodel",
      family: "bigmodel",
      label: null,
      sourceVerified: true,
      identitySource: "nativeCapture",
      officialLabel: null,
      capabilities: null,
      canActivate: false,
      needsKey: false,
      canCompleteCoding: false,
      completeCodingBlockedReason: null,
      canCheckConnections: true,
      checkConnectionsBlockedReason: null,
      activationBlockedReason: {
        code: "zcode.account.target_scope_mismatch",
        remedy: "chooseSavedAccount",
        committed: false,
      },
    },
  ],
  current: null,
  pending: false,
  nativeUnconfirmed: false,
  actions: {
    canAdd: true,
    canImport: true,
    canBackup: true,
    canEditLabels: true,
    blockedReason: null,
  },
};
const recovery: RecoveryStatus = {
  revision: "recovery-one",
  pending: false,
  nativeUnconfirmed: false,
  records: [],
};
function priorEvidence(): SessionCheckDisplay {
  const accepted = {
    state: "accepted" as const,
    reason: null,
    checkedAt: 100,
    source: "accountStartJwt" as const,
    latestFailure: null,
  };
  return {
    selectedProfileId: "opaque-zai",
    business: { check: accepted, officialOwnerId: null, displayName: null },
    start: {
      check: accepted,
      entitlement: "available",
      effectiveAtSeconds: null,
      quota: accepted,
      serverTimeSeconds: 100,
      plans: [],
      buckets: [
        {
          bucketId: "bucket",
          userPlanId: "instance",
          planId: "plan",
          entitlementId: "tokens",
          showName: "Prior allowance",
          meter: "tokens",
          unitType: "tokens",
          capabilities: [],
          totalUnits: 10,
          usedUnits: 4,
          reservedUnits: 0,
          remainingUnits: 6,
          availableUnits: 6,
          periodStartSeconds: 10,
          periodEndSeconds: 1000,
          expiresAtSeconds: 1000,
        },
      ],
    },
    coding: {
      check: accepted,
      subscription: accepted,
      entitlement: "available",
      quota: accepted,
      subscriptions: [],
      limits: [],
    },
  };
}
function mount(onBusyChange = vi.fn()) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ZCodeAccountPanel onBusyChange={onBusyChange} />
    </QueryClientProvider>,
  );
  return client;
}
async function inspect() {
  fireEvent.change(screen.getByLabelText("ZCode data directory"), {
    target: { value: source.dataRoot },
  });
  fireEvent.click(
    screen.getByRole("checkbox", {
      name: "Use only the standard local key to verify the selected data",
    }),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Inspect selected source" }),
  );
  await screen.findByText(/Inspected:/);
  await waitFor(() => expect(zcodeAccountsApi.status).toHaveBeenCalled());
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Refresh account status" }),
    ).toBeEnabled(),
  );
}
function confirm(name: string) {
  fireEvent.click(
    within(screen.getByRole("dialog")).getByRole("button", { name }),
  );
}
beforeEach(() => {
  vi.mocked(zcodeAccountsApi.checkConnections)
    .mockReset()
    .mockResolvedValue({ ...catalog, revision: "checked-revision" });
  vi.mocked(zcodeAccountsApi.cancelConnectionCheck)
    .mockReset()
    .mockResolvedValue("cancelled");
  vi.mocked(zcodeAccountsApi.queryLastOperation)
    .mockReset()
    .mockResolvedValue(null);
  vi.mocked(zcodeAccountsApi.openForLogin)
    .mockReset()
    .mockResolvedValue(undefined);
  localStorage.clear();
  vi.mocked(zcodeAccountsApi.latestVersion)
    .mockReset()
    .mockResolvedValue({ version: "99.0.0", checkedAt: 1, error: null });
  vi.mocked(invoke).mockReset();
  vi.mocked(zcodeAccountsApi.discover).mockReset().mockResolvedValue({
    dataRoot: "",
    sourceBasis: "osAccountHome",
    candidates: [],
    latestStatus: "notQueried",
  });
  vi.mocked(zcodeAccountsApi.inspect).mockReset().mockResolvedValue(context);
  vi.mocked(zcodeAccountsApi.status).mockReset().mockResolvedValue(catalog);
  vi.mocked(zcodeAccountsApi.library)
    .mockReset()
    .mockResolvedValue({
      ...catalog,
      profiles: catalog.profiles.map((p) => ({ ...p, canActivate: false })),
    });
  vi.mocked(zcodeAccountsApi.setLabel).mockReset().mockResolvedValue(catalog);
  vi.mocked(zcodeAccountsApi.readCurrentIdentity)
    .mockReset()
    .mockResolvedValue({
      contextRevision: context.contextRevision,
      id: "opaque-zai",
      label: "p…",
      family: "zai",
      readAt: 1_800_000_000_000,
    });
  vi.mocked(zcodeAccountsApi.recoveryStatus)
    .mockReset()
    .mockResolvedValue(recovery);
  vi.mocked(zcodeAccountsApi.previewCapture).mockReset().mockResolvedValue({
    id: "opaque-reviewed",
    label: "a…",
    family: "zai",
    duplicate: false,
    previewId: "review-one",
  });
  vi.mocked(zcodeAccountsApi.commitCapture)
    .mockReset()
    .mockResolvedValue("saved");
  vi.mocked(zcodeAccountsApi.switch).mockReset().mockResolvedValue("switched");
  vi.mocked(zcodeAccountsApi.archive).mockReset().mockResolvedValue("archived");
  vi.mocked(zcodeAccountsApi.confirmRecovery)
    .mockReset()
    .mockResolvedValue(undefined);
  vi.mocked(zcodeAccountsApi.recapture).mockReset().mockResolvedValue("saved");
  vi.mocked(zcodeAccountsApi.deleteRecovery)
    .mockReset()
    .mockResolvedValue(undefined);
});
describe("ZCode saved accounts", () => {
  it("keeps prior verified facts and marks retained amounts as historical after a query failure", async () => {
    const prior = {
      ...catalog,
      profiles: [{ ...catalog.profiles[0], capabilities: priorEvidence() }],
    };
    vi.mocked(zcodeAccountsApi.library).mockResolvedValue(prior);
    vi.mocked(zcodeAccountsApi.checkConnections).mockRejectedValue(
      new Error("private query transport"),
    );
    mount();
    await screen.findByText("Remaining: 6 tokens");
    expect(screen.getByText("Last known quota values")).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "Check connections and quota" }),
    );
    await screen.findByText("Previous quota values · not current");
    expect(
      screen.getByText("Current Start quota is unknown."),
    ).toBeInTheDocument();
    expect(screen.getByText("Start Plan: Accepted")).toBeInTheDocument();
    expect(screen.getByText("Remaining: 6 tokens")).toBeInTheDocument();
    expect(
      screen.getAllByText("Checked: 1970-01-01T00:01:40.000Z").length,
    ).toBeGreaterThan(0);
    expect(
      screen.queryByText(/private query transport/),
    ).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "Refresh account status" }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Refresh account status" }),
      ).toBeEnabled(),
    );
    expect(zcodeAccountsApi.checkConnections).toHaveBeenCalledTimes(1);
  });
  it("retains a returned saved check result when refreshing native action status fails", async () => {
    mount();
    await inspect();
    vi.mocked(zcodeAccountsApi.status).mockRejectedValueOnce(
      new Error("private local refresh failure"),
    );
    vi.mocked(zcodeAccountsApi.checkConnections).mockResolvedValue({
      ...catalog,
      revision: "checked",
      profiles: [
        {
          ...catalog.profiles[0],
          label: "Checked account",
          capabilities: priorEvidence(),
        },
      ],
    });
    fireEvent.click(
      within(screen.getByRole("region", { name: "Z.ai accounts" })).getByRole(
        "button",
        { name: "Check connections and quota" },
      ),
    );
    await screen.findByText("Checked account");
    await screen.findByText(
      /local action completed, but refreshing its status failed/,
    );
    expect(
      screen.getByText(
        "Connection and quota check completed for this account.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Quota returned by this check"),
    ).toBeInTheDocument();
    expect(zcodeAccountsApi.checkConnections).toHaveBeenCalledTimes(1);
  });
  it("does not claim that cancelling after commit revoked the saved result", async () => {
    let resolve!: (value: CatalogStatus) => void;
    vi.mocked(zcodeAccountsApi.checkConnections).mockReturnValue(
      new Promise((yes) => {
        resolve = yes;
      }),
    );
    vi.mocked(zcodeAccountsApi.cancelConnectionCheck).mockResolvedValue(
      "tooLate",
    );
    mount();
    await screen.findByText("Personal Z.ai");
    fireEvent.click(
      screen.getAllByRole("button", { name: "Check connections and quota" })[0],
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.checkConnections).toHaveBeenCalledTimes(1),
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel check" }));
    await screen.findByText(
      "The check may already have been saved. Refresh local account status to see its result.",
    );
    await act(async () =>
      resolve({
        ...catalog,
        profiles: [{ ...catalog.profiles[0], label: "Ignored late result" }],
      }),
    );
    expect(screen.queryByText("Ignored late result")).not.toBeInTheDocument();
    expect(
      screen.queryByText(
        "Connection check cancelled before saving its result.",
      ),
    ).not.toBeInTheDocument();
  });
  it("checks only after the single-account action and never from local list refresh", async () => {
    let resolve!: (value: CatalogStatus) => void;
    vi.mocked(zcodeAccountsApi.checkConnections).mockReturnValue(
      new Promise((yes) => {
        resolve = yes;
      }),
    );
    const onBusy = vi.fn();
    mount(onBusy);
    await screen.findByText("Personal Z.ai");
    expect(zcodeAccountsApi.checkConnections).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Refresh account status" }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Refresh account status" }),
      ).toBeEnabled(),
    );
    expect(zcodeAccountsApi.checkConnections).not.toHaveBeenCalled();
    const button = within(
      screen.getByRole("region", { name: "Z.ai accounts" }),
    ).getByRole("button", { name: "Check connections and quota" });
    fireEvent.click(button);
    fireEvent.click(button);
    await waitFor(() =>
      expect(zcodeAccountsApi.checkConnections).toHaveBeenCalledExactlyOnceWith(
        {
          requestId: expect.any(String),
          catalogRevision: "catalog-one",
          id: "opaque-zai",
          allowOfficialCheck: true,
        },
      ),
    );
    expect(
      screen.getByRole("button", { name: "Checking connections…" }),
    ).toBeDisabled();
    expect(onBusy).toHaveBeenLastCalledWith(false);
    await act(async () =>
      resolve({ ...catalog, revision: "checked-revision" }),
    );
    expect(zcodeAccountsApi.checkConnections).toHaveBeenCalledTimes(1);
  });
  it("uses the backend reason when a saved account cannot be checked", async () => {
    vi.mocked(zcodeAccountsApi.library).mockResolvedValue({
      ...catalog,
      profiles: [
        {
          ...catalog.profiles[0],
          canCheckConnections: false,
          checkConnectionsBlockedReason: {
            code: "zcode.account.vault_unavailable",
            remedy: "unlockVault",
            committed: false,
          },
        },
      ],
    });
    mount();
    expect(
      await screen.findByRole("button", {
        name: "Check connections and quota",
      }),
    ).toBeDisabled();
    expect(
      screen.getByText(/Unlock the local LoongPort vault/),
    ).toBeInTheDocument();
  });
  it("cancels on source change and discards the old query reply", async () => {
    let resolve!: (value: CatalogStatus) => void;
    vi.mocked(zcodeAccountsApi.checkConnections).mockReturnValue(
      new Promise((yes) => {
        resolve = yes;
      }),
    );
    mount();
    await screen.findByText("Personal Z.ai");
    fireEvent.click(
      screen.getAllByRole("button", { name: "Check connections and quota" })[0],
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.checkConnections).toHaveBeenCalledTimes(1),
    );
    const request = vi.mocked(zcodeAccountsApi.checkConnections).mock
      .calls[0][0];
    fireEvent.change(screen.getByLabelText("ZCode data directory"), {
      target: { value: "/new/library" },
    });
    await waitFor(() =>
      expect(
        zcodeAccountsApi.cancelConnectionCheck,
      ).toHaveBeenCalledExactlyOnceWith(request.requestId),
    );
    await act(async () =>
      resolve({
        ...catalog,
        profiles: [{ ...catalog.profiles[0], label: "stale query result" }],
      }),
    );
    expect(screen.queryByText("stale query result")).not.toBeInTheDocument();
  });
  it("cancels on unmount and does not publish a late checked catalog into the cache", async () => {
    let resolve!: (value: CatalogStatus) => void;
    vi.mocked(zcodeAccountsApi.checkConnections).mockReturnValue(
      new Promise((yes) => {
        resolve = yes;
      }),
    );
    const client = mount();
    await screen.findByText("Personal Z.ai");
    fireEvent.click(
      screen.getAllByRole("button", { name: "Check connections and quota" })[0],
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.checkConnections).toHaveBeenCalledTimes(1),
    );
    const before = client.getQueryData(["zcodeAccountLibrary", null]);
    cleanup();
    await act(async () => resolve({ ...catalog, revision: "late-check" }));
    expect(client.getQueryData(["zcodeAccountLibrary", null])).toEqual(before);
    expect(zcodeAccountsApi.cancelConnectionCheck).toHaveBeenCalledTimes(1);
  });
  it("does not let a late check replace a newer account record", async () => {
    let resolve!: (value: CatalogStatus) => void;
    vi.mocked(zcodeAccountsApi.checkConnections).mockReturnValue(
      new Promise((yes) => {
        resolve = yes;
      }),
    );
    const client = mount();
    await screen.findByText("Personal Z.ai");
    fireEvent.click(
      screen.getAllByRole("button", { name: "Check connections and quota" })[0],
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.checkConnections).toHaveBeenCalledTimes(1),
    );
    await act(async () =>
      client.setQueryData(["zcodeAccountLibrary", null], {
        ...catalog,
        revision: "newer",
        profiles: [{ ...catalog.profiles[0], label: "Newer record" }],
      }),
    );
    await act(async () => resolve({ ...catalog, revision: "old-result" }));
    expect(screen.getByText("Newer record")).toBeInTheDocument();
    expect(zcodeAccountsApi.cancelConnectionCheck).toHaveBeenCalledTimes(1);
  });
  it("opens saved Coding completion only with backend eligibility and keeps the chosen identity", async () => {
    vi.mocked(zcodeAccountsApi.library).mockResolvedValue({
      ...catalog,
      profiles: [
        { ...catalog.profiles[0], needsKey: true, canCompleteCoding: true },
      ],
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_zcode_last_login_progress") return null;
      if (command === "begin_saved_zcode_coding")
        return {
          flowId: "saved-flow",
          purpose: "completeCoding",
          phase: "keyRequired",
          family: "zai",
          authorization: null,
          account: {
            id: "opaque-zai",
            label: "p…",
            duplicate: true,
            identitySource: "nativeCapture",
          },
          connections: {
            start: "ready",
            coding: "unavailable",
            needsKey: true,
          },
          project: {
            organizationId: "org",
            organizationName: "Personal",
            projectId: "project",
            projectName: "Personal",
          },
          keyCreated: false,
          keyMayExist: false,
          keyManagementUrl: "https://example.test/keys",
          error: null,
          saved: null,
        };
      return null;
    });
    mount();
    fireEvent.click(
      await screen.findByRole("button", { name: "Complete Coding connection" }),
    );
    await screen.findByRole("button", {
      name: "Authorize creating and saving this Key",
    });
    expect(invoke).toHaveBeenCalledWith("begin_saved_zcode_coding", {
      id: "opaque-zai",
      catalogRevision: "catalog-one",
    });
    expect(
      vi
        .mocked(invoke)
        .mock.calls.some(
          ([command]) => command === "begin_zcode_official_login",
        ),
    ).toBe(false);
  });
  it("shows a blocked saved Coding entry without inferring eligibility from a missing Key", async () => {
    vi.mocked(zcodeAccountsApi.library).mockResolvedValue({
      ...catalog,
      profiles: [
        {
          ...catalog.profiles[0],
          needsKey: true,
          canCompleteCoding: false,
          completeCodingBlockedReason: {
            code: "zcode.account.official_unavailable",
            remedy: "queryOriginal",
            committed: false,
          },
        },
      ],
    });
    mount();
    expect(
      await screen.findByRole("button", { name: "Complete Coding connection" }),
    ).toBeDisabled();
    expect(
      screen.getByText(/official service could not complete this step/),
    ).toBeInTheDocument();
  });
  it("keeps account management visible without an admitted native installation", async () => {
    mount();
    await screen.findByText("Personal Z.ai");
    expect(screen.getByRole("button", { name: "Add account" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Import .zsb" })).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Encrypted backup" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.readCurrentIdentity).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Encrypted backup" }));
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      "Personal Z.ai",
    );
  });
  it("disables new library writes when backend action eligibility is absent", async () => {
    vi.mocked(zcodeAccountsApi.library).mockResolvedValue({
      ...catalog,
      actions: {
        canAdd: false,
        canImport: false,
        canBackup: false,
        canEditLabels: false,
        blockedReason: {
          code: "zcode.account.vault_unavailable",
          remedy: "unlockVault",
          committed: false,
        },
      },
    });
    mount();
    await screen.findByText("Personal Z.ai");
    expect(screen.getByRole("button", { name: "Add account" })).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Encrypted backup" }),
    ).toBeDisabled();
    expect(
      screen.getByText(/Unlock the local LoongPort vault/),
    ).toBeInTheDocument();
  });
  it("edits only a local display label using the reviewed library revision", async () => {
    vi.mocked(zcodeAccountsApi.setLabel).mockResolvedValue({
      ...catalog,
      revision: "label-updated",
      profiles: [{ ...catalog.profiles[0], label: "My work account" }],
    });
    mount();
    await screen.findByText("Personal Z.ai");
    fireEvent.click(
      screen.getAllByRole("button", { name: "Edit display name" })[0],
    );
    fireEvent.change(screen.getByLabelText("Display name"), {
      target: { value: "My work account" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save display name" }));
    await waitFor(() =>
      expect(zcodeAccountsApi.setLabel).toHaveBeenCalledExactlyOnceWith(
        undefined,
        "catalog-one",
        "opaque-zai",
        "My work account",
      ),
    );
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.switch).not.toHaveBeenCalled();
  });
  it("reads native identity only on request and invalidates the observation when focus leaves", async () => {
    mount();
    await inspect();
    expect(zcodeAccountsApi.readCurrentIdentity).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Read current native identity" }),
    );
    await screen.findByText(/Current native account: p…/);
    expect(
      zcodeAccountsApi.readCurrentIdentity,
    ).toHaveBeenCalledExactlyOnceWith(source, context.contextRevision);
    fireEvent(window, new Event("blur"));
    expect(screen.getByText("Current account: unknown")).toBeInTheDocument();
    expect(
      screen.queryByText(/Current native account:/),
    ).not.toBeInTheDocument();
  });
  it("ignores a native identity read that finishes after the observation is invalidated", async () => {
    let finish!: (
      value: Awaited<ReturnType<typeof zcodeAccountsApi.readCurrentIdentity>>,
    ) => void;
    vi.mocked(zcodeAccountsApi.readCurrentIdentity).mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    mount();
    await inspect();
    const button = screen.getByRole("button", {
      name: "Read current native identity",
    });
    fireEvent.click(button);
    fireEvent.click(button);
    fireEvent(window, new Event("blur"));
    await act(async () =>
      finish({
        contextRevision: context.contextRevision,
        id: "opaque-zai",
        label: "stale-native",
        family: "zai",
        readAt: 1234,
      }),
    );
    expect(screen.queryByText(/stale-native/)).not.toBeInTheDocument();
    expect(zcodeAccountsApi.readCurrentIdentity).toHaveBeenCalledTimes(1);
  });
  it("allows an imported row only when the backend grants activation", async () => {
    vi.mocked(zcodeAccountsApi.status).mockResolvedValue({
      ...catalog,
      profiles: [
        {
          ...catalog.profiles[0],
          sourceVerified: false,
          identitySource: "packageDeclared",
          canActivate: true,
        },
      ],
    });
    mount();
    await inspect();
    expect(
      screen.getByRole("button", { name: "Switch saved account" }),
    ).toBeEnabled();
  });
  it("shows independent credential checks and separate quota instances with authoritative units and times", async () => {
    const accepted = {
      state: "accepted" as const,
      reason: null,
      checkedAt: 1_800_000_000,
      source: "accountStartJwt" as const,
      latestFailure: null,
    };
    const bucket = {
      bucketId: "monthly-bucket",
      userPlanId: "monthly-instance",
      planId: "plan-one",
      entitlementId: "tokens",
      showName: "Monthly quota",
      meter: "tokens",
      unitType: "tokens",
      capabilities: [],
      totalUnits: 100,
      usedUnits: 100,
      reservedUnits: 0,
      remainingUnits: 0,
      availableUnits: 0,
      periodStartSeconds: 1_800_000_000,
      periodEndSeconds: 1_800_086_400,
      expiresAtSeconds: 1_800_172_800,
    };
    const capabilities: SessionCheckDisplay = {
      selectedProfileId: "opaque-zai",
      business: {
        check: {
          ...accepted,
          state: "unavailable",
          reason: "businessRejected",
          source: "businessToken",
        },
        officialOwnerId: null,
        displayName: null,
      },
      start: {
        check: accepted,
        entitlement: "available",
        effectiveAtSeconds: null,
        quota: accepted,
        serverTimeSeconds: 1_800_000_000,
        plans: [
          {
            userPlanId: "monthly-instance",
            planId: "plan-one",
            name: "Start subscription",
            status: "active",
            startsAtSeconds: 1_800_000_000,
            endsAtSeconds: 1_800_086_400,
            entitlements: [],
          },
        ],
        buckets: [
          bucket,
          {
            ...bucket,
            bucketId: "bonus-bucket",
            userPlanId: "bonus-instance",
            showName: "Bonus quota",
            totalUnits: 200,
            usedUnits: 40,
            remainingUnits: null,
            availableUnits: null,
          },
        ],
      },
      coding: {
        check: { ...accepted, source: "codingKey" },
        subscription: accepted,
        entitlement: "available",
        quota: {
          ...accepted,
          state: "unknown",
          reason: "network",
          latestFailure: { reason: "network", checkedAt: 1_800_000_010 },
        },
        subscriptions: [],
        limits: [
          {
            limitType: "TIME_LIMIT",
            unit: 999,
            number: 7,
            displayUnit: null,
            windowLabel: null,
            usage: 0,
            currentValue: 0,
            remaining: null,
            percentage: null,
            nextResetTimeMs: 1_800_000_000_123,
            usageDetails: [],
          },
        ],
      },
    };
    vi.mocked(zcodeAccountsApi.library).mockResolvedValue({
      ...catalog,
      profiles: [{ ...catalog.profiles[0], capabilities }],
    });
    mount();
    await screen.findByText("Start Plan: Accepted");
    expect(
      screen.getByText(
        "Business session: Unavailable · Business session rejected",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("Coding Plan: Accepted")).toBeInTheDocument();
    const monthly = screen.getByRole("group", { name: "Quota bucket 1" });
    const bonus = screen.getByRole("group", { name: "Quota bucket 2" });
    expect(monthly).toHaveTextContent("Remaining: 0 tokens");
    expect(monthly).toHaveTextContent("Plan instance: monthly-instance");
    expect(bonus).toHaveTextContent("Remaining: Unknown");
    expect(bonus).toHaveTextContent("Plan instance: bonus-instance");
    expect(monthly).toHaveTextContent(
      `Window ends: ${new Date(1_800_086_400_000).toISOString()}`,
    );
    const coding = screen.getByRole("group", { name: "Coding limit 1" });
    expect(coding).toHaveTextContent("Unit: Not provided");
    expect(coding).toHaveTextContent("Window: Not provided");
    expect(coding).toHaveTextContent(
      `Next reset: ${new Date(1_800_000_000_123).toISOString()}`,
    );
    expect(coding).not.toHaveTextContent("999");
    expect(
      screen.getByText(/Latest query: Network query failed/),
    ).toHaveTextContent(new Date(1_800_000_010_000).toISOString());
    expect(zcodeAccountsApi.readCurrentIdentity).not.toHaveBeenCalled();
  });
  it("queries the original request on inspect and refresh after reopening without a switch", async () => {
    vi.mocked(zcodeAccountsApi.queryLastOperation).mockResolvedValue({
      requestId: "original",
      phase: "restartVerified",
      target: "opaque-zai",
      refreshed: false,
      restartRequested: true,
    });
    mount();
    await inspect();
    expect(
      await screen.findByText(
        "The original local switch is confirmed. Online sign-in remains unverified.",
      ),
    ).toBeVisible();
    expect(zcodeAccountsApi.queryLastOperation).toHaveBeenCalledWith(
      source,
      context.contextRevision,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Refresh account status" }),
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.queryLastOperation).toHaveBeenCalledTimes(2),
    );
    expect(zcodeAccountsApi.switch).not.toHaveBeenCalled();
  });
  it("opens the selected official source for login without capturing or claiming account validity", async () => {
    mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Open official ZCode for login" }),
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.openForLogin).toHaveBeenCalledWith(source),
    );
    await screen.findByText(/Official ZCode opened with the selected source/);
    expect(zcodeAccountsApi.previewCapture).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.switch).not.toHaveBeenCalled();
  });
  it("shows the backend activation reason for an imported account without requiring every import to reauthenticate", async () => {
    vi.mocked(zcodeAccountsApi.status).mockResolvedValue({
      ...catalog,
      profiles: [
        {
          ...catalog.profiles[0],
          sourceVerified: false,
          identitySource: "packageDeclared",
          canActivate: false,
          activationBlockedReason: {
            code: "zcode.account.target_scope_mismatch",
            remedy: "chooseSavedAccount",
            committed: false,
          },
        },
      ],
    });
    mount();
    await inspect();
    await screen.findByText(/Declared by account bundle/);
    expect(
      screen.queryByText(/Source unverified\. Sign in/),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Switch saved account" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeEnabled();
    expect(zcodeAccountsApi.switch).not.toHaveBeenCalled();
  });
  it("discovers one verified installation and bootstrap root without inspecting credentials", async () => {
    vi.mocked(zcodeAccountsApi.discover).mockResolvedValue({
      dataRoot: source.dataRoot,
      sourceBasis: "bootstrapDataBaseDir",
      candidates: [
        {
          installPath: "/custom/ZCode.app",
          version: "3.14.4",
          build: "3.14.4.7912",
          verifiedBuild: true,
        },
      ],
      latestStatus: "notQueried",
    });
    mount();
    await screen.findByText("Current: 3.14.4 · 3.14.4.7912");
    await waitFor(() =>
      expect(screen.getByLabelText("ZCode data directory")).toHaveValue(
        source.dataRoot,
      ),
    );
    expect(screen.getByLabelText("ZCode installation")).toHaveValue(
      "/custom/ZCode.app",
    );
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.previewCapture).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Inspect selected source" }),
    ).toBeDisabled();
  });
  it("requires an explicit selection for conflicting detected installations", async () => {
    vi.mocked(zcodeAccountsApi.discover).mockResolvedValue({
      dataRoot: source.dataRoot,
      sourceBasis: "osAccountHome",
      candidates: ["/one/ZCode.app", "/two/ZCode.app"].map((installPath) => ({
        installPath,
        version: "3.14.4",
        build: "3.14.4.7912",
        verifiedBuild: true,
      })),
      latestStatus: "notQueried",
    });
    mount();
    await screen.findAllByRole("button", { name: "Choose this installation" });
    expect(screen.getByLabelText("ZCode installation")).toHaveValue("");
    fireEvent.click(
      screen.getAllByRole("button", { name: "Choose this installation" })[1],
    );
    expect(screen.getByLabelText("ZCode installation")).toHaveValue(
      "/two/ZCode.app",
    );
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
  });
  it("restores only untrusted source paths and never restores key or admission", async () => {
    localStorage.setItem(
      "loongport:zcode-source-v1",
      JSON.stringify({
        version: 1,
        installPath: "/saved/ZCode.app",
        dataRoot: "/saved/.zcode/v2",
        keyMode: "standard",
        verified: true,
        token: "canary-secret",
      }),
    );
    mount();
    await screen.findByText(
      "Saved paths restored as a preference. Installation, data source and key context must be checked again.",
    );
    expect(screen.getByLabelText("ZCode installation")).toHaveValue(
      "/saved/ZCode.app",
    );
    expect(
      screen.getByRole("checkbox", {
        name: "Use only the standard local key to verify the selected data",
      }),
    ).not.toBeChecked();
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.previewCapture).not.toHaveBeenCalled();
    expect(document.body.textContent).not.toContain("canary-secret");
  });
  it("does not silently select a verified candidate when another detected installation conflicts", async () => {
    vi.mocked(zcodeAccountsApi.discover).mockResolvedValue({
      dataRoot: source.dataRoot,
      sourceBasis: "osAccountHome",
      latestStatus: "notQueried",
      candidates: [
        {
          installPath: "/one/ZCode.app",
          version: "3.14.4",
          build: "3.14.4.7912",
          verifiedBuild: true,
        },
        {
          installPath: "/two/ZCode.app",
          version: "99",
          build: "99.1",
          verifiedBuild: false,
        },
      ],
    });
    mount();
    await screen.findAllByRole("button", { name: "Choose this installation" });
    expect(screen.getByLabelText("ZCode installation")).toHaveValue("");
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
  });
  it("shows the actual unique unverified installation without substituting another path", async () => {
    vi.mocked(zcodeAccountsApi.discover).mockResolvedValue({
      dataRoot: source.dataRoot,
      sourceBasis: "osAccountHome",
      latestStatus: "notQueried",
      candidates: [
        {
          installPath: "/user/Applications/ZCode.app",
          version: "99",
          build: "99.1",
          verifiedBuild: false,
        },
      ],
    });
    mount();
    await screen.findByText(
      "Installed build is unverified; native capture and switching remain blocked.",
    );
    await waitFor(() =>
      expect(screen.getByLabelText("ZCode installation")).toHaveValue(
        "/user/Applications/ZCode.app",
      ),
    );
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.previewCapture).not.toHaveBeenCalled();
  });
  it("queries latest only explicitly, never uses it as installed compatibility, and clears old success on failure", async () => {
    mount();
    await waitFor(() =>
      expect(zcodeAccountsApi.discover).toHaveBeenCalledTimes(1),
    );
    expect(zcodeAccountsApi.latestVersion).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Query official latest version" }),
    );
    await screen.findByText(
      "Official latest version: 99.0.0 · checked 1970-01-01T00:00:01.000Z",
    );
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
    vi.mocked(zcodeAccountsApi.latestVersion).mockResolvedValueOnce({
      version: null,
      checkedAt: 2,
      error: "network",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Query official latest version" }),
    );
    await screen.findByText(
      "Official version query failed at 1970-01-01T00:00:02.000Z. No current result is available.",
    );
    expect(
      screen.queryByText(/Official latest version: 99/),
    ).not.toBeInTheDocument();
    expect(zcodeAccountsApi.previewCapture).not.toHaveBeenCalled();
  });
  it("loads the independent library and local recovery on mount but never captures without review", async () => {
    mount();
    await waitFor(() =>
      expect(zcodeAccountsApi.recoveryStatus).toHaveBeenCalledTimes(1),
    );
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.status).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.library).toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Inspect selected source" }),
    ).toBeDisabled();
    await inspect();
    expect(zcodeAccountsApi.inspect).toHaveBeenCalledWith(source);
    expect(zcodeAccountsApi.status).toHaveBeenCalledWith(
      source,
      context.contextRevision,
    );
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Save current account" }),
    );
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent(source.dataRoot);
    expect(dialog).toHaveTextContent("masked preview");
    expect(dialog).toHaveTextContent("Canceling");
    expect(zcodeAccountsApi.previewCapture).not.toHaveBeenCalled();
    confirm("Cancel account action");
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.switch).not.toHaveBeenCalled();
  });
  it("cancels a masked preview without saving and keeps duplicate update explicit", async () => {
    vi.mocked(zcodeAccountsApi.previewCapture).mockResolvedValue({
      id: "opaque-reviewed",
      label: "a…",
      family: "zai",
      duplicate: true,
      previewId: "review-one",
    });
    vi.mocked(zcodeAccountsApi.commitCapture).mockResolvedValue("kept");
    mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Save current account" }),
    );
    confirm("Read masked preview");
    await screen.findByText("Save reviewed ZCode account?");
    expect(screen.getByRole("dialog")).toHaveTextContent("a…");
    expect(
      screen.getByRole("checkbox", {
        name: "Explicitly update this existing saved account",
      }),
    ).not.toBeChecked();
    confirm("Cancel account action");
    await waitFor(() =>
      expect(zcodeAccountsApi.cancelCapture).toHaveBeenCalledWith("review-one"),
    );
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Save current account" }),
    );
    confirm("Read masked preview");
    await screen.findByText("Save reviewed ZCode account?");
    confirm("Confirm account choice");
    await screen.findByText("Existing saved account kept unchanged.");
    expect(zcodeAccountsApi.commitCapture).toHaveBeenCalledWith(
      source,
      "context-revision",
      "catalog-one",
      "review-one",
      false,
    );
    expect(zcodeAccountsApi.cancelCapture).toHaveBeenCalledWith("review-one");
  });
  it("updates a duplicate only after an explicit preview checkbox choice", async () => {
    vi.mocked(zcodeAccountsApi.previewCapture).mockResolvedValue({
      id: "opaque-reviewed",
      label: "a…",
      family: "zai",
      duplicate: true,
      previewId: "review-one",
    });
    vi.mocked(zcodeAccountsApi.commitCapture).mockResolvedValue("refreshed");
    mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Save current account" }),
    );
    confirm("Read masked preview");
    await screen.findByText("Save reviewed ZCode account?");
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Explicitly update this existing saved account",
      }),
    );
    confirm("Confirm account choice");
    await screen.findByText("Saved account refreshed locally.");
    expect(zcodeAccountsApi.commitCapture).toHaveBeenCalledWith(
      source,
      "context-revision",
      "catalog-one",
      "review-one",
      true,
    );
  });
  it.each(["catalog_changed", "native_changed"])(
    "requires a new preview after second-step %s rejection and never replays on refresh",
    async (code) => {
      vi.mocked(zcodeAccountsApi.commitCapture).mockRejectedValueOnce({
        code: `zcode.account.${code}`,
        remedy: "refreshContext",
        committed: false,
      });
      mount();
      await inspect();
      fireEvent.click(
        screen.getByRole("button", { name: "Save current account" }),
      );
      confirm("Read masked preview");
      await screen.findByText("Save reviewed ZCode account?");
      confirm("Confirm account choice");
      await screen.findByRole("alert");
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "Save current account" }),
      ).toBeDisabled();
      fireEvent.click(
        screen.getByRole("button", { name: "Refresh account status" }),
      );
      await waitFor(() =>
        expect(zcodeAccountsApi.status).toHaveBeenCalledTimes(2),
      );
      expect(zcodeAccountsApi.commitCapture).toHaveBeenCalledTimes(1);
      expect(zcodeAccountsApi.previewCapture).toHaveBeenCalledTimes(1);
    },
  );
  it("serializes repeat capture clicks, passes reviewed revisions, and keeps navigation blocked through status refresh", async () => {
    let finish!: (value: "saved") => void;
    vi.mocked(zcodeAccountsApi.commitCapture).mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const onBusy = vi.fn();
    mount(onBusy);
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Save current account" }),
    );
    confirm("Read masked preview");
    await screen.findByText("Save reviewed ZCode account?");
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
    confirm("Confirm account choice");
    confirm("Confirm account choice");
    await waitFor(() =>
      expect(zcodeAccountsApi.commitCapture).toHaveBeenCalledTimes(1),
    );
    expect(zcodeAccountsApi.commitCapture).toHaveBeenCalledWith(
      source,
      "context-revision",
      "catalog-one",
      "review-one",
      false,
    );
    expect(onBusy).toHaveBeenLastCalledWith(true);
    let refreshDone!: (value: CatalogStatus) => void;
    vi.mocked(zcodeAccountsApi.status).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          refreshDone = resolve;
        }),
    );
    await act(async () => finish("saved"));
    await waitFor(() =>
      expect(zcodeAccountsApi.status).toHaveBeenCalledTimes(2),
    );
    expect(onBusy).toHaveBeenLastCalledWith(true);
    await act(async () => refreshDone(catalog));
    await screen.findByText("Account saved locally.");
    await waitFor(() => expect(onBusy).toHaveBeenLastCalledWith(false));
    expect(zcodeAccountsApi.status).toHaveBeenCalledTimes(2);
  });
  it("separates families, blocks cross-family targets and switches only the selected opaque ID without optimistic current state", async () => {
    mount();
    await inspect();
    const otherFamily = screen.getByRole("region", {
      name: "BigModel accounts",
    });
    expect(
      within(otherFamily).getByRole("button", { name: "Switch saved account" }),
    ).toBeDisabled();
    const sameFamily = screen.getByRole("region", { name: "Z.ai accounts" });
    fireEvent.click(
      within(sameFamily).getByRole("button", { name: "Switch saved account" }),
    );
    confirm("Switch to this account");
    await waitFor(() =>
      expect(zcodeAccountsApi.switch).toHaveBeenCalledWith(
        source,
        "context-revision",
        "opaque-zai",
        "catalog-one",
      ),
    );
    expect(screen.getByText("Current account: unknown")).toBeInTheDocument();
  });
  it("invalidates inspected context when source selection changes", async () => {
    mount();
    await inspect();
    fireEvent.change(screen.getByLabelText("ZCode data directory"), {
      target: { value: "/different/.zcode/v2" },
    });
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    expect(screen.queryByText("Personal Z.ai")).not.toBeInTheDocument();
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
  });
  it.each(["key_context_unknown", "team_unsupported", "native_gate_pending"])(
    "shows safe actionable %s errors without raw canary content",
    async (code) => {
      vi.mocked(zcodeAccountsApi.inspect).mockRejectedValue({
        code: `zcode.account.${code}`,
        remedy: "openNativeSettings",
        committed: false,
        message: "secret-canary-token",
      });
      const client = mount();
      fireEvent.change(screen.getByLabelText("ZCode data directory"), {
        target: { value: source.dataRoot },
      });
      fireEvent.click(
        screen.getByRole("checkbox", {
          name: "Use only the standard local key to verify the selected data",
        }),
      );
      fireEvent.click(
        screen.getByRole("button", { name: "Inspect selected source" }),
      );
      await screen.findByRole("alert");
      expect(
        screen.getByRole("button", { name: "Save current account" }),
      ).toBeDisabled();
      expect(document.body.textContent).not.toContain("secret-canary");
      expect(
        JSON.stringify(
          client
            .getQueryCache()
            .getAll()
            .map((q) => q.state),
        ),
      ).not.toContain("secret-canary");
    },
  );
  it("requires a new review after a stale revision and never replays a mutation on refresh", async () => {
    vi.mocked(zcodeAccountsApi.previewCapture).mockRejectedValueOnce({
      code: "zcode.account.catalog_changed",
      remedy: "refreshContext",
      committed: false,
    });
    mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Save current account" }),
    );
    confirm("Read masked preview");
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Refresh account status" }),
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.status).toHaveBeenCalledTimes(2),
    );
    expect(zcodeAccountsApi.previewCapture).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("retains a committed-but-recovery-required warning without success or automatic retry", async () => {
    vi.mocked(zcodeAccountsApi.switch).mockRejectedValueOnce({
      code: "zcode.account.committed_recovery_required",
      remedy: "reviewRecovery",
      committed: true,
      message: "secret-canary-token",
    });
    mount();
    await inspect();
    fireEvent.click(
      within(screen.getByRole("region", { name: "Z.ai accounts" })).getByRole(
        "button",
        { name: "Switch saved account" },
      ),
    );
    confirm("Switch to this account");
    expect(await screen.findByRole("alert")).toHaveTextContent("committed");
    expect(
      screen.queryByText(
        `Switched ${source.dataRoot} locally. Start official ZCode and verify the account there.`,
      ),
    ).not.toBeInTheDocument();
    expect(zcodeAccountsApi.switch).toHaveBeenCalledTimes(1);
    expect(document.body.textContent).not.toContain("secret-canary");
  });
  it("keeps local archive and confirmed-record cleanup available when source inspection fails", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      pending: true,
      nativeUnconfirmed: true,
      records: [
        {
          id: "old-confirmed",
          disposition: "full-before",
          latestCompleted: false,
        },
        {
          id: "old-unconfirmed",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    });
    mount();
    await screen.findByText("old-confirmed");
    fireEvent.click(
      screen.getByRole("button", { name: "Archive pending recovery" }),
    );
    confirm("Preserve pending recovery");
    await waitFor(() =>
      expect(zcodeAccountsApi.archive).toHaveBeenCalledWith("recovery-one"),
    );
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    const record = screen.getByRole("group", {
      name: "Recovery record old-confirmed",
    });
    expect(
      within(record).getByRole("button", {
        name: "Permanently delete recovery record",
      }),
    ).toBeEnabled();
    const unconfirmed = screen.getByRole("group", {
      name: "Recovery record old-unconfirmed",
    });
    expect(
      within(unconfirmed).getByRole("button", {
        name: "Permanently delete recovery record",
      }),
    ).toBeDisabled();
  });
  it("allows explicit recovery confirmation with an inspected source when the saved catalog is invalid", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      nativeUnconfirmed: true,
      records: [
        {
          id: "unconfirmed-record",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    });
    vi.mocked(zcodeAccountsApi.status).mockRejectedValue({
      code: "zcode.account.saved_data_invalid",
      remedy: "reviewSavedData",
      committed: false,
    });
    mount();
    fireEvent.change(screen.getByLabelText("ZCode data directory"), {
      target: { value: source.dataRoot },
    });
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Use only the standard local key to verify the selected data",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Inspect selected source" }),
    );
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Recapture after official sign-in" }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm this recovery record" }),
    );
    confirm("Check selected recovery record");
    await waitFor(() =>
      expect(zcodeAccountsApi.confirmRecovery).toHaveBeenCalledWith(
        source,
        "context-revision",
        "unconfirmed-record",
        "recovery-one",
      ),
    );
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.recapture).not.toHaveBeenCalled();
  });
  it("ignores an unrelated catalog revision change while confirming the reviewed recovery record", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      nativeUnconfirmed: true,
      records: [
        {
          id: "unconfirmed-record",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    });
    const client = mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm this recovery record" }),
    );
    await act(async () => {
      client.setQueryData(
        [
          "zcodeAccountCatalog",
          source.installPath,
          source.dataRoot,
          source.keyMode,
          context.contextRevision,
        ],
        {
          ...catalog,
          revision: "unrelated-catalog-new",
          profiles: [
            { ...catalog.profiles[0], label: "Fresh catalog label" },
            catalog.profiles[1],
          ],
        },
      );
    });
    await screen.findByText("Fresh catalog label");
    confirm("Check selected recovery record");
    await waitFor(() =>
      expect(zcodeAccountsApi.confirmRecovery).toHaveBeenCalledWith(
        source,
        "context-revision",
        "unconfirmed-record",
        "recovery-one",
      ),
    );
    expect(zcodeAccountsApi.confirmRecovery).toHaveBeenCalledTimes(1);
  });
  it("refuses recovery confirmation if the reviewed recovery revision changes", async () => {
    const initialRecovery: RecoveryStatus = {
      ...recovery,
      nativeUnconfirmed: true,
      records: [
        {
          id: "unconfirmed-record",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    };
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue(
      initialRecovery,
    );
    const client = mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm this recovery record" }),
    );
    await act(async () => {
      client.setQueryData(["zcodeAccountRecovery"], {
        ...initialRecovery,
        revision: "recovery-new",
        records: [
          ...initialRecovery.records,
          {
            id: "new-recovery-record",
            disposition: "full-after",
            latestCompleted: true,
          },
        ],
      });
    });
    await screen.findByText("new-recovery-record");
    confirm("Check selected recovery record");
    await screen.findByRole("alert");
    expect(zcodeAccountsApi.confirmRecovery).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("discards a reviewed recovery confirmation when its selected source changes", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      nativeUnconfirmed: true,
      records: [
        {
          id: "unconfirmed-record",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    });
    mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm this recovery record" }),
    );
    confirm("Cancel account action");
    fireEvent.change(screen.getByLabelText("ZCode data directory"), {
      target: { value: "/different/.zcode/v2" },
    });
    expect(
      screen.getByRole("button", { name: "Confirm this recovery record" }),
    ).toBeDisabled();
    expect(zcodeAccountsApi.confirmRecovery).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("blocks ordinary actions when pending and full, but permits exact-record confirmation, recapture and archive deduplication", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      pending: true,
      nativeUnconfirmed: true,
      records: [
        {
          id: "old-one",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
        {
          id: "old-two",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    });
    mount();
    await inspect();
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    expect(
      screen
        .getAllByRole("button", { name: "Switch saved account" })
        .every((button) => button.hasAttribute("disabled")),
    ).toBe(true);
    expect(
      screen.getByRole("button", { name: "Archive pending recovery" }),
    ).toBeEnabled();
    expect(screen.getByText(/Recovery storage is full/)).toHaveTextContent(
      "sign in again",
    );
    const record = screen.getByRole("group", {
      name: "Recovery record old-one",
    });
    fireEvent.click(
      within(record).getByRole("button", {
        name: "Confirm this recovery record",
      }),
    );
    confirm("Check selected recovery record");
    await waitFor(() =>
      expect(zcodeAccountsApi.confirmRecovery).toHaveBeenCalledWith(
        source,
        "context-revision",
        "old-one",
        "recovery-one",
      ),
    );
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    fireEvent.click(
      within(record).getByRole("button", {
        name: "Recapture after official sign-in",
      }),
    );
    expect(screen.getByRole("dialog")).toHaveTextContent("quit ZCode normally");
    confirm("Save and confirm selected record");
    await waitFor(() =>
      expect(zcodeAccountsApi.recapture).toHaveBeenCalledWith(
        source,
        "context-revision",
        "old-one",
        "recovery-one",
        "catalog-one",
      ),
    );
  });
  it("binds permanent cleanup to the named ID and revision and cancellation dispatches nothing", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      records: [
        {
          id: "selected-record",
          disposition: "full-after",
          latestCompleted: false,
        },
        {
          id: "other-record",
          disposition: "explicit-capture",
          latestCompleted: true,
        },
      ],
    });
    mount();
    await screen.findByText("selected-record");
    const row = screen.getByRole("group", {
      name: "Recovery record selected-record",
    });
    fireEvent.click(
      within(row).getByRole("button", {
        name: "Permanently delete recovery record",
      }),
    );
    expect(screen.getByRole("dialog")).toHaveTextContent("selected-record");
    expect(screen.getByRole("dialog")).toHaveTextContent("cannot be recovered");
    expect(screen.getByRole("dialog")).toHaveTextContent("source and target");
    confirm("Cancel account action");
    expect(zcodeAccountsApi.deleteRecovery).not.toHaveBeenCalled();
    fireEvent.click(
      within(row).getByRole("button", {
        name: "Permanently delete recovery record",
      }),
    );
    confirm("Delete selected record permanently");
    await waitFor(() =>
      expect(zcodeAccountsApi.deleteRecovery).toHaveBeenCalledWith(
        "selected-record",
        "recovery-one",
      ),
    );
  });
  it("keeps a completed local switch visible when the following status refresh fails", async () => {
    mount();
    await inspect();
    vi.mocked(zcodeAccountsApi.status).mockRejectedValueOnce({
      code: "zcode.account.storage_failed",
      remedy: "checkLocalStorage",
      committed: false,
    });
    fireEvent.click(
      within(screen.getByRole("region", { name: "Z.ai accounts" })).getByRole(
        "button",
        { name: "Switch saved account" },
      ),
    );
    confirm("Switch to this account");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "refreshing its status failed",
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      `Switched ${source.dataRoot} locally. Start official ZCode and verify the account there.`,
    );
    expect(screen.getByText("Current account: unknown")).toBeInTheDocument();
    expect(zcodeAccountsApi.switch).toHaveBeenCalledTimes(1);
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
  });
  it("keeps completed permanent cleanup visible when local recovery refresh fails", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      records: [
        {
          id: "selected-record",
          disposition: "full-after",
          latestCompleted: false,
        },
      ],
    });
    mount();
    await screen.findByText("selected-record");
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockRejectedValueOnce({
      code: "zcode.account.storage_failed",
      remedy: "checkLocalStorage",
      committed: false,
    });
    fireEvent.click(
      screen.getByRole("button", {
        name: "Permanently delete recovery record",
      }),
    );
    confirm("Delete selected record permanently");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "refreshing its status failed",
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "Selected recovery record permanently deleted.",
    );
    expect(zcodeAccountsApi.deleteRecovery).toHaveBeenCalledTimes(1);
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
  });
  it("keeps a failed status refresh read-only and exposes only static error text", async () => {
    const client = mount();
    await inspect();
    vi.mocked(zcodeAccountsApi.status).mockRejectedValueOnce({
      code: "zcode.account.context_changed",
      remedy: "refreshContext",
      message: "secret-canary-token",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Refresh account status" }),
    );
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.switch).not.toHaveBeenCalled();
    expect(
      JSON.stringify(
        client
          .getQueryCache()
          .getAll()
          .map((q) => q.state),
      ),
    ).not.toContain("secret-canary");
  });
  it("rejects a dialog whose catalog revision changed without silently rebasing the selected account", async () => {
    const client = mount();
    await inspect();
    fireEvent.click(
      within(screen.getByRole("region", { name: "Z.ai accounts" })).getByRole(
        "button",
        { name: "Switch saved account" },
      ),
    );
    await act(async () => {
      client.setQueryData(
        [
          "zcodeAccountCatalog",
          source.installPath,
          source.dataRoot,
          source.keyMode,
          context.contextRevision,
        ],
        {
          ...catalog,
          revision: "catalog-new",
          profiles: [
            { ...catalog.profiles[0], label: "New catalog label" },
            catalog.profiles[1],
          ],
        },
      );
    });
    await screen.findByText("New catalog label");
    confirm("Switch to this account");
    await screen.findByRole("alert");
    expect(zcodeAccountsApi.switch).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("blocks a new switch at full capacity even when both older records are confirmed", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      records: [
        { id: "old-one", disposition: "full-before", latestCompleted: false },
        { id: "old-two", disposition: "full-after", latestCompleted: false },
      ],
    });
    mount();
    await inspect();
    expect(
      within(screen.getByRole("region", { name: "Z.ai accounts" })).getByRole(
        "button",
        { name: "Switch saved account" },
      ),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeEnabled();
    expect(
      screen
        .getAllByRole("button", { name: "Permanently delete recovery record" })
        .every((button) => !button.hasAttribute("disabled")),
    ).toBe(true);
  });
  it("does not put backend error payloads into the query cache", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockRejectedValue({
      code: "untrusted-secret-canary",
      remedy: "untrusted-secret-canary",
      message: "secret-canary-token",
      committed: false,
    });
    const client = mount();
    await screen.findByRole("alert");
    expect(document.body.textContent).not.toContain("secret-canary");
    expect(
      JSON.stringify(
        client
          .getQueryCache()
          .getAll()
          .map((q) => q.state),
      ),
    ).not.toContain("secret-canary");
    expect(client.getMutationCache().getAll()).toHaveLength(0);
  });
  it("reports unsupported local storage without promising recovery or reading the native source", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockRejectedValue({
      code: "zcode.account.unsupported_platform",
      remedy: "finishPlatformCheck",
      committed: false,
    });
    mount();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "This platform does not support the requested account operation.",
    );
    expect(document.body.textContent).not.toContain(
      "Local recovery can be inspected",
    );
    expect(
      screen.getByRole("button", { name: "Archive pending recovery" }),
    ).toBeDisabled();
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.status).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.commitCapture).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.archive).not.toHaveBeenCalled();
  });
});

describe("ZCode account command bindings", () => {
  it("sends only the explicit source, opaque IDs and reviewed camelCase revisions", async () => {
    const { zcodeAccountsApi: api } = await vi.importActual<
      typeof import("@/lib/api/zcodeAccounts")
    >("@/lib/api/zcodeAccounts");
    vi.mocked(invoke).mockResolvedValueOnce(context);
    await api.inspect(source);
    vi.mocked(invoke).mockResolvedValueOnce(catalog);
    await api.status(source, "ctx");
    vi.mocked(invoke).mockResolvedValueOnce({
      previewId: "review-one",
      id: "opaque-reviewed",
      label: "a…",
      family: "zai",
      duplicate: false,
    });
    await api.previewCapture(source, "ctx", "cat");
    vi.mocked(invoke).mockResolvedValueOnce("kept");
    await api.commitCapture(source, "ctx", "cat", "review-one", false);
    await api.cancelCapture("review-one");
    vi.mocked(invoke).mockResolvedValueOnce("switched");
    await api.switch(source, "ctx", "opaque-id", "cat");
    vi.mocked(invoke).mockResolvedValueOnce(recovery);
    await api.recoveryStatus();
    vi.mocked(invoke).mockResolvedValueOnce("archived");
    await api.archive("rec");
    await api.confirmRecovery(source, "ctx", "record-id", "rec");
    vi.mocked(invoke).mockResolvedValueOnce("saved");
    await api.recapture(source, "ctx", "record-id", "rec", "cat");
    await api.deleteRecovery("record-id", "rec");
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["inspect_zcode_account_context", { source }],
      ["get_zcode_account_status", { source, contextRevision: "ctx" }],
      [
        "preview_zcode_current_account",
        { source, contextRevision: "ctx", catalogRevision: "cat" },
      ],
      [
        "save_zcode_account_preview",
        {
          source,
          contextRevision: "ctx",
          catalogRevision: "cat",
          previewId: "review-one",
          updateDuplicate: false,
        },
      ],
      ["cancel_zcode_account_preview", { previewId: "review-one" }],
      [
        "switch_zcode_saved_account",
        {
          source,
          contextRevision: "ctx",
          id: "opaque-id",
          catalogRevision: "cat",
          requestId: expect.any(String),
        },
      ],
      ["get_zcode_account_recovery", undefined],
      ["archive_zcode_account_recovery", { revision: "rec" }],
      [
        "confirm_zcode_account_recovery",
        {
          source,
          contextRevision: "ctx",
          id: "record-id",
          recoveryRevision: "rec",
        },
      ],
      [
        "recapture_zcode_account_recovery",
        {
          source,
          contextRevision: "ctx",
          id: "record-id",
          recoveryRevision: "rec",
          catalogRevision: "cat",
        },
      ],
      ["delete_zcode_account_recovery", { id: "record-id", revision: "rec" }],
    ]);
  });
  it("projects only safe metadata and never returns unknown error fields", async () => {
    const { zcodeAccountsApi: api } = await vi.importActual<
      typeof import("@/lib/api/zcodeAccounts")
    >("@/lib/api/zcodeAccounts");
    vi.mocked(invoke).mockResolvedValueOnce({
      ...catalog,
      secret: "canary-secret",
      profiles: catalog.profiles.map((profile) => ({
        ...profile,
        accessToken: "canary-secret",
      })),
    });
    expect(await api.status(source, "ctx")).toEqual(catalog);
    vi.mocked(invoke).mockRejectedValueOnce({
      code: "zcode.account.catalog_changed",
      remedy: "refreshContext",
      committed: false,
      message: "canary-secret",
      token: "canary-secret",
    });
    await expect(api.previewCapture(source, "ctx", "cat")).rejects.toEqual({
      code: "zcode.account.catalog_changed",
      remedy: "refreshContext",
      committed: false,
    });
  });
});
