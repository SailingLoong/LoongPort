import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApplicationWorkspace } from "../ApplicationWorkspace";
import {
  applicationRoutingApi,
  type ApplicationRouting,
} from "@/lib/api/applicationRouting";
import { applicationOverviewApi } from "@/lib/api/applicationOverview";
import { orderProfilesApi } from "@/lib/api/orderProfiles";
import { modelVerificationApi } from "@/lib/api/modelVerification";

// Keep every production hook, QueryClient, Workspace and child component real.
// Only API methods return isolated synthetic facts; no runtime IPC or network.
const config = (id: string, current: boolean) => ({
  providerId: id,
  name: `Synthetic ${id}`,
  source: "relay" as const,
  account: { kind: "relay" as const, id: 7 },
  serviceName: "Fixture",
  accountLabel: "Synthetic",
  configurationName: id,
  model: published === "B" ? "model-b" : "model-a",
  presentation: {
    isCurrent: current,
    isInConfig: true,
    isDefaultModel: false,
    isOfficial: false,
    isManaged: true,
    isReadOnly: false,
    canDelete: true,
    canEdit: true,
    canTestConnectivity: false,
    canConfigureUsage: false,
    usesOfficialSubscriptionUsage: false,
    canSetAsDefault: false,
    routingBadge: null,
    routingReason: null,
    switchBlockedReason: null,
  },
  canSelect: true,
  selection: { kind: "relay" as const },
});
let client: QueryClient;
let published: "A" | "B" | "C";
let readFailure: boolean;
let resolveRead: (value: ApplicationRouting) => void;
let rejectRead: (error: Error) => void;
let delayedRead: Promise<ApplicationRouting> | undefined;
let apply: ReturnType<typeof vi.spyOn>;
let get: ReturnType<typeof vi.spyOn>;
let queryRequest: ReturnType<typeof vi.spyOn>;
let receiptStatus: "completed" | "unknown";

const routingFacts = (): ApplicationRouting => ({
  modeState: {
    status: "ready",
    mode: "proxy",
    attached: true,
    currentProviderId: "a",
    directProviderId: "a",
    publicationStarted: false,
    canWrite: true,
    canRecheck: true,
  },
  autoFailoverEnabled: true,
  routingActive: true,
  model: published === "B" ? "model-b" : "model-a",
  modelOptions: [],
  chainIds: published === "B" ? ["b", "a"] : ["a", "b"],
  tiers: ["a", "b"].map((id, position) => ({
    providerId: id,
    name: `Synthetic ${id}`,
    position,
    isCurrent: id === "a",
    skipReason: null,
    canFailover: true,
    canVerifyModels: false,
    models: ["model-a", "model-b"],
    subscriptionWindows: [],
    nextResetAt: null,
    rateMultiplier: position + 1,
    errorRate: null,
    unitPricePerMillion: null,
    effectiveModel: published === "B" ? "model-b" : "model-a",
    avgFirstTokenMs: null,
    balanceUsd: null,
    verificationVerdict: null,
    isHealthy: null,
    consecutiveFailures: null,
    lastError: null,
    todayCostUsd: null,
    todayRequests: null,
    cacheHitRate: null,
    recentActivity: null,
    breakerState: null,
    breakerReopenInSecs: null,
    affinityRemainingSecs: null,
  })),
});
const props = {
  appId: "codex" as const,
  providers: {},
  onSwitchProvider: vi.fn(),
  onOpenAccount: vi.fn(),
  onAdd: vi.fn(),
  children: null,
};

beforeEach(() => {
  published = "A";
  receiptStatus = "completed";
  readFailure = false;
  delayedRead = undefined;
  client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  get = vi.spyOn(applicationRoutingApi, "get").mockImplementation(async () => {
    if (delayedRead) return delayedRead;
    if (readFailure) throw new Error("synthetic read unavailable");
    return routingFacts();
  });
  vi.spyOn(applicationOverviewApi, "get").mockImplementation(async () => ({
    configurations: [config("a", true), config("b", false)],
    recentProviderIds: [],
    isAdditive: false,
  }));
  vi.spyOn(orderProfilesApi, "list").mockImplementation(async () => ({
    profiles: [
      { name: "default", providerIds: ["a", "b"] },
      { name: "Travel", providerIds: ["b", "a"] },
    ],
    current:
      published === "B" ? "Travel" : published === "C" ? "Other" : "default",
  }));
  vi.spyOn(modelVerificationApi, "listSummaries").mockResolvedValue([]);
  vi.spyOn(applicationRoutingApi, "prepareRequest").mockImplementation(
    async (_app, _change, id) => ({
      id,
      scope: "applicationRouting",
      draftDigest: "a".repeat(64),
      revision: "b".repeat(64),
    }),
  );
  queryRequest = vi
    .spyOn(applicationRoutingApi, "queryRequest")
    .mockImplementation(async (_app, request) => ({
      request,
      status: receiptStatus,
    }));
  apply = vi
    .spyOn(applicationRoutingApi, "apply")
    .mockImplementation(async () => {
      published = "B";
      delayedRead = new Promise((resolve, reject) => {
        resolveRead = resolve;
        rejectRead = reject;
      });
      throw new Error("synthetic response lost after commit");
    });
});
afterEach(() => {
  cleanup();
  client.clear();
  vi.restoreAllMocks();
});

async function stageAndApply() {
  render(
    <QueryClientProvider client={client}>
      <ApplicationWorkspace {...props} />
    </QueryClientProvider>,
  );
  await waitFor(() =>
    expect(screen.getByTitle("applications.orderProfiles")).toBeEnabled(),
  );
  await userEvent.click(screen.getByTitle("applications.orderProfiles"));
  await userEvent.click(
    await screen.findByRole("menuitem", { name: /Travel/ }),
  );
  await userEvent.click(
    screen.getByRole("combobox", { name: "applications.modelFilter" }),
  );
  await userEvent.click(screen.getByRole("option", { name: /model-b/ }));
  const button = screen.getByRole("button", {
    name: /applications.applyOrder/,
  });
  await userEvent.click(button);
  await userEvent.click(
    await screen.findByRole("button", { name: "common.confirm" }),
  );
  await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(button).toBeDisabled());
  expect(
    screen.getByRole("button", { name: /applications.discardOrder/ }),
  ).toBeDisabled();
  return button;
}

describe("U01 real Workspace and routing hook after a lost apply reply", () => {
  it("resolves the submitted B intent independently after a later C supersedes it", async () => {
    await stageAndApply();
    published = "C";
    await act(async () => {
      delayedRead = undefined;
      resolveRead(routingFacts());
      await client.invalidateQueries({ queryKey: ["orderProfiles", "codex"] });
    });
    expect(
      client.getQueryData<ApplicationRouting>(["applicationRouting", "codex"])
        ?.chainIds,
    ).toEqual(["a", "b"]);
    // The API simulation already committed B. Current C is not proof that B
    // failed. Resolve original B through its original owner, without replaying.
    await waitFor(() =>
      expect(
        screen.queryByText("applications.pendingChanges"),
      ).not.toBeInTheDocument(),
    );
    expect(apply).toHaveBeenCalledTimes(1);
  });
  it("stays busy through the fact refresh, then clears pending B without a second apply", async () => {
    await stageAndApply();
    await act(async () => {
      const facts = routingFacts();
      delayedRead = undefined;
      resolveRead(facts);
    });
    await waitFor(() =>
      expect(
        screen.queryByText("applications.pendingChanges"),
      ).not.toBeInTheDocument(),
    );
    const reads = get.mock.calls.length;
    await act(async () => {
      await client.invalidateQueries({
        queryKey: ["applicationRouting", "codex"],
      });
    });
    expect(get.mock.calls.length).toBeGreaterThan(reads);
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply).toHaveBeenCalledWith(
      "codex",
      {
        order: { profileName: "Travel", providerIds: ["b", "a"] },
        selection: { providerId: "a", model: "model-b" },
      },
      undefined,
      expect.objectContaining({ scope: "applicationRouting" }),
    );
  });

  it("blocks writes after failed reading and retries only reads before recognizing B", async () => {
    const button = await stageAndApply();
    await act(async () => {
      delayedRead = undefined;
      readFailure = true;
      rejectRead(new Error("synthetic read unavailable"));
    });
    await screen.findByText("applications.loadFailed");
    expect(button).toBeDisabled();
    const reads = get.mock.calls.length;
    readFailure = false;
    await userEvent.click(screen.getByRole("button", { name: "common.retry" }));
    await waitFor(() =>
      expect(
        screen.queryByText("applications.pendingChanges"),
      ).not.toBeInTheDocument(),
    );
    expect(get.mock.calls.length).toBeGreaterThan(reads);
    expect(apply).toHaveBeenCalledTimes(1);
  });

  it("does not automatically replay B when the refreshed owner is now C", async () => {
    receiptStatus = "unknown";
    const button = await stageAndApply();
    published = "C";
    await act(async () => {
      delayedRead = undefined;
      resolveRead(routingFacts());
      await client.invalidateQueries({ queryKey: ["orderProfiles", "codex"] });
    });
    await screen.findByText("applications.originalRequestUnresolved");
    expect(button).toBeDisabled();
    expect(
      client.getQueryData<ApplicationRouting>(["applicationRouting", "codex"])
        ?.chainIds,
    ).toEqual(["a", "b"]);
    expect(screen.getByText("applications.pendingChanges")).toBeVisible();
    expect(apply).toHaveBeenCalledTimes(1);
    const request = apply.mock.calls[0][3];
    await userEvent.click(
      screen.getByRole("button", {
        name: "applications.recheckOriginalRequest",
      }),
    );
    await waitFor(() => expect(queryRequest).toHaveBeenCalledTimes(2));
    expect(apply).toHaveBeenCalledTimes(1);
    expect(queryRequest).toHaveBeenLastCalledWith("codex", request);
    await userEvent.click(
      screen.getByRole("button", { name: "applications.startNewRequest" }),
    );
    await waitFor(() => expect(button).toBeEnabled());
    apply.mockResolvedValue({
      status: "switched",
      providerName: "Synthetic",
      warnings: [],
      chatgptWasRunning: false,
      chatgptRelaunched: false,
    });
    await userEvent.click(button);
    await userEvent.click(
      await screen.findByRole("button", { name: "common.confirm" }),
    );
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    expect(apply.mock.calls[1][3].id).not.toEqual(request.id);
  });
});
