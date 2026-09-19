import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
import type { RelayRow } from "@/lib/api/relay";
import type { VendorAccountRow } from "@/lib/api/vendor";
import { ServicesPage } from "../ServicesPage";
import { useRowBusy } from "../../useRowBusy";

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
  RelaySection: (props: any) => {
    mocks.detail(props);
    return props.renderAccounts ? (
      props.renderAccounts(
        () => <div>Account controls</div>,
        () => null,
      )
    ) : (
      <div>Account operations</div>
    );
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
  localStorage.clear();
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

it("leaves back navigation to the shell for a routed account detail", async () => {
  const onBack = vi.fn();
  const onSelectAccount = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ServicesPage
        appId="codex"
        account={{ kind: "vendor", id: 1 }}
        onSelectAccount={onSelectAccount}
        onBack={onBack}
        onOpenAddHub={vi.fn()}
        onOpenApp={vi.fn()}
      />
    </QueryClientProvider>,
  );
  expect(
    screen.queryByRole("button", { name: "common.back" }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "loongport.accounts.back" }),
  ).not.toBeInTheDocument();
  expect(onBack).not.toHaveBeenCalled();
  expect(onSelectAccount).not.toHaveBeenCalled();
});

it("shows account details inline without a visibility toggle", async () => {
  setup();
  await screen.findByText("Example Official");
  // 账号信息常驻：标签（并进「label · app · 状态」一行）与余额直接可见，
  // 不再有「显示/隐藏账号信息」开关。
  expect(screen.getByText(/Official account/)).toBeInTheDocument();
  expect(screen.getByText("Account balance")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Codex" })).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "loongport.accounts.showDetails" }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "loongport.accounts.hideDetails" }),
  ).not.toBeInTheDocument();
});

describe("ServicesPage 账号卡活动状态行", () => {
  it("登录/导入进行中显示「正在导入」状态行，结束后消失", async () => {
    mocks.relays.mockResolvedValue([{ ...relay, id: 41 }]);
    mocks.vendors.mockResolvedValue({ supported: false, accounts: [] });
    let release!: () => void;
    function Probe() {
      const { run } = useRowBusy();
      return (
        <button
          onClick={() =>
            run(
              "login:41",
              () =>
                new Promise<void>((resolve) => {
                  release = resolve;
                }),
            )
          }
        >
          start
        </button>
      );
    }
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <ServicesPage
          appId="gemini"
          onOpenApp={() => {}}
          onOpenAddHub={() => {}}
        />
        <Probe />
      </QueryClientProvider>,
    );
    const user = userEvent.setup();
    await user.click(await screen.findByText("start"));
    expect(
      await screen.findByText("loongport.accounts.importingTiers"),
    ).toBeInTheDocument();
    release();
    await waitFor(() =>
      expect(
        screen.queryByText("loongport.accounts.importingTiers"),
      ).not.toBeInTheDocument(),
    );
  });

  it("失败后保留失败行，直到重试同 key 才清除", async () => {
    mocks.relays.mockResolvedValue([{ ...relay, id: 42 }]);
    mocks.vendors.mockResolvedValue({ supported: false, accounts: [] });
    function Probe() {
      const { run, fail } = useRowBusy();
      return (
        <>
          <button
            onClick={() =>
              run("provision:42", () => {
                fail("provision:42", "HTTP 500");
                return Promise.resolve();
              })
            }
          >
            run-and-fail
          </button>
          <button onClick={() => run("provision:42", () => Promise.resolve())}>
            retry
          </button>
        </>
      );
    }
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <ServicesPage
          appId="gemini"
          onOpenApp={() => {}}
          onOpenAddHub={() => {}}
        />
        <Probe />
      </QueryClientProvider>,
    );
    const user = userEvent.setup();
    await user.click(await screen.findByText("run-and-fail"));
    expect(
      await screen.findByText("loongport.accounts.importFailedLine"),
    ).toBeInTheDocument();
    await user.click(screen.getByText("retry"));
    await waitFor(() =>
      expect(
        screen.queryByText("loongport.accounts.importFailedLine"),
      ).not.toBeInTheDocument(),
    );
  });
});
