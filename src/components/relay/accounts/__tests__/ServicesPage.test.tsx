import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
import type { RelayRow } from "@/lib/api/relay";
import type { VendorAccountRow } from "@/lib/api/vendor";
import { ServicesPage } from "../ServicesPage";

const mocks = vi.hoisted(() => ({
  relays: vi.fn(),
  vendors: vi.fn(),
  detail: vi.fn(),
  balance: vi.fn(),
}));
vi.mock("@/lib/api/relay", () => ({ relayApi: { listRelays: mocks.relays } }));
vi.mock("@/lib/api/vendor", () => ({ vendorApi: { list: mocks.vendors } }));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("../../RelaySection", () => ({
  RelaySection: (props: unknown) => {
    mocks.detail(props);
    return <div>Account operations</div>;
  },
}));
vi.mock("../../RowBalance", () => ({
  RowBalance: (props: unknown) => {
    mocks.balance(props);
    return <div>Account balance</div>;
  },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const relay: RelayRow = {
  id: 1,
  siteOrigin: "https://relay.example",
  siteName: "Example Relay",
  accountLabel: "Relay account",
  status: "notLoggedIn",
  isCurrent: false,
  canQueryBalance: false,
  canPurchase: false,
  canViewUsage: false,
  canRefresh: false,
  usageBlockers: [],
  removeConfirmation: "neverLoggedIn",
  tiers: [],
};
const vendor: VendorAccountRow = {
  id: 1,
  vendorId: "example",
  vendorName: "Example Official",
  accountLabel: "Official account",
  status: "sessionExpiredUsable",
  canQueryBalance: true,
  canRefresh: true,
  canDelete: true,
  plans: [
    {
      planId: "standard",
      planName: "Standard",
      providerId: "example-provider",
      isCurrent: false,
      userEdited: false,
      canEditConfig: true,
      canSwitch: true,
    },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.relays.mockResolvedValue([relay]);
  mocks.vendors.mockImplementation(async (app: AppId) => ({
    supported: app === "codex",
    accounts: app === "codex" ? [vendor] : [],
  }));
});

function setup(appId: AppId = "gemini") {
  const onOpenApp = vi.fn();
  const onOpenAddHub = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ServicesPage
        appId={appId}
        onOpenApp={onOpenApp}
        onOpenAddHub={onOpenAddHub}
      />
    </QueryClientProvider>,
  );
  return { onOpenApp, onOpenAddHub, user: userEvent.setup() };
}

describe("ServicesPage", () => {
  it("lists all account kinds independently of the selected application's vendor support", async () => {
    setup();
    expect(await screen.findByText("Example Relay")).toBeInTheDocument();
    expect(screen.getByText("Example Official")).toBeInTheDocument();
    expect(screen.getByText("loongport.accounts.relay")).toBeInTheDocument();
    expect(screen.getByText("loongport.accounts.vendor")).toBeInTheDocument();
    expect(mocks.vendors.mock.calls.map(([app]) => app)).toEqual(APP_IDS);
    expect(mocks.balance).toHaveBeenCalledWith({
      rowKind: "vendor",
      rowId: 1,
      enabled: true,
    });
    expect(
      mocks.balance.mock.calls.every(([props]) => props.rowKind === "vendor"),
    ).toBe(true);
  });

  it("opens only the chosen account's existing operations and returns to an updated global list", async () => {
    const { user } = setup();
    await screen.findByText("Example Official");
    await user.click(
      screen.getAllByRole("button", { name: "loongport.accounts.detail" })[1],
    );
    expect(screen.getByText("Account operations")).toBeInTheDocument();
    expect(mocks.detail).toHaveBeenLastCalledWith(
      expect.objectContaining({
        appId: "codex",
        accountFilter: { kind: "vendor", id: 1 },
      }),
    );
    mocks.vendors.mockResolvedValue({ supported: true, accounts: [] });
    await user.click(
      screen.getByRole("button", { name: "loongport.accounts.back" }),
    );
    await waitFor(() =>
      expect(screen.queryByText("Example Official")).not.toBeInTheDocument(),
    );
    expect(screen.getByText("Example Relay")).toBeInTheDocument();
  });

  it("navigates configured apps and the actual add flow", async () => {
    const { user, onOpenApp, onOpenAddHub } = setup();
    await screen.findByText("Example Official");
    await user.click(screen.getByRole("button", { name: "Codex" }));
    expect(onOpenApp).toHaveBeenCalledWith("codex");
    await user.click(
      screen.getByRole("button", { name: "loongport.accounts.add" }),
    );
    expect(onOpenAddHub).toHaveBeenCalledWith("directory");
  });
});
