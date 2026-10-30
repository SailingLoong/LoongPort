import { act, render, screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import i18n from "i18next";
import zh from "@/i18n/locales/zh.json";
import {
  accountPlatformPresenceKey,
  type PlatformPresenceSnapshot,
} from "../useAccountPlatformPresence";
import { ServicesPage } from "../ServicesPage";

const mocks = vi.hoisted(() => ({
  set: vi.fn(),
  zcodeSet: vi.fn(),
  zcodeRead: vi.fn(),
  read: vi.fn(),
  rows: [] as any[],
  unavailable: [] as string[],
}));
vi.mock("@/lib/api/zcodeClaim", () => ({
  zcodeClaimApi: { setAuto: mocks.zcodeSet, state: mocks.zcodeRead },
}));
vi.mock("@/lib/api/workbuddy", () => ({
  workbuddyApi: { setAutomatic: mocks.set },
}));
vi.mock("../useServiceAccounts", () => ({
  useServiceAccounts: () => ({ accounts: [], reload: vi.fn() }),
  configuredApps: () => [],
}));
vi.mock("../useAccountPlatformPresence", async (load) => ({
  ...(await load<typeof import("../useAccountPlatformPresence")>()),
  useAccountPlatformPresence: () => ({
    isPending: false,
    isFetching: false,
    refetch: mocks.read,
    data: {
      platforms: ["workbuddy", "github_copilot"],
      unavailable: mocks.unavailable,
      connections: [],
      accounts: mocks.rows,
    },
  }),
}));
vi.mock("../ServiceSourceList", () => ({ ServiceSourceList: () => null }));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
function row(id: string, enabled = false, canSetAutomatic = true) {
  return {
    id,
    label: `Synthetic ${id}`,
    source: "workbuddy",
    platform: "workbuddy",
    snapshot: {
      kind: "workbuddy",
      value: {
        id,
        generation: 2,
        autoClaimRevision: 3,
        autoClaimPolicyVersion: 1,
        autoClaimEnabled: enabled,
        canSetAutomatic,
        autoClaimBlocked: false,
        credits: { totalRemaining: 0 },
        claimState: "unconfirmed",
      },
    },
  };
}
beforeEach(() => {
  i18n.addResourceBundle(
    "zh",
    "translation",
    { claimSettings: zh.claimSettings, accountsOverview: zh.accountsOverview },
    true,
    true,
  );
  vi.clearAllMocks();
  mocks.unavailable = [];
  mocks.rows = [
    row("a"),
    row("b"),
    {
      id: "c",
      label: "Synthetic unsupported",
      platform: "github_copilot",
      source: "github_copilot",
    },
  ];
  mocks.read.mockImplementation(async () => ({
    data: {
      accounts: mocks.rows,
      unavailable: [],
      platforms: ["workbuddy", "github_copilot"],
      connections: [],
    },
  }));
  mocks.set.mockImplementation(async (value, enabled) => ({
    ...value,
    autoClaimEnabled: enabled,
    autoClaimRevision: value.autoClaimRevision + 1,
  }));
});
function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ServicesPage appId="codex" onOpenAddHub={vi.fn()} onOpenApp={vi.fn()} />
    </QueryClientProvider>,
  );
  return Object.assign(userEvent.setup(), { client });
}

it("shows explicit off and unavailable reasons without granting consent on read", () => {
  setup();
  const section = screen.getByRole("region", { name: "自动领取管理" });
  expect(within(section).getByText("自动领取未开启")).toBeVisible();
  expect(within(section).getByText(/当前范围：全部账号/)).toBeVisible();
  expect(within(section).getByText(/未提供自动领取设置/)).toBeVisible();
  expect(mocks.set).not.toHaveBeenCalled();
});
it("freezes the explicit scope, prevents duplicate writes, and reports partial failure separately", async () => {
  let complete!: (v: unknown) => void;
  mocks.set.mockImplementation((account) =>
    account.id === "a"
      ? new Promise((resolve) => {
          complete = resolve;
        })
      : Promise.reject(new Error("synthetic failure")),
  );
  const user = setup();
  const section = screen.getByRole("region", { name: "自动领取管理" });
  const button = within(section).getByRole("button", { name: "全部开启" });
  await user.click(button);
  expect(button).toBeDisabled();
  await user.click(button);
  mocks.rows.push(row("new"));
  await act(async () =>
    complete({
      ...mocks.rows[0].snapshot.value,
      autoClaimEnabled: true,
      autoClaimRevision: 4,
    }),
  );
  await waitFor(() =>
    expect(
      within(section).getByText(/Synthetic b.*设置结果未确认/),
    ).toBeVisible(),
  );
  await user.click(screen.getByRole("checkbox", { name: "隐藏已耗尽" }));
  const failedCard = screen
    .getByRole("button", { name: "查看 Synthetic b" })
    .closest("article")!;
  expect(within(failedCard).getByText(/设置结果未确认/)).toBeVisible();
  expect(
    within(failedCard).queryByText("自动领取未开启"),
  ).not.toBeInTheDocument();
  expect(mocks.set.mock.calls.map(([a]) => a.id).sort()).toEqual(["a", "b"]);
  expect(mocks.set.mock.calls.every(([, enabled]) => enabled === true)).toBe(
    true,
  );
});
it("filters the action scope and permits disabling an account that cannot be enabled", async () => {
  mocks.rows = [
    row("a", true, false),
    {
      id: "c",
      label: "Synthetic unsupported",
      platform: "github_copilot",
      source: "github_copilot",
    },
  ];
  const user = setup();
  await user.click(screen.getByRole("tab", { name: "WorkBuddy" }));
  const section = screen.getByRole("region", { name: "自动领取管理" });
  expect(within(section).getByText(/当前范围：WorkBuddy/)).toBeVisible();
  expect(
    within(section).getByText("全部开启", { selector: "span" }),
  ).toBeVisible();
  await user.click(within(section).getByRole("button", { name: "全部关闭" }));
  await waitFor(() => expect(mocks.set).toHaveBeenCalledTimes(1));
  expect(mocks.set).toHaveBeenCalledWith(mocks.rows[0].snapshot.value, false);
});
it("does not treat stale owner responses as confirmed policy changes", async () => {
  mocks.set.mockImplementation(async (account) => account);
  const user = setup();
  await user.click(
    within(screen.getByRole("region", { name: "自动领取管理" })).getByRole(
      "button",
      { name: "全部开启" },
    ),
  );
  await waitFor(() =>
    expect(screen.getByText(/Synthetic a.*设置结果未确认/)).toBeVisible(),
  );
});

it("keeps unknown consent distinct from a confirmed disabled setting", () => {
  mocks.rows = [row("legacy")];
  delete mocks.rows[0].snapshot.value.autoClaimEnabled;
  setup();
  const region = screen.getByRole("region", { name: "自动领取管理" });
  expect(within(region).queryByText("自动领取未开启")).not.toBeInTheDocument();
  expect(within(region).getByText("部分设置待确认")).toBeVisible();
});

it("keeps explicit close available after a lost enable response and uses the fresh owner revision", async () => {
  mocks.rows = [row("lost")];
  mocks.set.mockRejectedValue(new Error("Synthetic lost response"));
  const user = setup();
  const region = screen.getByRole("region", { name: "自动领取管理" });
  await user.click(within(region).getByRole("button", { name: "全部开启" }));
  await within(region).findByText(/Synthetic lost.*设置结果未确认/);
  const fresh = row("lost", true);
  fresh.snapshot.value.autoClaimRevision = 4;
  const freshData = {
    accounts: [fresh],
    unavailable: [],
    platforms: ["workbuddy" as const],
    connections: [],
  };
  user.client.setQueryData(accountPlatformPresenceKey, freshData);
  mocks.read
    .mockReset()
    .mockResolvedValueOnce({ data: freshData })
    .mockRejectedValueOnce(new Error("Synthetic readback failure"));
  mocks.set.mockImplementation(async (account, enabled) => ({
    ...account,
    autoClaimEnabled: enabled,
    autoClaimRevision: account.autoClaimRevision + 1,
  }));
  const close = within(region).getByRole("button", { name: "全部关闭" });
  expect(close).toBeEnabled();
  await user.click(close);
  await waitFor(() =>
    expect(mocks.set).toHaveBeenLastCalledWith(fresh.snapshot.value, false),
  );
  await waitFor(() =>
    expect(
      user.client.getQueryData<PlatformPresenceSnapshot>(
        accountPlatformPresenceKey,
      )?.accounts?.[0].snapshot?.value,
    ).toMatchObject({ autoClaimRevision: 5, autoClaimEnabled: false }),
  );
});

it("does not attach another platform's read failure to the selected scope", async () => {
  mocks.rows = [row("ready", true)];
  mocks.unavailable = ["openrouter"];
  const user = setup();
  await user.click(screen.getByRole("tab", { name: "WorkBuddy" }));
  const region = screen.getByRole("region", { name: "自动领取管理" });
  expect(
    within(region).getByText("全部开启", { selector: "span" }),
  ).toBeVisible();
  expect(within(region).queryByText("部分设置待确认")).not.toBeInTheDocument();
});

it("does not replace the clicked ZCode consent revision during the page refresh", async () => {
  const old = {
    id: "zcode-a",
    label: "Synthetic ZCode",
    source: "zcode",
    platform: "zcode",
    snapshot: {
      kind: "zcode",
      dataRoot: "synthetic-root",
      value: {
        consentRevision: "context:1",
        enabled: false,
        participants: [],
        records: {},
        busy: false,
      },
    },
  };
  const revoked = {
    ...old,
    snapshot: {
      ...old.snapshot,
      value: { ...old.snapshot.value, consentRevision: "context:2" },
    },
  };
  mocks.rows = [old];
  mocks.read.mockResolvedValue({
    data: {
      accounts: [revoked],
      unavailable: [],
      platforms: ["zcode"],
      connections: [],
    },
  });
  mocks.zcodeRead.mockResolvedValue(revoked.snapshot.value);
  mocks.zcodeSet.mockResolvedValue({
    ...revoked.snapshot.value,
    consentRevision: "context:3",
    enabled: true,
    participants: ["zcode-a"],
  });
  const user = setup();
  const region = screen.getByRole("region", { name: "自动领取管理" });
  await user.click(within(region).getByRole("button", { name: "全部开启" }));
  await within(region).findByText(/Synthetic ZCode.*设置结果未确认/);
  expect(mocks.zcodeSet).not.toHaveBeenCalled();
});
