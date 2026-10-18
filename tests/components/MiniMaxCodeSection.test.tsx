import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import i18n from "i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AuthCenterPanel } from "@/components/settings/AuthCenterPanel";
import type {
  MiniMaxCodeAccount,
  MiniMaxCodeLogin,
} from "@/lib/api/minimaxCode";
import zh from "@/i18n/locales/zh.json";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), openExternal: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/api/settings", () => ({
  settingsApi: { openExternal: mocks.openExternal },
}));
vi.mock("@/components/providers/forms/CopilotAuthSection", () => ({
  CopilotAuthSection: () => <div />,
}));
vi.mock("@/components/providers/forms/CodexOAuthSection", () => ({
  CodexOAuthSection: () => <div />,
}));
vi.mock("@/components/providers/forms/XaiOAuthSection", () => ({
  XaiOAuthSection: () => <div />,
}));
vi.mock("@/components/relay/RestoreOfficialLoginButton", () => ({
  RestoreOfficialLoginButton: () => <div />,
}));

const account = (
  id = "code-a",
  overrides: Partial<MiniMaxCodeAccount> = {},
): MiniMaxCodeAccount => ({
  id,
  label: `Code ${id}`,
  region: "cn",
  buildEnv: "prod",
  realUserId: `verified-${id}`,
  authGeneration: 1,
  credentialVersion: 1,
  authorized: true,
  status: "claimable",
  canClaim: true,
  canRefresh: true,
  canRecheck: false,
  canRetire: true,
  operation: null,
  lastCheckedAt: null,
  lastObservedStatus: null,
  creditAmount: null,
  ...overrides,
});
const unknown = (id = "code-a", overrides: Partial<MiniMaxCodeAccount> = {}) =>
  account(id, {
    status: "unknown",
    canClaim: false,
    canRecheck: true,
    operation: {
      attemptId: `attempt-${id}`,
      startedAt: 1780000000000,
      day: "server-round",
      status: "unknown",
      creditAmount: null,
    },
    lastCheckedAt: 1780000010000,
    lastObservedStatus: "alreadyClaimed",
    ...overrides,
  });
const flow: MiniMaxCodeLogin = {
  flowId: "synthetic-flow",
  verificationUri: "https://www.minimaxi.com/oauth/authorize",
  userCode: "TEST-CODE",
  expiresAt: 1800000000000,
  intervalSeconds: 5,
};
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
let catalog: MiniMaxCodeAccount[];
let handlers: Record<string, (payload: Record<string, string>) => unknown>;
const calls = (command: string) =>
  mocks.invoke.mock.calls.filter(([name]) => name === command);
const row = (id = "code-a") =>
  screen.getByRole("article", { name: `Code ${id}` });
const button = (name: string, id = "code-a") =>
  within(row(id)).getByRole("button", { name });
async function mount() {
  const view = render(<AuthCenterPanel />);
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "重新读取账号" })).toBeEnabled(),
  );
  return view;
}

beforeEach(async () => {
  await i18n.changeLanguage("zh");
  i18n.addResourceBundle("zh", "translation", zh, true, true);
  catalog = [account()];
  handlers = {};
  mocks.invoke.mockReset();
  mocks.openExternal.mockReset().mockResolvedValue(undefined);
  mocks.invoke.mockImplementation(
    async (command: string, payload: Record<string, string> = {}) => {
      if (handlers[command]) return handlers[command](payload);
      if (command === "list_minimax_code_accounts")
        return structuredClone(catalog);
      if (command === "begin_minimax_code_authorization") return flow;
      if (command === "poll_minimax_code_authorization")
        return { state: "waiting", account: null };
      if (command === "cancel_minimax_code_authorization") return;
      if (command === "retire_minimax_code_account") return;
      return catalog.find((item) => item.id === payload.id);
    },
  );
});

describe("MiniMax Code in the original authentication center", () => {
  it("exposes the real entry and only reads its own catalog on mount", async () => {
    await mount();
    expect(
      screen.getByRole("heading", { name: "MiniMax Code" }),
    ).toBeInTheDocument();
    expect(row()).toHaveTextContent("verified-code-a");
    expect(row()).toHaveTextContent("中国区");
    expect(mocks.invoke.mock.calls).toEqual([["list_minimax_code_accounts"]]);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByText(/Token Plan 和 API Key/)).toBeInTheDocument();
  });

  it("uses backend capabilities instead of inferring eligibility from a plan or status", async () => {
    catalog = [
      account("code-a", {
        status: "unavailable",
        canClaim: true,
        canRefresh: false,
      }),
    ];
    await mount();
    expect(button("领取今日额度")).toBeEnabled();
    expect(button("刷新状态")).toBeDisabled();
    expect(button("查证原操作")).toBeDisabled();
  });

  it("refreshes via its account command without claiming", async () => {
    await mount();
    fireEvent.click(button("刷新状态"));
    await waitFor(() =>
      expect(calls("refresh_minimax_code_account")).toEqual([
        ["refresh_minimax_code_account", { id: "code-a" }],
      ]),
    );
    await waitFor(() => expect(button("刷新状态")).toBeEnabled());
    expect(calls("claim_minimax_code_today")).toHaveLength(0);
  });

  it("sends only one claim for a double click and does not block another account", async () => {
    const pendingA = deferred<MiniMaxCodeAccount>();
    const pendingB = deferred<MiniMaxCodeAccount>();
    catalog = [account(), account("code-b", { region: "global" })];
    handlers.claim_minimax_code_today = ({ id }) =>
      id === "code-a" ? pendingA.promise : pendingB.promise;
    await mount();
    const claimA = button("领取今日额度");
    fireEvent.click(claimA);
    fireEvent.click(claimA);
    expect(calls("claim_minimax_code_today")).toEqual([
      ["claim_minimax_code_today", { id: "code-a" }],
    ]);
    expect(button("领取今日额度", "code-b")).toBeEnabled();
    fireEvent.click(button("领取今日额度", "code-b"));
    expect(calls("claim_minimax_code_today")).toHaveLength(2);
    catalog = [
      unknown(),
      account("code-b", {
        status: "claimed",
        canClaim: false,
        creditAmount: 12,
      }),
    ];
    await act(async () => pendingB.resolve(catalog[1]));
    expect(row()).toHaveTextContent("领取结果未确认");
    expect(row("code-b")).toHaveTextContent("领取结果已核验");
    await act(async () => pendingA.resolve(unknown()));
    expect(row()).not.toHaveTextContent("领取结果已核验");
    expect(row("code-b")).toHaveTextContent("12");
  });

  it("re-reads a failed claim, preserves Unknown, and uses only recheck for verification", async () => {
    handlers.claim_minimax_code_today = () => {
      catalog = [unknown()];
      throw "networkUnavailable";
    };
    await mount();
    fireEvent.click(button("领取今日额度"));
    await waitFor(() => expect(button("查证原操作")).toBeEnabled());
    expect(button("领取今日额度")).toBeDisabled();
    expect(row()).toHaveTextContent("attempt-code-a");
    expect(row()).toHaveTextContent(new Date(1780000000000).toLocaleString());
    expect(row()).toHaveTextContent(new Date(1780000010000).toLocaleString());
    expect(row()).toHaveTextContent("这不代表原操作已确认");
    fireEvent.click(button("查证原操作"));
    await waitFor(() =>
      expect(calls("recheck_minimax_code_claim")).toEqual([
        ["recheck_minimax_code_claim", { id: "code-a" }],
      ]),
    );
    await waitFor(() => expect(button("查证原操作")).toBeEnabled());
    expect(calls("claim_minimax_code_today")).toHaveLength(1);
    expect(row()).not.toHaveTextContent("领取结果已核验");
  });

  it("keeps an old pending operation visible across remounts without a date-based retry", async () => {
    catalog = [
      unknown("code-a", {
        operation: {
          attemptId: "old-pending",
          startedAt: 1,
          day: "old-round",
          status: "pending",
          creditAmount: null,
        },
      }),
    ];
    const view = await mount();
    expect(row()).toHaveTextContent("old-pending");
    view.unmount();
    await mount();
    expect(row()).toHaveTextContent("old-pending");
    expect(button("领取今日额度")).toBeDisabled();
    expect(calls("claim_minimax_code_today")).toHaveLength(0);
  });

  it("does not let a late response replace a new login or another account's projection", async () => {
    const pending = deferred<MiniMaxCodeAccount>();
    handlers.claim_minimax_code_today = () => pending.promise;
    await mount();
    fireEvent.click(button("领取今日额度"));
    catalog = [
      unknown("code-a", { authGeneration: 2, credentialVersion: 2 }),
      account("code-b"),
    ];
    fireEvent.click(screen.getByRole("button", { name: "重新读取账号" }));
    await waitFor(() => expect(row("code-b")).toBeInTheDocument());
    await act(async () =>
      pending.resolve(
        account("code-a", { status: "claimed", creditAmount: 999 }),
      ),
    );
    expect(row()).toHaveTextContent("领取结果未确认");
    expect(row()).not.toHaveTextContent("999");
    expect(row("code-b")).not.toHaveTextContent("999");
  });

  it("retirement displays the retained unresolved identity instead of deleting the pointer", async () => {
    catalog = [unknown()];
    handlers.retire_minimax_code_account = () => {
      catalog = [
        unknown("code-a", {
          authorized: false,
          canRefresh: false,
          canRecheck: false,
          canRetire: false,
        }),
      ];
    };
    await mount();
    fireEvent.click(button("停用账号"));
    await waitFor(() => expect(button("停用账号")).toBeDisabled());
    expect(row()).toHaveTextContent("attempt-code-a");
    expect(calls("retire_minimax_code_account")).toEqual([
      ["retire_minimax_code_account", { id: "code-a" }],
    ]);
    expect(calls("claim_minimax_code_today")).toHaveLength(0);
  });

  it("clears the projection and disables login when the encrypted catalog is locked", async () => {
    await mount();
    handlers.list_minimax_code_accounts = () => {
      throw "storageUnavailable";
    };
    fireEvent.click(screen.getByRole("button", { name: "重新读取账号" }));
    await waitFor(() =>
      expect(screen.queryByRole("article")).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("alert")).toHaveTextContent("解锁后重新读取");
    expect(
      screen.getByRole("button", { name: "添加账号或重新登录" }),
    ).toBeDisabled();
    expect(calls("begin_minimax_code_authorization")).toHaveLength(0);
  });

  it("never exposes raw errors or presents a rejected list as an empty ready catalog", async () => {
    handlers.list_minimax_code_accounts = () => {
      throw new Error("secret-token-in-error");
    };
    await mount();
    expect(screen.getByRole("alert")).toHaveTextContent("未能确认操作结果");
    expect(screen.queryByText(/secret-token-in-error/)).not.toBeInTheDocument();
    expect(
      screen.queryByText("尚未添加 MiniMax Code 账号。"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "添加账号或重新登录" }),
    ).toBeDisabled();
  });

  it("uses a region-bound official device flow, never an API-key form", async () => {
    catalog = [];
    const user = userEvent.setup();
    await mount();
    await user.click(screen.getByRole("combobox", { name: "账号地区" }));
    await user.click(screen.getByRole("option", { name: "国际区" }));
    await user.click(
      screen.getByRole("button", { name: "添加账号或重新登录" }),
    );
    expect(await screen.findByText("TEST-CODE")).toBeInTheDocument();
    expect(calls("begin_minimax_code_authorization")).toEqual([
      ["begin_minimax_code_authorization", { region: "global" }],
    ]);
    await user.click(screen.getByRole("link", { name: "打开官方授权页" }));
    expect(mocks.openExternal).toHaveBeenCalledWith(flow.verificationUri);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "我已授权，检查登录" }),
    );
    expect(await screen.findByText(/尚未完成授权/)).toBeInTheDocument();
    expect(calls("poll_minimax_code_authorization")).toEqual([
      ["poll_minimax_code_authorization", { flowId: "synthetic-flow" }],
    ]);
    expect(calls("claim_minimax_code_today")).toHaveLength(0);
  });

  it("reads saved login accounts from the catalog without trusting the poll response", async () => {
    catalog = [];
    handlers.poll_minimax_code_authorization = () => {
      catalog = [account()];
      return { state: "saved", account: account("wrong-id") };
    };
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "添加账号或重新登录" }));
    await screen.findByText("TEST-CODE");
    fireEvent.click(screen.getByRole("button", { name: "我已授权，检查登录" }));
    await waitFor(() => expect(row()).toBeInTheDocument());
    expect(
      screen.queryByRole("article", { name: "Code wrong-id" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("TEST-CODE")).not.toBeInTheDocument();
    expect(calls("claim_minimax_code_today")).toHaveLength(0);
  });

  it("cancels a late begin result and does not expose its device code", async () => {
    const pending = deferred<MiniMaxCodeLogin>();
    handlers.begin_minimax_code_authorization = () => pending.promise;
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "添加账号或重新登录" }));
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    await act(async () => pending.resolve(flow));
    expect(screen.queryByText("TEST-CODE")).not.toBeInTheDocument();
    expect(calls("cancel_minimax_code_authorization")).toEqual([
      ["cancel_minimax_code_authorization", { flowId: "synthetic-flow" }],
    ]);
    expect(calls("poll_minimax_code_authorization")).toHaveLength(0);
  });

  it("ignores a canceled late poll and clears authorization UI on lock", async () => {
    const pending = deferred<{ state: "saved"; account: MiniMaxCodeAccount }>();
    handlers.poll_minimax_code_authorization = () => pending.promise;
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "添加账号或重新登录" }));
    await screen.findByText("TEST-CODE");
    fireEvent.click(screen.getByRole("button", { name: "我已授权，检查登录" }));
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    await act(async () =>
      pending.resolve({ state: "saved", account: account("late-account") }),
    );
    expect(
      screen.queryByRole("article", { name: "Code late-account" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "添加账号或重新登录" }));
    await screen.findByText("TEST-CODE");
    handlers.list_minimax_code_accounts = () => {
      throw "storageUnavailable";
    };
    fireEvent.click(screen.getByRole("button", { name: "重新读取账号" }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("解锁后重新读取"),
    );
    expect(screen.queryByText("TEST-CODE")).not.toBeInTheDocument();
  });

  it("reports a completed authorization when durable save wins cancellation and re-reads the catalog", async () => {
    const pending = deferred<{ state: "saved"; account: MiniMaxCodeAccount }>();
    catalog = [];
    handlers.poll_minimax_code_authorization = () => pending.promise;
    handlers.cancel_minimax_code_authorization = () => {
      catalog = [unknown()];
      throw "authorizationCompleted";
    };
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "添加账号或重新登录" }));
    await screen.findByText("TEST-CODE");
    fireEvent.click(screen.getByRole("button", { name: "我已授权，检查登录" }));
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("授权已完成"),
    );
    await waitFor(() => expect(row()).toHaveTextContent("领取结果未确认"));
    expect(screen.getByRole("alert")).toHaveTextContent("本次取消未生效");
    expect(screen.queryByText("TEST-CODE")).not.toBeInTheDocument();
    expect(button("领取今日额度")).toBeDisabled();
    expect(calls("list_minimax_code_accounts")).toHaveLength(2);
    await act(async () =>
      pending.resolve({ state: "saved", account: account("late-account") }),
    );
    expect(
      screen.queryByRole("article", { name: "Code late-account" }),
    ).not.toBeInTheDocument();
    expect(calls("claim_minimax_code_today")).toHaveLength(0);
  });

  it.each(["expiredAuthorization", "unrecognized-result"])(
    "does not treat %s from cancellation as a completed authorization",
    async (error) => {
      catalog = [];
      handlers.cancel_minimax_code_authorization = () => {
        throw error;
      };
      await mount();
      fireEvent.click(
        screen.getByRole("button", { name: "添加账号或重新登录" }),
      );
      await screen.findByText("TEST-CODE");
      fireEvent.click(screen.getByRole("button", { name: "取消" }));
      await screen.findByRole("alert");
      expect(screen.getByRole("alert")).not.toHaveTextContent("授权已完成");
      expect(screen.getByRole("alert")).not.toHaveTextContent("已取消");
      expect(calls("list_minimax_code_accounts")).toHaveLength(1);
      expect(screen.queryByRole("article")).not.toBeInTheDocument();
      expect(screen.queryByText("TEST-CODE")).not.toBeInTheDocument();
    },
  );

  it("fails closed if the catalog locks after a confirmed authorization completion", async () => {
    catalog = [];
    handlers.cancel_minimax_code_authorization = () => {
      handlers.list_minimax_code_accounts = () => {
        throw "storageUnavailable";
      };
      throw { code: "authorizationCompleted" };
    };
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "添加账号或重新登录" }));
    await screen.findByText("TEST-CODE");
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("解锁后重新读取"),
    );
    expect(screen.queryByText("TEST-CODE")).not.toBeInTheDocument();
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "添加账号或重新登录" }),
    ).toBeDisabled();
  });

  it("does not attribute a late completed cancellation to a new region or device flow", async () => {
    const cancellation = deferred<void>();
    catalog = [];
    handlers.cancel_minimax_code_authorization = () => cancellation.promise;
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "添加账号或重新登录" }));
    await screen.findByText("TEST-CODE");
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    const user = userEvent.setup();
    await user.click(screen.getByRole("combobox", { name: "账号地区" }));
    await user.click(screen.getByRole("option", { name: "国际区" }));
    handlers.begin_minimax_code_authorization = () => ({
      ...flow,
      flowId: "new-global-flow",
      userCode: "NEW-CODE",
    });
    fireEvent.click(screen.getByRole("button", { name: "添加账号或重新登录" }));
    await screen.findByText("NEW-CODE");
    catalog = [unknown()];
    await act(async () => cancellation.reject("authorizationCompleted"));
    await waitFor(() => expect(row()).toHaveTextContent("领取结果未确认"));
    expect(screen.getByText("NEW-CODE")).toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: "账号地区" }),
    ).toHaveTextContent("国际区");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "我已授权，检查登录" }));
    await waitFor(() =>
      expect(calls("poll_minimax_code_authorization")).toEqual([
        ["poll_minimax_code_authorization", { flowId: "new-global-flow" }],
      ]),
    );
    expect(calls("claim_minimax_code_today")).toHaveLength(0);
  });

  it("does not label a changed region as completed when the old cancellation arrives late", async () => {
    const cancellation = deferred<void>();
    catalog = [];
    handlers.cancel_minimax_code_authorization = () => cancellation.promise;
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "添加账号或重新登录" }));
    await screen.findByText("TEST-CODE");
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    const user = userEvent.setup();
    await user.click(screen.getByRole("combobox", { name: "账号地区" }));
    await user.click(screen.getByRole("option", { name: "国际区" }));
    catalog = [account()];
    await act(async () => cancellation.reject("authorizationCompleted"));
    await waitFor(() => expect(row()).toBeInTheDocument());
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: "账号地区" }),
    ).toHaveTextContent("国际区");
    expect(calls("begin_minimax_code_authorization")).toHaveLength(1);
  });

  it("cancels an active device flow on unmount", async () => {
    const view = await mount();
    fireEvent.click(screen.getByRole("button", { name: "添加账号或重新登录" }));
    await screen.findByText("TEST-CODE");
    view.unmount();
    expect(calls("cancel_minimax_code_authorization")).toEqual([
      ["cancel_minimax_code_authorization", { flowId: "synthetic-flow" }],
    ]);
  });
});
