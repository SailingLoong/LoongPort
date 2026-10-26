import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApplicationWorkspace } from "../ApplicationWorkspace";
import {
  applicationRoutingApi,
  type ApplicationRouting,
} from "@/lib/api/applicationRouting";
import { applicationOverviewApi } from "@/lib/api/applicationOverview";
import { startupUpgradeApi } from "@/lib/api/startupUpgrade";
import { modelVerificationApi } from "@/lib/api/modelVerification";
import { orderProfilesApi } from "@/lib/api/orderProfiles";

// The real remaining consumer, query client, hooks and checkpoint component stay
// mounted together. Only API results are synthetic; no IPC or network is used.
let client: QueryClient;
const routing: ApplicationRouting = {
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
  autoFailoverEnabled: false,
  routingActive: false,
  model: null,
  modelOptions: [],
  chainIds: [],
  tiers: [],
};
const props = {
  appId: "claude-desktop" as const,
  providers: {},
  onSwitchProvider: vi.fn(),
  onOpenAccount: vi.fn(),
  onAdd: vi.fn(),
  children: null,
};
beforeEach(() => {
  client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  vi.spyOn(applicationRoutingApi, "get").mockResolvedValue(routing);
  vi.spyOn(applicationOverviewApi, "get").mockResolvedValue({
    configurations: [],
    recentProviderIds: [],
    isAdditive: false,
  });
  vi.spyOn(modelVerificationApi, "listSummaries").mockResolvedValue([]);
  vi.spyOn(startupUpgradeApi, "query").mockResolvedValue({
    status: "runtime_active",
    reviewToken: null,
  } as any);
  vi.spyOn(applicationRoutingApi, "apply");
  vi.spyOn(applicationRoutingApi, "prepareRequest");
  vi.spyOn(applicationRoutingApi, "setOrder");
  vi.spyOn(orderProfilesApi, "list");
});
afterEach(() => {
  cleanup();
  client.clear();
  vi.restoreAllMocks();
});
function mount() {
  return render(
    <QueryClientProvider client={client}>
      <ApplicationWorkspace {...props} />
    </QueryClientProvider>,
  );
}
describe("real non-proxy Workspace checkpoint recovery compatibility", () => {
  it.each([
    ["runtime_active", null, false],
    ["database_verified", null, false],
    ["database_verified", "", false],
    ["database_verified", "review-token", true],
  ] as const)(
    "only offers the original verified checkpoint entry: %s/%s",
    async (status, reviewToken, canReview) => {
      vi.mocked(startupUpgradeApi.query).mockResolvedValue({
        status,
        reviewToken,
      } as any);
      mount();
      await screen.findByText("applications.operationNeedsVerification");
      await waitFor(() => expect(startupUpgradeApi.query).toHaveBeenCalled());
      if (canReview) {
        expect(
          await screen.findByRole("button", {
            name: "startupUpgrade.reviewApps",
          }),
        ).toBeEnabled();
      } else {
        await screen.findByTestId("startup-checkpoint-fault");
        expect(
          screen.queryByRole("button", { name: "startupUpgrade.reviewApps" }),
        ).not.toBeInTheDocument();
      }
      expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
      expect(applicationRoutingApi.prepareRequest).not.toHaveBeenCalled();
      expect(applicationRoutingApi.setOrder).not.toHaveBeenCalled();
      expect(orderProfilesApi.list).not.toHaveBeenCalled();
    },
  );
  it("rechecks an unknown checkpoint using only original reads, never replaying a route write", async () => {
    vi.mocked(startupUpgradeApi.query).mockRejectedValueOnce(
      new Error("checkpoint reply unavailable"),
    );
    mount();
    await screen.findByText("startupUpgrade.queryFailed");
    const reads = vi.mocked(applicationRoutingApi.get).mock.calls.length;
    vi.mocked(startupUpgradeApi.query).mockResolvedValue({
      status: "database_verified",
      reviewToken: "review-token",
    } as any);
    await userEvent.click(
      screen.getByRole("button", { name: "applications.recheckOperation" }),
    );
    expect(
      await screen.findByRole("button", { name: "startupUpgrade.reviewApps" }),
    ).toBeEnabled();
    expect(
      vi.mocked(applicationRoutingApi.get).mock.calls.length,
    ).toBeGreaterThan(reads);
    expect(applicationRoutingApi.apply).not.toHaveBeenCalled();
    expect(applicationRoutingApi.prepareRequest).not.toHaveBeenCalled();
    expect(applicationRoutingApi.setOrder).not.toHaveBeenCalled();
  });
});
