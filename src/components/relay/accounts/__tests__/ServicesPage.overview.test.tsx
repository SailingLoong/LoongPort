import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { ServicesPage } from "../ServicesPage";
import i18n from "i18next";
import zh from "@/i18n/locales/zh.json";
import { useEffect, useState } from "react";
import { ACCOUNT_OVERVIEW_PREFERENCES_KEY } from "../accountOverviewPreferences";
import { PreservedView } from "@/components/ui/PreservedView";

const mocks = vi.hoisted(() => ({
  refresh: vi.fn(),
  reload: vi.fn(),
  stopped: vi.fn(),
  extra: [] as any[],
  authorization: Promise.resolve(),
}));
vi.mock("../useServiceAccounts", () => ({
  useServiceAccounts: () => ({ accounts: [], reload: mocks.reload }),
  configuredApps: () => [],
}));
vi.mock("../useAccountPlatformPresence", () => ({
  useAccountPlatformPresence: () => ({
    isPending: false,
    isFetching: false,
    refetch: mocks.refresh,
    data: {
      platforms: ["workbuddy", "minimax_code"],
      unavailable: [],
      connections: [],
      accounts: [
        ...mocks.extra,
        {
          source: "workbuddy",
          platform: "workbuddy",
          id: "wb",
          label: "Example account",
          snapshot: {
            kind: "workbuddy",
            value: {
              claimState: "claimed",
              credited: 999,
              lastSuccessfulClaim: {
                confirmedAtMs: Date.now() - 1000,
                timeSource: "localReconciled",
                credited: 20,
                unit: "points",
              },
              creditExpiryStatus: "known",
              nextKnownCreditExpiry: {
                expiresAtMs: Date.now() + 86400000,
                remaining: 60,
                unit: "points",
              },
              autoClaimEnabled: false,
              credits: { totalRemaining: null },
            },
          },
        },
        {
          source: "minimax_code",
          platform: "minimax_code",
          id: "mm",
          label: "Other account",
          identity: "CN",
          snapshot: {
            kind: "minimax",
            value: { authorized: false, status: "unknown" },
          },
        },
      ],
    },
  }),
}));
vi.mock("@/components/settings/AuthCenterPanel", () => ({
  AuthCenterPanel: () => {
    const [draft, setDraft] = useState("");
    const [status, setStatus] = useState("Waiting for synthetic authorization");
    useEffect(() => {
      let observing = true;
      void mocks.authorization.then(() => {
        if (observing) setStatus("Synthetic authorization finished");
      });
      return () => {
        observing = false;
        mocks.stopped();
      };
    }, []);
    return (
      <div>
        Original account manager
        <input
          aria-label="Authorization draft"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
        />
        <span>{status}</span>
      </div>
    );
  },
}));
vi.mock("../ServiceSourceList", () => ({ ServiceSourceList: () => null }));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));

beforeEach(() => {
  vi.clearAllMocks();
  i18n.addResourceBundle("zh", "translation", zh, true, true);
  localStorage.removeItem(ACCOUNT_OVERVIEW_PREFERENCES_KEY);
  mocks.extra = [];
  mocks.authorization = Promise.resolve();
  mocks.refresh.mockResolvedValue(undefined);
  mocks.reload.mockResolvedValue(undefined);
});
function setup() {
  const onOpenAddHub = vi.fn();
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ServicesPage
        appId="codex"
        onOpenApp={vi.fn()}
        onOpenAddHub={onOpenAddHub}
      />
    </QueryClientProvider>,
  );
  return { user: userEvent.setup(), onOpenAddHub };
}

it("defaults to a platform account tab and separates relay accounts", async () => {
  const { user } = setup();
  expect(screen.getByRole("tab", { name: "平台账号" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(screen.getByText("2 个账号")).toBeVisible();
  await user.click(screen.getByRole("tab", { name: "中转站" }));
  expect(
    screen.queryByRole("button", { name: "查看 Example account" }),
  ).not.toBeInTheDocument();
});

it("filters summaries repeatedly without opening an account manager", async () => {
  const { user } = setup();
  await user.click(screen.getByRole("tab", { name: "WorkBuddy" }));
  expect(
    screen.getByRole("button", { name: "查看 Example account" }),
  ).toBeVisible();
  expect(
    screen.queryByRole("button", { name: "查看 Other account" }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText("Original account manager"),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "全部账号" }));
  expect(
    screen.getByRole("button", { name: "查看 Other account" }),
  ).toBeVisible();
});

it("shows four facts without converting a successful claim into inference or balance", () => {
  setup();
  const card = screen
    .getByRole("button", { name: "查看 Example account" })
    .closest("article")!;
  for (const label of ["授权", "额度 / 积分", "自动领取", "推理消费"])
    expect(within(card).getByText(label)).toBeVisible();
  expect(within(card).getByText("未验证")).toBeVisible();
  expect(within(card).getByText("自动领取未开启")).toBeVisible();
  expect(within(card).getByText(/本地确认时间.*\+20 积分/)).toBeVisible();
  expect(within(card).getByText(/剩余 60 积分/)).toBeVisible();
  expect(within(card).queryByText(/\+999/)).not.toBeInTheDocument();
  expect(within(card).queryByText(/已接通|已连接/)).not.toBeInTheDocument();
});

it("serializes repeated refresh actions until the owner reads finish", async () => {
  let finish!: () => void;
  mocks.refresh.mockImplementation(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  const { user } = setup();
  const button = screen.getByRole("button", { name: "刷新状态" });
  await user.click(button);
  expect(button).toBeDisabled();
  await user.click(button);
  expect(mocks.refresh).toHaveBeenCalledTimes(1);
  await act(async () => finish());
  expect(button).toBeEnabled();
});

it("keeps unified add repeatable and account actions outside the platform filter", async () => {
  const { user, onOpenAddHub } = setup();
  await user.click(screen.getByRole("button", { name: "添加账号" }));
  await user.click(screen.getByRole("button", { name: "添加账号" }));
  expect(onOpenAddHub).toHaveBeenCalledTimes(2);
  expect(
    screen.queryByRole("button", { name: /删除平台|隐藏平台|移除平台/ }),
  ).not.toBeInTheDocument();
});

it("restores focus to the unified add trigger after returning from the add page", async () => {
  function Shell() {
    const [adding, setAdding] = useState(false);
    return (
      <>
        <PreservedView active={!adding}>
          <ServicesPage
            appId="codex"
            onOpenApp={vi.fn()}
            onOpenAddHub={() => setAdding(true)}
          />
        </PreservedView>
        {adding && (
          <button onClick={() => setAdding(false)}>Return from add</button>
        )}
      </>
    );
  }
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Shell />
    </QueryClientProvider>,
  );
  const user = userEvent.setup();
  const trigger = screen.getByRole("button", { name: "添加账号" });
  await user.click(trigger);
  await user.click(screen.getByRole("button", { name: "Return from add" }));
  expect(trigger).toHaveFocus();
});

it("restores focus after opening add from an account manager", async () => {
  function Shell() {
    const [adding, setAdding] = useState(false);
    return (
      <>
        <PreservedView active={!adding}>
          <ServicesPage
            appId="codex"
            platform="workbuddy"
            onOpenApp={vi.fn()}
            onOpenAddHub={() => setAdding(true)}
          />
        </PreservedView>
        {adding && (
          <button onClick={() => setAdding(false)}>
            Return from manager add
          </button>
        )}
      </>
    );
  }
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Shell />
    </QueryClientProvider>,
  );
  const user = userEvent.setup();
  const trigger = screen.getByRole("button", { name: "添加账号" });
  await user.click(trigger);
  await user.click(
    screen.getByRole("button", { name: "Return from manager add" }),
  );
  expect(trigger).toHaveFocus();
});

const appOrder = () =>
  within(screen.getByRole("tablist", { name: "平台账号筛选" }))
    .getAllByRole("tab")
    .map((tab) => tab.getAttribute("aria-label"));
it("shares App order between the real tab menu and group menu without changing selection", async () => {
  const { user } = setup();
  await user.click(screen.getByRole("button", { name: "App 排序" }));
  await user.click(screen.getByRole("menuitem", { name: "WorkBuddy：左移" }));
  expect(appOrder()).toEqual(["全部账号", "WorkBuddy", "MiniMax Code 国内版"]);
  expect(
    screen
      .getAllByRole("region", { name: /账号总览/ })
      .map((group) => group.getAttribute("aria-label")),
  ).toEqual(["WorkBuddy · 账号总览", "MiniMax Code 国内版 · 账号总览"]);
  await user.click(screen.getByRole("button", { name: "WorkBuddy 排序" }));
  await user.click(screen.getByRole("menuitem", { name: "WorkBuddy：下移" }));
  expect(appOrder()).toEqual(["全部账号", "MiniMax Code 国内版", "WorkBuddy"]);
  expect(screen.getByRole("tab", { name: "全部账号" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(
    screen.queryByText("Original account manager"),
  ).not.toBeInTheDocument();
  expect(mocks.refresh).not.toHaveBeenCalled();
});
it("defaults to hiding only confirmed exhausted rows and exposes independent row actions", async () => {
  mocks.extra = [0, null].map((balance, i) => ({
    source: "workbuddy",
    platform: "workbuddy",
    id: `extra${i}`,
    label: i ? "Unknown balance" : "Exhausted balance",
    snapshot: {
      kind: "workbuddy",
      value: { credits: { totalRemaining: balance } },
    },
  }));
  const { user } = setup();
  expect(screen.getByRole("checkbox", { name: "隐藏已耗尽" })).toBeChecked();
  expect(
    screen.queryByRole("button", { name: "查看 Exhausted balance" }),
  ).not.toBeInTheDocument();
  const details = screen.getByRole("button", { name: "查看 Unknown balance" });
  const row = details.closest("article")!;
  expect(row).not.toBeNull();
  expect(details.parentElement?.closest("button")).toBeNull();
  expect(within(row).getByRole("button", { name: "切换" })).toBeDisabled();
  expect(within(row).getByText("当前尚无可验证的客户端切换能力")).toBeVisible();
  await user.click(screen.getByRole("checkbox", { name: "隐藏已耗尽" }));
  expect(
    screen.getByRole("button", { name: "查看 Exhausted balance" }),
  ).toBeVisible();
});
it("keeps the original manager and its pending authorization alive through collapse and filtering", async () => {
  let complete!: () => void;
  mocks.authorization = new Promise<void>((resolve) => {
    complete = resolve;
  });
  const { user } = setup();
  await user.click(
    screen.getByRole("button", { name: "查看 Example account" }),
  );
  await user.type(
    screen.getByRole("textbox", { name: "Authorization draft" }),
    "Retained draft",
  );
  await user.click(screen.getByRole("button", { name: "返回账号" }));
  await user.click(screen.getByRole("button", { name: "折叠 WorkBuddy" }));
  await user.click(screen.getByRole("tab", { name: "MiniMax Code 国内版" }));
  await user.click(screen.getByRole("tab", { name: "全部账号" }));
  expect(mocks.stopped).not.toHaveBeenCalled();
  await act(async () => complete());
  await user.click(screen.getByRole("button", { name: "展开 WorkBuddy" }));
  await user.click(
    screen.getByRole("button", { name: "查看 Example account" }),
  );
  expect(
    screen.getByRole("textbox", { name: "Authorization draft" }),
  ).toHaveValue("Retained draft");
  expect(screen.getByText("Synthetic authorization finished")).toBeVisible();
  expect(mocks.stopped).not.toHaveBeenCalled();
});

it("keeps ordinary tab arrow keys as navigation instead of display-order changes", async () => {
  const { user } = setup();
  const before = appOrder();
  const all = screen.getByRole("tab", { name: "全部账号" });
  all.focus();
  await user.keyboard("{ArrowRight}");
  expect(
    screen.getByRole("tab", { name: "MiniMax Code 国内版" }),
  ).toHaveAttribute("aria-selected", "true");
  expect(appOrder()).toEqual(before);
  expect(localStorage.getItem(ACCOUNT_OVERVIEW_PREFERENCES_KEY)).toBeNull();
  expect(
    screen.queryByText("Original account manager"),
  ).not.toBeInTheDocument();
});

it("keeps both order surfaces usable for this session when persistence fails", async () => {
  const { user } = setup();
  const failure = vi
    .spyOn(Storage.prototype, "setItem")
    .mockImplementation(() => {
      throw new Error("synthetic quota failure");
    });
  try {
    await user.click(screen.getByRole("button", { name: "App 排序" }));
    await user.click(screen.getByRole("menuitem", { name: "WorkBuddy：左移" }));
    expect(appOrder()).toEqual([
      "全部账号",
      "WorkBuddy",
      "MiniMax Code 国内版",
    ]);
    expect(screen.getByText("偏好仅在本次会话生效，未能保存。")).toBeVisible();
    expect(failure).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "WorkBuddy 排序" }));
    await user.click(screen.getByRole("menuitem", { name: "WorkBuddy：下移" }));
    expect(appOrder()).toEqual([
      "全部账号",
      "MiniMax Code 国内版",
      "WorkBuddy",
    ]);
    expect(failure).toHaveBeenCalledTimes(2);
  } finally {
    failure.mockRestore();
  }
});
