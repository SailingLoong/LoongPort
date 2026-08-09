import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  listRelays: vi.fn(),
  listTierRates: vi.fn(),
  checkSession: vi.fn(),
  provision: vi.fn(),
  status: vi.fn(),
  listSites: vi.fn(),
  listResults: vi.fn(),
  list: vi.fn(),
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
  vendorApi: { list: api.list },
  vendorSupportsApp: () => false,
}));
vi.mock("@/lib/api/modelVerification", () => ({
  modelVerificationApi: { listResults: api.listResults },
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
                verify {tier.providerId}
              </button>
            )}
            <button type="button" onClick={props.onRefresh}>
              refresh
            </button>
          </div>
        )),
      )}
    </div>
  ),
}));
vi.mock("../model-verification/ModelVerificationDialog", () => ({
  ModelVerificationDialog: (props: any) => (
    <div data-testid="verification-dialog">
      <span data-testid="dialog-tier">{props.tierDisplayName}</span>
      <span data-testid="dialog-run-id">run-1</span>
      <button type="button" onClick={() => props.onOpenChange(false)}>
        close dialog
      </button>
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

import { RelaySection } from "../RelaySection";

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

describe("RelaySection model verification ownership", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    eventHandlers.clear();
    api.listRelays.mockResolvedValue([relay]);
    api.listTierRates.mockResolvedValue([]);
    api.checkSession.mockResolvedValue([]);
    api.provision.mockResolvedValue({
      tiers: [],
      failures: [],
      keysCreated: 0,
    });
    api.status.mockResolvedValue({
      defaultSite: "",
      chatgptNeedsAttention: false,
    });
    api.listSites.mockResolvedValue([{}]);
    api.list.mockResolvedValue([]);
    api.listResults.mockResolvedValue([
      report("provider-a", "suspicious", "one"),
      report("provider-a", "anomaly", "two"),
    ]);
  });

  it("reduces reports from the initial and refreshed relay fetch, with one dialog owner", async () => {
    render(<RelaySection appId="codex" />);
    await waitFor(() =>
      expect(api.listResults).toHaveBeenCalledWith(["provider-a"]),
    );
    expect(screen.getByTestId("verdict-provider-a")).toHaveTextContent(
      "anomaly",
    );
    expect(screen.queryAllByTestId("verification-dialog")).toHaveLength(0);

    fireEvent.click(screen.getByRole("button", { name: "refresh" }));
    await waitFor(() => expect(api.listRelays).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(api.listResults).toHaveBeenCalledTimes(2));

    fireEvent.click(screen.getByRole("button", { name: "verify provider-a" }));
    expect(screen.getAllByTestId("verification-dialog")).toHaveLength(1);
    expect(screen.getByTestId("dialog-tier")).toHaveTextContent("provider-a");
  });

  it("clears a reset badge only after the matching backend change event", async () => {
    render(<RelaySection appId="codex" />);
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

  it("keeps one owner through close and reopen of the same backend run", async () => {
    render(<RelaySection appId="codex" />);
    await waitFor(() =>
      screen.getByRole("button", { name: "verify provider-a" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "verify provider-a" }));
    expect(screen.getByTestId("dialog-run-id")).toHaveTextContent("run-1");
    fireEvent.click(screen.getByRole("button", { name: "close dialog" }));
    fireEvent.click(screen.getByRole("button", { name: "verify provider-a" }));
    expect(screen.getAllByTestId("verification-dialog")).toHaveLength(1);
    expect(screen.getByTestId("dialog-run-id")).toHaveTextContent("run-1");
  });

  it.each([
    ["claude", true],
    ["codex-image", false],
    ["gemini", false],
  ] as const)("gates verification for %s tiers", async (appId, eligible) => {
    api.listRelays.mockResolvedValue([
      { ...relay, tiers: [tier("provider-a", appId)] },
    ]);
    render(<RelaySection appId={appId} />);
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
});
