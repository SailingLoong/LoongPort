import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { ServicesPage } from "../ServicesPage";
import { useState } from "react";
import { PreservedView } from "@/components/ui/PreservedView";

const mocks = vi.hoisted(() => ({ refresh: vi.fn(), reload: vi.fn() }));
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
  AuthCenterPanel: () => <div>Original account manager</div>,
}));
vi.mock("../ServiceSourceList", () => ({ ServiceSourceList: () => null }));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));

beforeEach(() => {
  vi.clearAllMocks();
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
  await user.click(screen.getByRole("tab", { name: "全部" }));
  expect(
    screen.getByRole("button", { name: "查看 Other account" }),
  ).toBeVisible();
});

it("shows four facts without converting a successful claim into inference or balance", () => {
  setup();
  const card = screen.getByRole("button", { name: "查看 Example account" });
  for (const label of ["授权", "额度 / 积分", "自动领取", "推理消费"])
    expect(within(card).getByText(label)).toBeVisible();
  expect(within(card).getByText("未验证")).toBeVisible();
  expect(within(card).getByText("关闭")).toBeVisible();
  expect(within(card).queryByText(/999|已接通|已连接/)).not.toBeInTheDocument();
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
