import { QueryClientProvider } from "@tanstack/react-query";
import { createTestQueryClient } from "../utils/testQueryClient";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen as page,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import i18n from "i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthCenterPanel } from "@/components/settings/AuthCenterPanel";
import type {
  MiniMaxCodeAccount,
  MiniMaxCodeLogin,
} from "@/lib/api/minimaxCode";
import zh from "@/i18n/locales/zh.json";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  openExternal: vi.fn(),
  open: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));
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
let queryClient: ReturnType<typeof createTestQueryClient>;
let miniMax: ReturnType<typeof within>;
let catalog: MiniMaxCodeAccount[];
let handlers: Record<string, (payload: Record<string, string>) => unknown>;
const calls = (command: string) =>
  mocks.invoke.mock.calls.filter(([name]) => name === command);
const row = (id = "code-a") =>
  miniMax.getByRole("article", { name: `Code ${id}` });
const button = (name: string, id = "code-a") =>
  within(row(id)).getByRole("button", { name });
async function mount() {
  const view = render(
    <QueryClientProvider client={queryClient}>
      <section>
        <h2>MiniMax Code</h2>
        <AuthCenterPanel platform="minimax_code" />
      </section>
    </QueryClientProvider>,
  );
  // Keep assertions on the real MiniMax owner, in its single-platform services view.
  miniMax = within(
    page.getByRole("heading", { name: "MiniMax Code" }).closest("section")!,
  );
  await waitFor(() =>
    expect(miniMax.getByRole("button", { name: "重新读取账号" })).toBeEnabled(),
  );
  return view;
}

beforeEach(async () => {
  queryClient = createTestQueryClient();
  await i18n.changeLanguage("zh");
  i18n.addResourceBundle("zh", "translation", zh, true, true);
  catalog = [account()];
  handlers = {};
  mocks.invoke.mockReset();
  mocks.open
    .mockReset()
    .mockResolvedValue("/fixture/.minimax/auth/prod/cn/mcode-public/auth.json");
  mocks.openExternal.mockReset().mockResolvedValue(undefined);
  mocks.invoke.mockImplementation(
    async (command: string, payload: Record<string, string> = {}) => {
      if (
        [
          "list_trae_accounts",
          "list_workbuddy_accounts",
          "list_workbuddy_inference_accounts",
        ].includes(command)
      )
        return [];
      if (command === "get_zcode_account_library")
        return { revision: "test", pending: false, profiles: [] };
      if (handlers[command]) return handlers[command](payload);
      if (command === "get_automatic_claim_status")
        return {
          provider: payload.provider,
          phase: "waiting",
          lastCheckedAt: null,
          nextCheckAt: null,
          result: null,
          reason: null,
        };
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
  it("exposes the real entry and only reads catalogs on mount", async () => {
    await mount();
    expect(
      miniMax.getByRole("heading", { name: "MiniMax Code" }),
    ).toBeInTheDocument();
    expect(row()).toHaveTextContent("verified-code-a");
    expect(row()).toHaveTextContent("中国区");
    expect(mocks.invoke.mock.calls).toEqual([
      ["get_automatic_claim_status", { provider: "minimaxCode" }],
      ["list_minimax_code_accounts"],
      ["get_automatic_claim_status", { provider: "minimaxCode" }],
      ["list_minimax_code_accounts"],
    ]);
    expect(miniMax.queryByRole("textbox")).not.toBeInTheDocument();
    expect(miniMax.getByText(/Token Plan 和 API Key/)).toBeInTheDocument();
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
    fireEvent.click(miniMax.getByRole("button", { name: "重新读取账号" }));
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
    fireEvent.click(miniMax.getByRole("button", { name: "重新读取账号" }));
    await waitFor(() =>
      expect(miniMax.queryByRole("article")).not.toBeInTheDocument(),
    );
    expect(miniMax.getByRole("alert")).toHaveTextContent("解锁后重新读取");
    expect(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    ).toBeDisabled();
    expect(calls("begin_minimax_code_authorization")).toHaveLength(0);
  });

  it("never exposes raw errors or presents a rejected list as an empty ready catalog", async () => {
    handlers.list_minimax_code_accounts = () => {
      throw new Error("secret-token-in-error");
    };
    await mount();
    expect(miniMax.getByRole("alert")).toHaveTextContent("未能确认操作结果");
    expect(
      miniMax.queryByText(/secret-token-in-error/),
    ).not.toBeInTheDocument();
    expect(
      miniMax.queryByText("尚未添加 MiniMax Code 账号。"),
    ).not.toBeInTheDocument();
    expect(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    ).toBeDisabled();
  });

  it("uses a region-bound official device flow, never an API-key form", async () => {
    catalog = [];
    const user = userEvent.setup();
    await mount();
    await user.click(miniMax.getByRole("combobox", { name: "账号地区" }));
    await user.click(page.getByRole("option", { name: "国际区" }));
    await user.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    expect(await miniMax.findByText("TEST-CODE")).toBeInTheDocument();
    expect(calls("begin_minimax_code_authorization")).toEqual([
      ["begin_minimax_code_authorization", { region: "global" }],
    ]);
    await user.click(miniMax.getByRole("link", { name: "打开官方授权页" }));
    expect(mocks.openExternal).toHaveBeenCalledWith(flow.verificationUri);
    expect(miniMax.queryByRole("textbox")).not.toBeInTheDocument();
    await user.click(
      miniMax.getByRole("button", { name: "我已授权，检查登录" }),
    );
    expect(await miniMax.findByText(/尚未完成授权/)).toBeInTheDocument();
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
    fireEvent.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    await miniMax.findByText("TEST-CODE");
    fireEvent.click(
      miniMax.getByRole("button", { name: "我已授权，检查登录" }),
    );
    await waitFor(() => expect(row()).toBeInTheDocument());
    expect(
      miniMax.queryByRole("article", { name: "Code wrong-id" }),
    ).not.toBeInTheDocument();
    expect(miniMax.queryByText("TEST-CODE")).not.toBeInTheDocument();
    expect(calls("claim_minimax_code_today")).toHaveLength(0);
  });

  it("cancels a late begin result and does not expose its device code", async () => {
    const pending = deferred<MiniMaxCodeLogin>();
    handlers.begin_minimax_code_authorization = () => pending.promise;
    await mount();
    fireEvent.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    fireEvent.click(miniMax.getByRole("button", { name: "取消" }));
    await act(async () => pending.resolve(flow));
    expect(miniMax.queryByText("TEST-CODE")).not.toBeInTheDocument();
    expect(calls("cancel_minimax_code_authorization")).toEqual([
      ["cancel_minimax_code_authorization", { flowId: "synthetic-flow" }],
    ]);
    expect(calls("poll_minimax_code_authorization")).toHaveLength(0);
  });

  it("ignores a canceled late poll and clears authorization UI on lock", async () => {
    const pending = deferred<{ state: "saved"; account: MiniMaxCodeAccount }>();
    handlers.poll_minimax_code_authorization = () => pending.promise;
    await mount();
    fireEvent.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    await miniMax.findByText("TEST-CODE");
    fireEvent.click(
      miniMax.getByRole("button", { name: "我已授权，检查登录" }),
    );
    fireEvent.click(miniMax.getByRole("button", { name: "取消" }));
    await act(async () =>
      pending.resolve({ state: "saved", account: account("late-account") }),
    );
    expect(
      miniMax.queryByRole("article", { name: "Code late-account" }),
    ).not.toBeInTheDocument();
    fireEvent.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    await miniMax.findByText("TEST-CODE");
    handlers.list_minimax_code_accounts = () => {
      throw "storageUnavailable";
    };
    fireEvent.click(miniMax.getByRole("button", { name: "重新读取账号" }));
    await waitFor(() =>
      expect(miniMax.getByRole("alert")).toHaveTextContent("解锁后重新读取"),
    );
    expect(miniMax.queryByText("TEST-CODE")).not.toBeInTheDocument();
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
    fireEvent.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    await miniMax.findByText("TEST-CODE");
    fireEvent.click(
      miniMax.getByRole("button", { name: "我已授权，检查登录" }),
    );
    fireEvent.click(miniMax.getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(miniMax.getByRole("alert")).toHaveTextContent("授权已完成"),
    );
    await waitFor(() => expect(row()).toHaveTextContent("领取结果未确认"));
    expect(miniMax.getByRole("alert")).toHaveTextContent("本次取消未生效");
    expect(miniMax.queryByText("TEST-CODE")).not.toBeInTheDocument();
    expect(button("领取今日额度")).toBeDisabled();
    // The manager and provider consumer each read once on mount, then the manager rereads.
    expect(calls("list_minimax_code_accounts")).toHaveLength(3);
    await act(async () =>
      pending.resolve({ state: "saved", account: account("late-account") }),
    );
    expect(
      miniMax.queryByRole("article", { name: "Code late-account" }),
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
        miniMax.getByRole("button", { name: "添加账号或重新登录" }),
      );
      await miniMax.findByText("TEST-CODE");
      fireEvent.click(miniMax.getByRole("button", { name: "取消" }));
      await miniMax.findByRole("alert");
      expect(miniMax.getByRole("alert")).not.toHaveTextContent("授权已完成");
      expect(miniMax.getByRole("alert")).not.toHaveTextContent("已取消");
      // No reread beyond the manager and provider consumer mount reads.
      expect(calls("list_minimax_code_accounts")).toHaveLength(2);
      expect(miniMax.queryByRole("article")).not.toBeInTheDocument();
      expect(miniMax.queryByText("TEST-CODE")).not.toBeInTheDocument();
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
    fireEvent.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    await miniMax.findByText("TEST-CODE");
    fireEvent.click(miniMax.getByRole("button", { name: "取消" }));
    await waitFor(() =>
      expect(miniMax.getByRole("alert")).toHaveTextContent("解锁后重新读取"),
    );
    expect(miniMax.queryByText("TEST-CODE")).not.toBeInTheDocument();
    expect(miniMax.queryByRole("article")).not.toBeInTheDocument();
    expect(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    ).toBeDisabled();
  });

  it("does not attribute a late completed cancellation to a new region or device flow", async () => {
    const cancellation = deferred<void>();
    catalog = [];
    handlers.cancel_minimax_code_authorization = () => cancellation.promise;
    await mount();
    fireEvent.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    await miniMax.findByText("TEST-CODE");
    fireEvent.click(miniMax.getByRole("button", { name: "取消" }));
    const user = userEvent.setup();
    await user.click(miniMax.getByRole("combobox", { name: "账号地区" }));
    await user.click(page.getByRole("option", { name: "国际区" }));
    handlers.begin_minimax_code_authorization = () => ({
      ...flow,
      flowId: "new-global-flow",
      userCode: "NEW-CODE",
    });
    fireEvent.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    await miniMax.findByText("NEW-CODE");
    catalog = [unknown()];
    await act(async () => cancellation.reject("authorizationCompleted"));
    await waitFor(() => expect(row()).toHaveTextContent("领取结果未确认"));
    expect(miniMax.getByText("NEW-CODE")).toBeInTheDocument();
    expect(
      miniMax.getByRole("combobox", { name: "账号地区" }),
    ).toHaveTextContent("国际区");
    expect(miniMax.queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.click(
      miniMax.getByRole("button", { name: "我已授权，检查登录" }),
    );
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
    fireEvent.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    await miniMax.findByText("TEST-CODE");
    fireEvent.click(miniMax.getByRole("button", { name: "取消" }));
    const user = userEvent.setup();
    await user.click(miniMax.getByRole("combobox", { name: "账号地区" }));
    await user.click(page.getByRole("option", { name: "国际区" }));
    catalog = [account()];
    await act(async () => cancellation.reject("authorizationCompleted"));
    await waitFor(() => expect(row()).toBeInTheDocument());
    expect(miniMax.queryByRole("alert")).not.toBeInTheDocument();
    expect(
      miniMax.getByRole("combobox", { name: "账号地区" }),
    ).toHaveTextContent("国际区");
    expect(calls("begin_minimax_code_authorization")).toHaveLength(1);
  });

  it("cancels an active device flow on unmount", async () => {
    const view = await mount();
    fireEvent.click(
      miniMax.getByRole("button", { name: "添加账号或重新登录" }),
    );
    await miniMax.findByText("TEST-CODE");
    view.unmount();
    expect(calls("cancel_minimax_code_authorization")).toEqual([
      ["cancel_minimax_code_authorization", { flowId: "synthetic-flow" }],
    ]);
  });
});

it("exposes a staged import entry through the existing account workspace without reading credentials", async () => {
  await mount();
  fireEvent.click(
    miniMax.getByRole("button", { name: "导入本机 MiniMax CLI 账号" }),
  );
  expect(page.getByRole("dialog")).toHaveTextContent("导入公共 CLI 会话");
  expect(page.getByRole("button", { name: "读取本地预览" })).toBeDisabled();
  expect(
    page.getByRole("button", { name: "加密保存仅访问账号" }),
  ).toBeDisabled();
  expect(calls("preview_minimax_native_file")).toHaveLength(0);
  expect(calls("begin_minimax_code_authorization")).toHaveLength(0);
});

afterEach(() => {
  cleanup();
  queryClient.clear();
});

it("enables automatic claiming only for the chosen account without claiming immediately", async () => {
  catalog = [account("code-a"), account("code-b", { region: "global" })];
  handlers.set_minimax_code_automatic_claim = () => {
    catalog[0] = {
      ...catalog[0],
      autoClaimEnabled: true,
      autoClaimRevision: 1,
    };
    return catalog[0];
  };
  await mount();
  fireEvent.click(button("开启自动签到", "code-a"));
  await waitFor(() => expect(button("关闭自动签到", "code-a")).toBeEnabled());
  expect(button("开启自动签到", "code-b")).toBeEnabled();
  expect(calls("set_minimax_code_automatic_claim")).toEqual([
    [
      "set_minimax_code_automatic_claim",
      { id: "code-a", authGeneration: 1, policyRevision: 0, enabled: true },
    ],
  ]);
  expect(calls("claim_minimax_code_today")).toEqual([]);
});

it("re-reads the authoritative account catalog after native save and preserves automatic controls", async () => {
  catalog = [];
  const preview = {
    previewId: "synthetic-native",
    source: "publicCliOAuthV1",
    region: "cn",
    maskedSubject: null,
    expiresAtMs: 1900000000000,
    stage: "preview",
    verifiedUserId: null,
    label: null,
    canSave: false,
  };
  handlers.preview_minimax_native_file = () => preview;
  handlers.verify_minimax_native_import = () => ({
    ...preview,
    stage: "verified",
    verifiedUserId: "verified-code-a",
    label: "Code code-a",
    canSave: true,
  });
  handlers.save_minimax_native_import = () => {
    catalog = [
      account("code-a", {
        credentialKind: "nativeAccessOnly",
        credentialExpiresAt: 1900000000000,
        requiresReimport: false,
        canRefreshCredentials: false,
      }),
    ];
    return account("wrong-save-response");
  };
  await mount();
  fireEvent.click(
    miniMax.getByRole("button", { name: "导入本机 MiniMax CLI 账号" }),
  );
  fireEvent.click(page.getByRole("button", { name: "选择 auth.json" }));
  await page.findByDisplayValue(
    "/fixture/.minimax/auth/prod/cn/mcode-public/auth.json",
  );
  fireEvent.click(
    page.getByRole("checkbox", { name: "允许在本地读取所选文件" }),
  );
  fireEvent.click(page.getByRole("button", { name: "读取本地预览" }));
  await waitFor(() =>
    expect(
      page.getByRole("checkbox", {
        name: "允许将此账号的访问令牌发送至所选地区的 MiniMax 官方服务以验证账号资料",
      }),
    ).toBeEnabled(),
  );
  fireEvent.click(
    page.getByRole("checkbox", {
      name: "允许将此账号的访问令牌发送至所选地区的 MiniMax 官方服务以验证账号资料",
    }),
  );
  fireEvent.click(page.getByRole("button", { name: "联网验证账号" }));
  await page.findByText("verified-code-a");
  fireEvent.click(
    page.getByRole("checkbox", {
      name: "将此访问令牌的副本保存至 LoongPort 加密账号保管库",
    }),
  );
  fireEvent.click(page.getByRole("button", { name: "加密保存仅访问账号" }));
  await waitFor(() => expect(row()).toBeInTheDocument());
  expect(calls("list_minimax_code_accounts")).toHaveLength(3);
  expect(
    miniMax.queryByText("Code wrong-save-response"),
  ).not.toBeInTheDocument();
  expect(button("开启自动签到")).toBeEnabled();
  expect(calls("set_minimax_code_automatic_claim")).toHaveLength(0);
  expect(calls("claim_minimax_code_today")).toHaveLength(0);
  expect(mocks.openExternal).not.toHaveBeenCalled();
});
it("displays native access expiry and backend reimport capability without promising credential renewal", async () => {
  catalog = [
    account("code-a", {
      credentialKind: "nativeAccessOnly",
      credentialExpiresAt: 1900000000000,
      requiresReimport: true,
      canRefreshCredentials: false,
    }),
  ];
  await mount();
  expect(row()).toHaveTextContent("仅访问凭据，不会自动续期");
  expect(row()).toHaveTextContent("访问凭据到期：");
  expect(row()).toHaveTextContent("请在 CLI 重新登录，再明确重新导入此账号。");
});

it("reflects automatic completion through local reads without repeating a claim", async () => {
  const { emitTauriEvent } = await import("../msw/tauriMocks");
  await mount();
  catalog = [
    account("code-a", { status: "alreadyClaimed", autoClaimEnabled: true }),
  ];
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      provider: "workbuddy",
      phase: "waiting",
    }),
  );
  expect(calls("list_minimax_code_accounts")).toHaveLength(2);
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      provider: "minimaxCode",
      phase: "waiting",
    }),
  );
  await waitFor(() => expect(row()).toHaveTextContent("当前面板已领取"));
  expect(calls("refresh_minimax_code_account")).toHaveLength(0);
  expect(calls("claim_minimax_code_today")).toHaveLength(0);
  expect(calls("set_minimax_code_automatic_claim")).toHaveLength(0);
});

it("preserves explicitly refreshed entitlements only for the same identity on background reads", async () => {
  const { emitTauriEvent } = await import("../msw/tauriMocks");
  handlers.refresh_minimax_code_account = () =>
    account("code-a", {
      entitlement: {
        remainingCredits: "123.45",
        hasTokenPlan: true,
        tokenPlanExpiresAt: null,
      },
    });
  await mount();
  await userEvent.click(button("刷新状态"));
  await waitFor(() => expect(row()).toHaveTextContent("123.45"));
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      provider: "minimaxCode",
      phase: "waiting",
    }),
  );
  await waitFor(() =>
    expect(calls("list_minimax_code_accounts")).toHaveLength(4),
  );
  expect(row()).toHaveTextContent("123.45");
  catalog = [account("code-a", { authGeneration: 2, credentialVersion: 2 })];
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      provider: "minimaxCode",
      phase: "waiting",
    }),
  );
  await waitFor(() =>
    expect(calls("list_minimax_code_accounts")).toHaveLength(5),
  );
  await waitFor(() => expect(row()).not.toHaveTextContent("123.45"));
  expect(calls("refresh_minimax_code_account")).toHaveLength(1);
});

it("keeps a refresh projection when a background catalog read overtakes its pending catalog read", async () => {
  const { emitTauriEvent } = await import("../msw/tauriMocks");
  await mount();
  const manualRead = deferred<MiniMaxCodeAccount[]>();
  const passiveRead = deferred<MiniMaxCodeAccount[]>();
  let reads = 0;
  handlers.refresh_minimax_code_account = () =>
    account("code-a", {
      entitlement: {
        remainingCredits: "123.45",
        hasTokenPlan: true,
        tokenPlanExpiresAt: null,
      },
    });
  handlers.list_minimax_code_accounts = () =>
    ++reads === 1 ? manualRead.promise : passiveRead.promise;
  fireEvent.click(button("刷新状态"));
  await waitFor(() => expect(reads).toBe(1));
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      provider: "minimaxCode",
      phase: "waiting",
    }),
  );
  await waitFor(() => expect(reads).toBe(2));
  await act(async () => passiveRead.resolve([account()]));
  await act(async () => manualRead.resolve([account()]));
  expect(row()).toHaveTextContent("123.45");
  expect(calls("refresh_minimax_code_account")).toHaveLength(1);
  expect(calls("claim_minimax_code_today")).toHaveLength(0);
});

it.each([
  { authGeneration: 2 },
  { credentialVersion: 2 },
  { realUserId: "replacement-subject" },
  { region: "global" as const },
  { authorized: false, status: "needsLogin" as const },
  null,
])(
  "does not apply a pending entitlement after identity replacement or retirement: %j",
  async (replacement) => {
    const { emitTauriEvent } = await import("../msw/tauriMocks");
    await mount();
    const manualRead = deferred<MiniMaxCodeAccount[]>();
    const passiveRead = deferred<MiniMaxCodeAccount[]>();
    let reads = 0;
    handlers.refresh_minimax_code_account = () =>
      account("code-a", {
        entitlement: {
          remainingCredits: "123.45",
          hasTokenPlan: true,
          tokenPlanExpiresAt: null,
        },
      });
    handlers.list_minimax_code_accounts = () =>
      ++reads === 1 ? manualRead.promise : passiveRead.promise;
    fireEvent.click(button("刷新状态"));
    await waitFor(() => expect(reads).toBe(1));
    act(() =>
      emitTauriEvent("automatic-claim-status-updated", {
        provider: "minimaxCode",
        phase: "waiting",
      }),
    );
    await waitFor(() => expect(reads).toBe(2));
    await act(async () =>
      passiveRead.resolve(replacement ? [account("code-a", replacement)] : []),
    );
    await act(async () => manualRead.resolve([account()]));
    expect(miniMax.queryByText(/123\.45/)).not.toBeInTheDocument();
    expect(calls("claim_minimax_code_today")).toHaveLength(0);
  },
);
