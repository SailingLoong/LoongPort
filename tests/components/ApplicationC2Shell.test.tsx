import { Suspense } from "react";
import { render, screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import App from "@/App";
import { ThemeProvider } from "@/components/theme-provider";
import { sharedRoutingApi } from "@/lib/api/sharedRouting";
import { serviceSourcesApi } from "@/lib/api/serviceSources";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
import { credentialProtectionApi } from "@/lib/api/credentialProtection";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";
import {
  LAST_APP_STORAGE_KEY,
  LAST_VIEW_STORAGE_KEY,
} from "@/config/constants";
vi.mock("@/components/UpdateBadge", () => ({ UpdateBadge: () => null }));
vi.mock("@/components/relay/accounts/ManagedAccountApplicationPanel", () => ({
  ManagedAccountApplicationPanel: () => null,
}));
beforeEach(() => {
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => ({
      matches: false,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })),
  );
  localStorage.setItem(LAST_APP_STORAGE_KEY, "codex");
  localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
  vi.spyOn(sharedRoutingApi, "get").mockImplementation(async (appId) => ({
    appId,
    binding: null,
    groups: [],
    pendingRequest: null,
    canApply: true,
    blockedReason: null,
  }));
  vi.spyOn(serviceSourcesApi, "list").mockResolvedValue([]);
  vi.spyOn(applicationRoutingApi, "get").mockResolvedValue({
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
  vi.spyOn(credentialProtectionApi, "status").mockResolvedValue({
    mode: "plaintext",
    status: "available",
    shouldPrompt: false,
    canConfigure: true,
    canUnlock: false,
    canSwitchToPlaintext: true,
    canCreateSeparateConfiguration: false,
    reviewToken: "synthetic",
    restartRequired: false,
    errorCode: null,
  });
  vi.spyOn(serviceOnboardingApi, "status").mockResolvedValue({
    shouldPrompt: false,
    completed: true,
    plazaVisible: false,
  });
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  client.setQueryData(["runtimeAvailability"], {
    credentialRuntimeAvailable: true,
  });
  render(
    <QueryClientProvider client={client}>
      <ThemeProvider>
        <Suspense fallback={null}>
          <App />
        </Suspense>
      </ThemeProvider>
    </QueryClientProvider>,
  );
  return client;
}
it("mounts the actual compact application switcher at the top, only once, above the C2 workspace", async () => {
  mount();
  await screen.findByRole("region", { name: "routingWorkspace.workspace" });
  const header = document.querySelector("header")!;
  expect(within(header).getByRole("button", { name: "Codex" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  expect(screen.getAllByRole("button", { name: "Codex" })).toHaveLength(1);
  expect(screen.getByTestId("actual-request-fact")).toHaveTextContent(
    "routingWorkspace.requestUnknown",
  );
  expect(
    document.querySelector('[data-client-shell="true"]'),
  ).toBeInTheDocument();
});
it("uses the same app switch and global shared-group navigation rather than cloning groups per app", async () => {
  const user = userEvent.setup();
  mount();
  await screen.findByRole("region", { name: "routingWorkspace.workspace" });
  await user.click(
    within(document.querySelector("header")!).getByRole("button", {
      name: "Claude Code",
    }),
  );
  await waitFor(() =>
    expect(sharedRoutingApi.get).toHaveBeenCalledWith("claude"),
  );
  await user.click(screen.getByRole("button", { name: "client.sharedGroups" }));
  await screen.findByRole("heading", { name: "sharedGroups.title" });
  expect(
    screen.queryByRole("region", { name: "routingWorkspace.workspace" }),
  ).not.toBeInTheDocument();
});

it("keeps an unsaved C2 draft behind the existing leave guard when switching apps", async () => {
  const user = userEvent.setup();
  mount();
  await screen.findByRole("region", { name: "routingWorkspace.workspace" });
  await user.click(screen.getByLabelText("routingWorkspace.ordered"));
  await user.click(
    within(document.querySelector("header")!).getByRole("button", {
      name: "Claude Code",
    }),
  );
  await screen.findByText("applications.leaveDraftTitle");
  expect(sharedRoutingApi.get).not.toHaveBeenCalledWith("claude");
  await user.click(
    screen.getByRole("button", { name: "loongport.tier.editConfirmButton" }),
  );
  expect(screen.getByLabelText("routingWorkspace.ordered")).toBeChecked();
});
