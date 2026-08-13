import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import type { ComponentProps, ReactElement } from "react";
import { describe, expect, it, vi } from "vitest";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      (
        ({
          "loongport.modelVerification.title": "模型验证",
          "loongport.modelVerification.tierVerdict.trusted": "验证通过",
          "loongport.modelVerification.tierVerdict.suspicious": "需要复核",
          "loongport.modelVerification.tierVerdict.anomaly": "检测到异常",
          "loongport.modelVerification.tierVerdict.trustedHint":
            "现有证据验证通过。打开“模型验证”查看详情。",
          "loongport.modelVerification.tierVerdict.suspiciousHint":
            "现有结果需要人工复核。打开“模型验证”查看详情。",
          "loongport.modelVerification.tierVerdict.anomalyHint":
            "检测到响应不一致。打开“模型验证”查看详情。",
          "loongport.tier.checkConnectivity": "测试连接",
          "loongport.tier.userEdited": "手动维护",
          "loongport.tier.userEditedHint": "此接入配置包含手动修改。",
          "loongport.tier.rate": "{{value}} 倍",
          "loongport.tier.rateUnknown": "倍率未知",
          "loongport.tier.edit": "编辑配置",
          "loongport.tier.resetConfig": "恢复默认配置",
          "provider.enable": "启用",
          "provider.inUse": "使用中",
          "loongport.row.dragHandle": "拖动排序",
          "loongport.row.collapse": "收起",
          "loongport.row.expand": "展开",
          "loongport.row.remove": "删除",
          "loongport.row.removeBlockedByCurrent": "无法删除",
        }) as Record<string, string>
      )[key] ?? key,
  }),
}));

import { createTestQueryClient } from "../../../../tests/utils/testQueryClient";

import { RelayRow } from "../RelayRow";

// 行内的 `RowBalance` 用 react-query 拉余额 ⇒ 得有 provider。余额不是这些闸关心
// 的东西，让 `invoke` reject 即可（行会渲染失败态的用量条，不影响其它断言）。
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => {
    throw new Error("balance not stubbed in this test");
  }),
}));

function renderWithQuery(ui: ReactElement) {
  return render(
    <QueryClientProvider client={createTestQueryClient()}>
      {ui}
    </QueryClientProvider>,
  );
}

const tier = {
  providerId: "provider-a",
  appId: "codex" as const,
  groupName: "Pro",
  displayName: "Pro",
  model: "gpt-5.6-sol",
  models: [],
  rateMultiplier: 1,
  isCurrent: false,
  userEdited: false,
  allowImageGeneration: false,
};

function renderRow(overrides: Partial<ComponentProps<typeof RelayRow>> = {}) {
  const onVerifyTier = vi.fn();
  const props: ComponentProps<typeof RelayRow> = {
    relay: {
      id: 1,
      siteOrigin: "https://relay.example",
      siteName: "Relay",
      accountLabel: "account",
      loggedIn: true,
      sessionExpired: false,
      tiers: [tier],
    },
    open: true,
    onOpenChange: vi.fn(),
    busy: new Set(),
    onLogin: vi.fn(),
    onProvision: vi.fn(),
    onSwitchTier: vi.fn(),
    onSelectTierModel: vi.fn(),
    onPurchase: vi.fn(),
    onCheckTier: vi.fn(),
    isCheckingTier: () => false,
    onResetTier: vi.fn(),
    onEditTier: vi.fn(),
    onDelete: vi.fn(),
    onVerifyTier,
    verificationVerdictForTier: () => undefined,
    isVerifyingTier: () => false,
    ...overrides,
  };
  return { onVerifyTier, ...renderWithQuery(<RelayRow {...props} />) };
}

describe("RelayRow model verification", () => {
  it("shows the managed Codex action immediately after connectivity in the existing hover group", () => {
    const { onVerifyTier } = renderRow();

    const connectivity = screen.getByTitle("测试连接");
    const verify = screen.getByTitle("模型验证");
    expect(connectivity.nextElementSibling).toBe(verify);
    expect(verify.parentElement?.className).toContain(
      "group-hover/tier:opacity-100",
    );

    fireEvent.click(verify);
    fireEvent.click(verify);
    expect(onVerifyTier).toHaveBeenCalledTimes(2);
    expect(onVerifyTier).toHaveBeenLastCalledWith(tier);
  });

  it("does not expose verification for unmanaged app tiers", () => {
    renderRow({
      relay: {
        id: 1,
        siteOrigin: "https://relay.example",
        siteName: "Relay",
        accountLabel: "account",
        loggedIn: true,
        sessionExpired: false,
        tiers: [{ ...tier, appId: "gemini" }],
      },
    });

    expect(screen.queryByTitle("模型验证")).not.toBeInTheDocument();
  });

  it("pins a spinner for the running tier and keeps problem labels independent from manual maintenance", () => {
    const onVerifyTier = vi.fn();
    const { rerender } = renderRow({
      relay: { ...relayWithTier({ userEdited: true }) },
      verificationVerdictForTier: () => "suspicious",
      isVerifyingTier: () => false,
    });
    expect(screen.getByText("手动维护")).toBeInTheDocument();
    expect(screen.getByText("需要复核")).toBeInTheDocument();
    expect(
      screen.getByTitle("现有结果需要人工复核。打开“模型验证”查看详情。"),
    ).toBeInTheDocument();

    rerender(
      <QueryClientProvider client={createTestQueryClient()}>
        <RelayRow
          {...rowProps({
            onVerifyTier,
            isVerifyingTier: () => true,
            verificationVerdictForTier: () => "anomaly",
          })}
        />
      </QueryClientProvider>,
    );
    const verify = screen.getByTitle("模型验证");
    expect(verify).toBeEnabled();
    fireEvent.click(verify);
    expect(onVerifyTier).toHaveBeenCalledWith(
      expect.objectContaining({ providerId: "provider-a" }),
    );
    expect(verify.querySelector(".animate-spin")).toBeInTheDocument();
    expect(screen.getByText("检测到异常")).toBeInTheDocument();
    expect(
      screen.getByTitle("检测到响应不一致。打开“模型验证”查看详情。"),
    ).toBeInTheDocument();
  });

  it("shows a success-colored label for a verified model", () => {
    renderRow({ verificationVerdictForTier: () => "trusted" });

    const label = screen.getByText("验证通过").closest("span");
    expect(label).toHaveClass("text-emerald-600");
    expect(
      screen.getByTitle("现有证据验证通过。打开“模型验证”查看详情。"),
    ).toBeInTheDocument();
    expect(screen.queryByText("需要复核")).not.toBeInTheDocument();
  });

  it("does not render a tier label for an inconclusive result", () => {
    renderRow({ verificationVerdictForTier: () => "inconclusive" });

    expect(screen.queryByText("验证通过")).not.toBeInTheDocument();
    expect(screen.queryByText("需要复核")).not.toBeInTheDocument();
    expect(screen.queryByText("检测到异常")).not.toBeInTheDocument();
  });
});

function relayWithTier(tierOverrides: Partial<typeof tier>) {
  return {
    id: 1,
    siteOrigin: "https://relay.example",
    siteName: "Relay",
    accountLabel: "account",
    loggedIn: true,
    sessionExpired: false,
    tiers: [{ ...tier, ...tierOverrides }],
  };
}

function rowProps(
  overrides: Partial<ComponentProps<typeof RelayRow>> = {},
): ComponentProps<typeof RelayRow> {
  return {
    relay: relayWithTier({ userEdited: true }),
    open: true,
    onOpenChange: vi.fn(),
    busy: new Set(),
    onLogin: vi.fn(),
    onProvision: vi.fn(),
    onSwitchTier: vi.fn(),
    onSelectTierModel: vi.fn(),
    onPurchase: vi.fn(),
    onCheckTier: vi.fn(),
    isCheckingTier: () => false,
    onResetTier: vi.fn(),
    onEditTier: vi.fn(),
    onDelete: vi.fn(),
    onVerifyTier: vi.fn(),
    verificationVerdictForTier: () => undefined,
    isVerifyingTier: () => false,
    ...overrides,
  };
}
