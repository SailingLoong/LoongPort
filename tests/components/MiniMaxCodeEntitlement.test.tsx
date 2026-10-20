import "@testing-library/jest-dom/vitest";
import { QueryClientProvider } from "@tanstack/react-query";
import { createTestQueryClient } from "../utils/testQueryClient";
import {
  cleanup,
  render,
  screen as page,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import i18n from "i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MiniMaxCodeSection } from "@/components/providers/forms/MiniMaxCodeSection";
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
const flow: MiniMaxCodeLogin = {
  flowId: "synthetic-flow",
  verificationUri: "https://www.minimaxi.com/oauth/authorize",
  userCode: "TEST-CODE",
  expiresAt: 1800000000000,
  intervalSeconds: 5,
};
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
        <MiniMaxCodeSection />
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

afterEach(() => {
  cleanup();
  queryClient.clear();
});

describe("MiniMax informational entitlement", () => {
  it("shows remaining credits separately from sign-in awards after explicit refresh", async () => {
    catalog = [account("code-a", { creditAmount: 200 })];
    handlers.refresh_minimax_code_account = () =>
      account("code-a", {
        creditAmount: 200,
        entitlement: {
          remainingCredits: "23.50",
          hasTokenPlan: false,
          tokenPlanExpiresAt: null,
        },
      } as Partial<MiniMaxCodeAccount>);
    await mount();
    await userEvent.click(button("刷新状态"));
    await waitFor(() =>
      expect(within(row()).getByText("剩余积分：23.50")).toBeInTheDocument(),
    );
    expect(within(row()).getByText("已核验领取额度：200")).toBeInTheDocument();
    expect(within(row()).getByText("Token Plan：未开通")).toBeInTheDocument();
    await userEvent.click(
      miniMax.getByRole("button", { name: "重新读取账号" }),
    );
    await waitFor(() =>
      expect(
        within(row()).queryByText("剩余积分：23.50"),
      ).not.toBeInTheDocument(),
    );
  });
  it("does not attach a late entitlement to a reauthorized account", async () => {
    handlers.refresh_minimax_code_account = () => {
      catalog = [account("code-a", { authGeneration: 2 })];
      return account("code-a", {
        entitlement: {
          remainingCredits: "23.50",
          hasTokenPlan: false,
          tokenPlanExpiresAt: null,
        },
      } as Partial<MiniMaxCodeAccount>);
    };
    await mount();
    await userEvent.click(button("刷新状态"));
    await waitFor(() =>
      expect(calls("list_minimax_code_accounts").length).toBeGreaterThan(1),
    );
    expect(
      within(row()).queryByText("剩余积分：23.50"),
    ).not.toBeInTheDocument();
  });
});

describe("MiniMax entitlement freshness", () => {
  it.each([
    { credentialVersion: 2 },
    { realUserId: "different-user" },
    { region: "global" as const },
  ])("does not attach a mismatched snapshot %j", async (change) => {
    handlers.refresh_minimax_code_account = () => {
      catalog = [account("code-a", change)];
      return account("code-a", {
        entitlement: {
          remainingCredits: "23.50",
          hasTokenPlan: true,
          tokenPlanExpiresAt: null,
        },
      });
    };
    await mount();
    await userEvent.click(button("刷新状态"));
    await waitFor(() =>
      expect(calls("list_minimax_code_accounts").length).toBeGreaterThan(1),
    );
    expect(
      within(row()).queryByText("剩余积分：23.50"),
    ).not.toBeInTheDocument();
  });
  it("keeps missing entitlement unknown and claim eligible", async () => {
    handlers.refresh_minimax_code_account = () =>
      account("code-a", { entitlement: null });
    await mount();
    await userEvent.click(button("刷新状态"));
    await waitFor(() => expect(button("领取今日额度")).toBeEnabled());
    expect(
      within(row()).getByText("剩余积分：未知（刷新查询）"),
    ).toBeInTheDocument();
    expect(within(row()).queryByText("剩余积分：0")).not.toBeInTheDocument();
  });
});

describe("MiniMax Code OAuth plan quota", () => {
  it("shows independent five-hour and weekly windows, zero and reset time", async () => {
    const reset = 1790000000000;
    handlers.refresh_minimax_code_account = () =>
      account("code-a", {
        entitlement: {
          remainingCredits: "23.50",
          hasTokenPlan: true,
          tokenPlanExpiresAt: null,
          quotaState: "available",
          quota: {
            fiveHour: {
              remainingPercent: 30,
              resetAtMs: reset,
              unlimited: false,
            },
            weekly: { remainingPercent: 0, resetAtMs: null, unlimited: false },
          },
        },
      });
    await mount();
    await userEvent.click(button("刷新状态"));
    await waitFor(() =>
      expect(
        within(row()).getByText("5 小时额度：剩余 30%"),
      ).toBeInTheDocument(),
    );
    expect(within(row()).getByText("每周额度：剩余 0%")).toBeInTheDocument();
    expect(
      within(row()).getByText(`重置时间：${new Date(reset).toLocaleString()}`),
    ).toBeInTheDocument();
    expect(within(row()).getByText("剩余积分：23.50")).toBeInTheDocument();
  });
  it("shows unavailable as unknown rather than zero and keeps claims enabled", async () => {
    handlers.refresh_minimax_code_account = () =>
      account("code-a", {
        entitlement: {
          remainingCredits: "17",
          hasTokenPlan: true,
          tokenPlanExpiresAt: null,
          quotaState: "unavailable",
          quota: null,
        },
      });
    await mount();
    await userEvent.click(button("刷新状态"));
    await waitFor(() =>
      expect(within(row()).getByText("剩余积分：17")).toBeInTheDocument(),
    );
    expect(
      within(row()).getByText("5 小时额度：未知（刷新查询）"),
    ).toBeInTheDocument();
    expect(
      within(row()).queryByText("5 小时额度：剩余 0%"),
    ).not.toBeInTheDocument();
    expect(button("领取今日额度")).toBeEnabled();
  });
  it("distinguishes unlimited, missing weekly value and not subscribed", async () => {
    handlers.refresh_minimax_code_account = () =>
      account("code-a", {
        entitlement: {
          remainingCredits: null,
          hasTokenPlan: true,
          tokenPlanExpiresAt: null,
          quotaState: "available",
          quota: {
            fiveHour: {
              remainingPercent: null,
              resetAtMs: null,
              unlimited: true,
            },
            weekly: {
              remainingPercent: null,
              resetAtMs: null,
              unlimited: false,
            },
          },
        },
      });
    await mount();
    await userEvent.click(button("刷新状态"));
    await waitFor(() =>
      expect(within(row()).getByText("5 小时额度：不限量")).toBeInTheDocument(),
    );
    expect(
      within(row()).getByText("每周额度：未知（刷新查询）"),
    ).toBeInTheDocument();
    handlers.refresh_minimax_code_account = () =>
      account("code-a", {
        entitlement: {
          remainingCredits: null,
          hasTokenPlan: false,
          tokenPlanExpiresAt: null,
          quotaState: "not-subscribed",
          quota: null,
        },
      });
    await userEvent.click(button("刷新状态"));
    await waitFor(() =>
      expect(
        within(row()).getByText("Token Plan 用量：未订阅"),
      ).toBeInTheDocument(),
    );
    expect(
      within(row()).queryByText("5 小时额度：不限量"),
    ).not.toBeInTheDocument();
  });
  it("does not display quota from an old credential generation", async () => {
    handlers.refresh_minimax_code_account = () => {
      catalog = [account("code-a", { credentialVersion: 2 })];
      return account("code-a", {
        entitlement: {
          remainingCredits: null,
          hasTokenPlan: true,
          tokenPlanExpiresAt: null,
          quotaState: "available",
          quota: {
            fiveHour: {
              remainingPercent: 99,
              resetAtMs: null,
              unlimited: false,
            },
            weekly: { remainingPercent: 99, resetAtMs: null, unlimited: false },
          },
        },
      });
    };
    await mount();
    await userEvent.click(button("刷新状态"));
    await waitFor(() =>
      expect(calls("list_minimax_code_accounts").length).toBeGreaterThan(1),
    );
    expect(
      within(row()).queryByText("5 小时额度：剩余 99%"),
    ).not.toBeInTheDocument();
  });
});
