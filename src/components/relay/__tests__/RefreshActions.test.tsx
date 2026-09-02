import { render, screen } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import userEvent from "@testing-library/user-event";
import type { ComponentProps, ReactElement } from "react";
import { describe, expect, it, vi } from "vitest";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "loongport.row.refreshAll":
          "更新账号信息、额度、可用分组、倍率和接入配置",
        "loongport.vendor.refreshAll": "更新账号信息、额度、密钥配置和当前状态",
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
        "loongport.row.removeInUseHint": "无法删除",
        "loongport.row.notLoggedIn": "未登录",
        "loongport.row.login": "登录",
        "loongport.row.sessionExpired": "登录已过期",
        "loongport.row.reLogin": "重新登录",
        "loongport.row.noTiers": "没有可用分组",
        "loongport.row.getKeys": "创建接入配置",
        "loongport.row.tierCount": "{{count}} 个接入配置",
        "loongport.row.sessionExpiredUsable":
          "登录已过期，但 API 密钥仍可使用（余额也照常显示）。点击重新登录。",
        "loongport.vendor.noKey": "尚未配置 API 密钥",
        "loongport.vendor.keyReady": "已创建接入配置",
      })[key] ?? key,
  }),
}));

import { createTestQueryClient } from "../../../../tests/utils/testQueryClient";

import { RelayRow } from "../RelayRow";
import { VendorRow } from "../VendorRow";

// 这组测试只守两类行把统一刷新回调接到用量区，不重复验证余额查询本身。
// `RowBalance.test.tsx` 已覆盖真实查询、布局和刷新顺序；这里隔离掉异步查询，
// 避免行组件测试被 React Query 的加载态绑住。
vi.mock("../RowBalance", () => ({
  RowBalance: ({
    enabled,
    onPurchase,
    onRefresh,
    refreshBusy,
    refreshLabel,
  }: {
    enabled: boolean;
    onPurchase?: () => void | Promise<void>;
    onRefresh?: () => void | Promise<void>;
    refreshBusy?: boolean;
    refreshLabel?: string;
  }) => (
    <>
      {onPurchase ? (
        <button type="button" aria-label="purchase" onClick={onPurchase}>
          purchase
        </button>
      ) : null}
      {enabled ? (
        <button
          type="button"
          aria-label={refreshLabel ?? "refresh"}
          title={refreshLabel}
          disabled={refreshBusy}
          onClick={onRefresh}
        >
          refresh
        </button>
      ) : null}
    </>
  ),
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
  canVerifyModels: true,
  userEdited: false,
  allowImageGeneration: false,
  siteDeclaredOrigin: null,
};

function renderRelayRow(
  onProvision = vi.fn(),
  relayOverrides: Partial<ComponentProps<typeof RelayRow>["relay"]> = {},
) {
  const onLogin = vi.fn();
  const props: ComponentProps<typeof RelayRow> = {
    relay: {
      id: 1,
      siteOrigin: "https://relay.example",
      siteName: "Relay",
      accountLabel: "account",
      status: "ready",
      isCurrent: false,
      canQueryBalance: true,
      canPurchase: true,
      canViewUsage: false,
      canRefresh: true,
      usageBlockers: [],
      removeConfirmation: "configured",
      tiers: [tier],
      ...relayOverrides,
    },
    open: true,
    onOpenChange: vi.fn(),
    busy: new Set(),
    onLogin,
    onProvision,
    onSiteConfigApplied: vi.fn(),
    onSwitchTier: vi.fn(),
    onSelectTierModel: vi.fn(),
    onPurchase: vi.fn(),
    onOpenUsage: undefined,
    onCheckTier: vi.fn(),
    isCheckingTier: () => false,
    onResetTier: vi.fn(),
    onEditTier: vi.fn(),
    onDelete: vi.fn(),
    onVerifyTier: vi.fn(),
    verificationVerdictForTier: () => undefined,
    isVerifyingTier: () => false,
  };

  return { onProvision, onLogin, ...renderWithQuery(<RelayRow {...props} />) };
}

function renderVendorRow(
  onProvision = vi.fn(),
  accountOverrides: Partial<ComponentProps<typeof VendorRow>["account"]> = {},
) {
  const props: ComponentProps<typeof VendorRow> = {
    account: {
      id: 1,
      vendorId: "deepseek",
      vendorName: "DeepSeek",
      accountLabel: "account",
      status: "ready",
      canQueryBalance: true,
      canRefresh: true,
      canDelete: true,
      plans: [
        {
          planId: "deepseek",
          planName: "DeepSeek",
          providerId: "provider-a",
          isCurrent: false,
          userEdited: false,
          canEditConfig: true,
          canSwitch: true,
        },
      ],
      ...accountOverrides,
    },
    busy: new Set(),
    onLogin: vi.fn(),
    onProvision,
    onUse: vi.fn(),
    onDelete: vi.fn(),
    onEdit: vi.fn(),
    onReset: vi.fn(),
  };

  return { onProvision, ...renderWithQuery(<VendorRow {...props} />) };
}

/** 两类行都只保留用量区里的图标入口，并通过 tooltip 说明完整刷新范围。 */
describe("行上的刷新动作", () => {
  it("后端不允许购买时不展示购买入口", () => {
    renderRelayRow(vi.fn(), { canPurchase: false });

    expect(
      screen.queryByRole("button", { name: "purchase" }),
    ).not.toBeInTheDocument();
  });

  it("中转站行只保留用量区的统一刷新入口", async () => {
    const user = userEvent.setup();
    const { onProvision } = renderRelayRow();

    const refetch = screen.getByRole("button", {
      name: "更新账号信息、额度、可用分组、倍率和接入配置",
    });
    expect(refetch).toBeInTheDocument();
    expect(screen.queryByText("更新可用分组")).not.toBeInTheDocument();

    await user.click(refetch);
    expect(onProvision).toHaveBeenCalledTimes(1);
  });

  it("官网行只保留用量区的统一刷新入口", async () => {
    const user = userEvent.setup();
    const { onProvision } = renderVendorRow();

    const reprovision = screen.getByRole("button", {
      name: "更新账号信息、额度、密钥配置和当前状态",
    });
    expect(reprovision).toBeInTheDocument();
    expect(screen.queryByText("重新应用密钥配置")).not.toBeInTheDocument();

    await user.click(reprovision);
    expect(onProvision).toHaveBeenCalledTimes(1);
  });

  it("后端不允许删除时不展示官网账号删除入口", () => {
    renderVendorRow(vi.fn(), { canDelete: false });

    expect(
      screen.queryByRole("button", { name: "loongport.vendor.remove" }),
    ).not.toBeInTheDocument();
  });

  it("当前档位所在的中转站复用浅蓝当前态外框", () => {
    renderRelayRow(vi.fn(), {
      isCurrent: true,
      tiers: [{ ...tier, isCurrent: true }],
    });

    const row = screen.getByText("Relay").closest(".rounded-xl");
    expect(row).toHaveClass(
      "border-blue-500/60",
      "shadow-sm",
      "shadow-blue-500/10",
    );
  });

  /**
   * ⭐ 登录态过期**不等于**分组和密钥失效。
   *
   * 后端探到会话失效时只清会话（`creds::clear_session`），账号身份、分组与 sk
   * 全都留着 ⇒ 这一行照样能切档位。所以这种行不该退化成「登录已过期」那个空壳，
   * 而要照常显示档位数 + 一条重新登录的路。
   *
   * 「不摆更新可用分组」同样要钉住：重拉分组必须有登录态，摆一个点下去必然报错的
   * 按钮不如不摆。
   */
  it("登录过期但仍有档位时，显示档位数与重新登录，而不是「没有可用分组」", () => {
    renderRelayRow(vi.fn(), {
      status: "sessionExpiredUsable",
      canQueryBalance: true,
      canRefresh: false,
    });

    expect(screen.getByTitle("{{count}} 个接入配置")).toHaveTextContent("1");
    expect(screen.queryByText("没有可用分组")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", {
        name: "更新账号信息、额度、可用分组、倍率和接入配置",
      }),
    ).not.toBeInTheDocument();

    const reLogin = screen.getByRole("button", { name: "重新登录" });
    expect(reLogin).toHaveAttribute(
      "title",
      "登录已过期，但 API 密钥仍可使用（余额也照常显示）。点击重新登录。",
    );
  });

  it("未登录但后端确认有 SK 时，仍展示余额刷新入口", () => {
    const { onProvision } = renderRelayRow(vi.fn(), {
      status: "notLoggedIn",
      canQueryBalance: true,
      canRefresh: false,
    });

    expect(screen.getByRole("button", { name: "refresh" })).toBeInTheDocument();
    expect(screen.getByText("未登录")).toBeInTheDocument();
    expect(onProvision).not.toHaveBeenCalled();
  });
});
