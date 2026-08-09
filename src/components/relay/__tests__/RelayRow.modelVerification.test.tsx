import { fireEvent, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      (
        ({
          "loongport.modelVerification.title": "模型验证",
          "loongport.modelVerification.tierVerdict.suspicious": "模型存疑",
          "loongport.modelVerification.tierVerdict.anomaly": "模型异常",
          "loongport.tier.checkConnectivity": "检测连通",
          "loongport.tier.userEdited": "已手工维护",
          "loongport.tier.userEditedHint": "手工维护说明",
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

import { RelayRow } from "../RelayRow";

const tier = {
  providerId: "provider-a",
  appId: "codex" as const,
  groupName: "Pro",
  displayName: "Pro",
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
    balance: null,
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
  return { onVerifyTier, ...render(<RelayRow {...props} />) };
}

describe("RelayRow model verification", () => {
  it("shows the managed Codex action immediately after connectivity in the existing hover group", () => {
    const { onVerifyTier } = renderRow();

    const connectivity = screen.getByTitle("检测连通");
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
    const { rerender } = renderRow({
      relay: { ...relayWithTier({ userEdited: true }) },
      verificationVerdictForTier: () => "suspicious",
      isVerifyingTier: () => false,
    });
    expect(screen.getByText("已手工维护")).toBeInTheDocument();
    expect(screen.getByText("模型存疑")).toBeInTheDocument();

    rerender(
      <RelayRow
        {...rowProps({
          isVerifyingTier: () => true,
          verificationVerdictForTier: () => "anomaly",
        })}
      />,
    );
    const verify = screen.getByTitle("模型验证");
    expect(verify).toBeDisabled();
    expect(verify.querySelector(".animate-spin")).toBeInTheDocument();
    expect(screen.getByText("模型异常")).toBeInTheDocument();
  });

  it.each(["trusted", "inconclusive"] as const)(
    "does not render a problem label for %s",
    (verificationVerdict) => {
      renderRow({ verificationVerdictForTier: () => verificationVerdict });

      expect(screen.queryByText("模型存疑")).not.toBeInTheDocument();
      expect(screen.queryByText("模型异常")).not.toBeInTheDocument();
    },
  );
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
    balance: null,
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
