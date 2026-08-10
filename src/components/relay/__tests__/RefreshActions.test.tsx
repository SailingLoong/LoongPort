import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "loongport.row.more": "更多",
        "loongport.row.refetchGroups": "更新可用分组",
        "loongport.vendor.refreshKey": "重新应用密钥配置",
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
        "loongport.tier.switching": "切换中...",
        "loongport.row.dragHandle": "拖动排序",
        "loongport.row.collapse": "收起",
        "loongport.row.expand": "展开",
        "loongport.row.remove": "删除",
        "loongport.row.removeBlockedByCurrent": "无法删除",
        "loongport.row.notLoggedIn": "未登录",
        "loongport.row.login": "登录",
        "loongport.row.sessionExpired": "登录已过期",
        "loongport.row.reLogin": "重新登录",
        "loongport.row.noTiers": "没有可用分组",
        "loongport.row.getKeys": "创建接入配置",
        "loongport.row.tierCount": "{{count}} 个接入配置",
        "loongport.vendor.sessionExpiredUsable":
          "登录已过期，但 API 密钥仍可使用；余额暂时无法查询。点击重新登录。",
        "loongport.vendor.noKey": "尚未配置 API 密钥",
        "loongport.vendor.keyReady": "已创建接入配置",
      })[key] ?? key,
  }),
}));

import { RelayRow } from "../RelayRow";
import { VendorRow } from "../VendorRow";

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

function renderRelayRow(onProvision = vi.fn()) {
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
    onProvision,
    onSwitchTier: vi.fn(),
    onSelectTierModel: vi.fn(),
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
  };

  return { onProvision, ...render(<RelayRow {...props} />) };
}

function renderVendorRow(onProvision = vi.fn()) {
  const props: ComponentProps<typeof VendorRow> = {
    account: {
      id: 1,
      vendorId: "deepseek",
      vendorName: "DeepSeek",
      accountLabel: "account",
      loggedIn: true,
      sessionExpired: false,
      keyReady: true,
      providerId: "provider-a",
      isCurrent: false,
      userEdited: false,
    },
    busy: new Set(),
    balance: null,
    isCurrent: false,
    onLogin: vi.fn(),
    onProvision,
    onUse: vi.fn(),
    onDelete: vi.fn(),
    onEdit: vi.fn(),
    onReset: vi.fn(),
  };

  return { onProvision, ...render(<VendorRow {...props} />) };
}

describe("refresh actions", () => {
  it("puts relay group refetch behind a clearly labeled more menu", async () => {
    const user = userEvent.setup();
    const { onProvision } = renderRelayRow();

    expect(screen.getByRole("button", { name: "更多" })).toBeInTheDocument();
    expect(screen.queryByTitle("更新可用分组")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "更多" }));
    const refetch = screen.getByRole("menuitem", {
      name: "更新可用分组",
    });
    expect(refetch).toBeInTheDocument();

    await user.click(refetch);
    expect(onProvision).toHaveBeenCalledTimes(1);
  });

  it("puts vendor key reprovision behind the same more menu", async () => {
    const user = userEvent.setup();
    const { onProvision } = renderVendorRow();

    expect(screen.getByRole("button", { name: "更多" })).toBeInTheDocument();
    expect(screen.queryByTitle("重新应用密钥配置")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "更多" }));
    const reprovision = screen.getByRole("menuitem", {
      name: "重新应用密钥配置",
    });
    expect(reprovision).toBeInTheDocument();

    await user.click(reprovision);
    expect(onProvision).toHaveBeenCalledTimes(1);
  });
});
