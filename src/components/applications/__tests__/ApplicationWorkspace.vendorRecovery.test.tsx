import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ApplicationWorkspace } from "../ApplicationWorkspace";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
import { applicationOverviewApi } from "@/lib/api/applicationOverview";
import { vendorApi } from "@/lib/api/vendor";
import { modelVerificationApi } from "@/lib/api/modelVerification";
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
let client: QueryClient;
const operationId = "11fdd126-d40c-452f-8fb1-22597dfd5250";
const app = "claude-desktop" as const;
const target = {
  providerId: "vendor-3",
  name: "Account B",
  source: "official",
  account: { kind: "vendor", id: 3 },
  serviceName: "Vendor",
  accountLabel: "Account B",
  configurationName: "standard",
  model: null,
  presentation: {
    isOfficial: true,
    isManaged: true,
    isReadOnly: true,
    canDelete: false,
    canEdit: false,
    canTestConnectivity: false,
    canConfigureUsage: false,
    usesOfficialSubscriptionUsage: false,
    canSetAsDefault: false,
    routingBadge: null,
    routingReason: null,
    switchBlockedReason: null,
    isCurrent: false,
    isInConfig: true,
    isDefaultModel: false,
  },
  canSelect: true,
  selection: { kind: "vendor", rowId: 3, planId: "standard" },
} as const;
beforeEach(() => {
  client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  vi.spyOn(applicationRoutingApi, "get").mockResolvedValue({
    autoFailoverEnabled: false,
    routingActive: false,
    model: null,
    modelOptions: [],
    chainIds: [],
    tiers: [],
  });
  vi.spyOn(applicationOverviewApi, "get").mockResolvedValue({
    configurations: [target],
    recentProviderIds: [],
    isAdditive: false,
  });
  vi.spyOn(modelVerificationApi, "listSummaries").mockResolvedValue([]);
  vi.spyOn(vendorApi, "queryConnection").mockResolvedValue({
    status: "unknown",
    operationId,
    app,
  });
  vi.spyOn(vendorApi, "previewNewConnectionIntent").mockResolvedValue({
    status: "newIntentReviewRequired",
    app,
    operationId,
    receiptRevision: 1,
    originalRowId: 3,
    originalPlan: "standard",
    targetRowId: 3,
    targetPlan: "standard",
    targetProviderId: "vendor-3",
    reviewToken: "a".repeat(64),
  });
  vi.spyOn(vendorApi, "confirmNewConnectionIntent").mockResolvedValue({
    status: "newIntentPermitted",
    app,
  });
  vi.spyOn(vendorApi, "prepareConnection").mockResolvedValue({
    status: "unknown",
  });
  vi.spyOn(vendorApi, "switchPreparedConnection");
});
afterEach(() => {
  client.clear();
  vi.restoreAllMocks();
});
function mount() {
  return render(
    <QueryClientProvider client={client}>
      <ApplicationWorkspace
        appId={app}
        providers={{}}
        onSwitchProvider={() => {}}
        onOpenAccount={() => {}}
        onAdd={() => {}}
      >
        {null}
      </ApplicationWorkspace>
    </QueryClientProvider>,
  );
}
it("routes a real workspace use click through review UI, with cancellation never preparing or switching", async () => {
  const user = userEvent.setup();
  mount();
  await user.click(
    await screen.findByRole("button", { name: /applications.use/ }),
  );
  expect(await screen.findByRole("dialog")).toHaveTextContent(
    "applications.vendorRecovery.description",
  );
  expect(vendorApi.prepareConnection).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "common.cancel" }));
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  expect(vendorApi.confirmNewConnectionIntent).not.toHaveBeenCalled();
  expect(vendorApi.switchPreparedConnection).not.toHaveBeenCalled();
});
it("forwards only an explicit visible review confirmation before preparing a new UUID", async () => {
  const user = userEvent.setup();
  mount();
  await user.click(
    await screen.findByRole("button", { name: /applications.use/ }),
  );
  await user.click(
    await screen.findByRole("button", {
      name: "applications.vendorRecovery.confirm",
    }),
  );
  await waitFor(() =>
    expect(vendorApi.prepareConnection).toHaveBeenCalledTimes(1),
  );
  expect(
    vi.mocked(vendorApi.prepareConnection).mock.calls[0][0].operationId,
  ).not.toBe(operationId);
  expect(vendorApi.confirmNewConnectionIntent).toHaveBeenCalledWith({
    app,
    operationId,
    targetRowId: 3,
    targetPlan: "standard",
    reviewToken: "a".repeat(64),
  });
  expect(vendorApi.switchPreparedConnection).not.toHaveBeenCalled();
});
