import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { UsageResult } from "@/types";

const api = vi.hoisted(() => ({
  relayBalance: vi.fn(),
  vendorBalance: vi.fn(),
}));

vi.mock("@/lib/api", () => ({ relayApi: { balance: api.relayBalance } }));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: { balance: api.vendorBalance },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) =>
      ({
        "usage.refreshUsage": "刷新用量",
        "usage.queryFailed": "查询失败",
        "usage.remaining": "剩余：",
        "usage.used": "已使用：",
        "usage.never": "从不",
        "usage.justNow": "刚刚",
        "loongport.row.purchaseHint": "去充值",
        "loongport.row.lowBalanceHint": "余额不足，去充值",
      })[key] ??
      options?.defaultValue ??
      key,
  }),
}));

import { createTestQueryClient } from "../../../../tests/utils/testQueryClient";

import { RowBalance } from "../RowBalance";

function renderWithQuery(ui: ReactElement) {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      {ui}
    </QueryClientProvider>,
  );
}

const wallet = (remaining: number): UsageResult => ({
  success: true,
  data: [{ planName: "钱包余额", remaining, unit: "USD" }],
});

beforeEach(() => {
  api.relayBalance.mockReset();
  api.vendorBalance.mockReset();
});

describe("RowBalance", () => {
  /**
   * ⭐ **这一轮改动的目的本身：登录态过期时余额仍然显示。**
   *
   * sk 与网页登录态是两份独立凭据，后端那条回落链的前两步只用 sk
   * （`src-tauri/src/relay/balance.rs`）。改造前这里判的是 `loggedIn` ——
   * 过期的行连查都不查，用户看到的是「密钥还能用，但余额和充值入口都不见了」。
   *
   * **会红的改法**：把 `enabled` 的判据从「登录过」改回「登录态还有效」
   * （即 `RelayRow` 传 `relay.loggedIn`）。
   */
  it("登录态过期的行照样查余额并显示出来", async () => {
    api.relayBalance.mockResolvedValue(wallet(42.5));

    // `enabled` 由调用方按「登录过」算：过期的行是 `loggedIn:false` +
    // `sessionExpired:true` ⇒ 仍然为 true。
    renderWithQuery(
      <RowBalance
        rowKind="relay"
        rowId={1}
        enabled
        onPurchase={vi.fn()}
        purchaseBusy={false}
      />,
    );

    expect(await screen.findByText("42.50")).toBeInTheDocument();
    expect(api.relayBalance).toHaveBeenCalledWith(1);
  });

  /**
   * ⭐ **查失败要留下重试入口，不能整块消失。**
   *
   * 这条守的正是改造前那个死路：余额由一个依赖 `id:accountLabel` 的 effect 拉，
   * 失败一次键不变 ⇒ 永不重试 ⇒ 那一行整个会话都没有余额，而充值按钮只在有余额
   * 时渲染 ⇒ 用户连入口都看不到。
   */
  it("查失败时渲染失败态 + 刷新按钮，点它会重查", async () => {
    api.relayBalance.mockResolvedValue({
      success: false,
      error: "三条路都查不到",
    } satisfies UsageResult);

    renderWithQuery(
      <RowBalance rowKind="relay" rowId={7} enabled onPurchase={vi.fn()} />,
    );

    expect(await screen.findByText("查询失败")).toBeInTheDocument();
    const refresh = screen.getByTitle("刷新用量");

    api.relayBalance.mockResolvedValue(wallet(3));
    await userEvent.click(refresh);

    expect(await screen.findByText("3.00")).toBeInTheDocument();
  });

  /**
   * ⭐ **低余额提醒不作用于官网行** —— 阈值是美元，DeepSeek 的钱包是人民币。
   *
   * 判据是「有没有充值入口」（只有中转站行传 `onPurchase`），
   * 见 `lowBalanceScopeContract.test.ts` 与 `lowBalance.ts` 的文档。
   */
  it("官网行没有充值按钮，低余额也不出琥珀叹号", async () => {
    // 1.5 在美元口径下是低余额；官网行不该因此变态。
    api.vendorBalance.mockResolvedValue(wallet(1.5));

    renderWithQuery(<RowBalance rowKind="vendor" rowId={1} enabled />);

    expect(await screen.findByText("1.50")).toBeInTheDocument();
    expect(screen.queryByTitle("去充值")).not.toBeInTheDocument();
    expect(screen.queryByTitle("余额不足，去充值")).not.toBeInTheDocument();
  });

  it("中转站行余额偏低时，充值按钮切到催充值的提示", async () => {
    api.relayBalance.mockResolvedValue(wallet(1.5));

    renderWithQuery(
      <RowBalance rowKind="relay" rowId={1} enabled onPurchase={vi.fn()} />,
    );

    expect(await screen.findByTitle("余额不足，去充值")).toBeInTheDocument();
  });

  it("从没登录过的行不查也不渲染", async () => {
    renderWithQuery(
      <RowBalance
        rowKind="relay"
        rowId={1}
        enabled={false}
        onPurchase={vi.fn()}
      />,
    );

    await waitFor(() => expect(api.relayBalance).not.toHaveBeenCalled());
    expect(screen.queryByTitle("刷新用量")).not.toBeInTheDocument();
  });
});
