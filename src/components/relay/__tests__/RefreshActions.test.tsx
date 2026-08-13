import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
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
        "loongport.row.sessionExpiredUsable":
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
      loggedIn: true,
      sessionExpired: false,
      tiers: [tier],
      ...relayOverrides,
    },
    open: true,
    onOpenChange: vi.fn(),
    busy: new Set(),
    onLogin,
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

  return { onProvision, onLogin, ...render(<RelayRow {...props} />) };
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

/**
 * 两类行的刷新动作**都是一眼可见的带文字按钮**，不再藏在 `...` 菜单里。
 *
 * 「带文字」是这道闸真正守的东西：两个动作都不是「刷新一下当前显示」——
 * 中转站那个会去远端重新发现分组，官网那个会把本地 sk 重写进六个平台的配置。
 * 换成裸刷新图标的话，用户无从知道点下去会发生什么（那正是它们当初被收进
 * 菜单的理由；现在改成外置 + 文字，两个诉求同时满足）。
 */
describe("行上的刷新动作", () => {
  it("中转站行把「更新可用分组」直接摆出来", async () => {
    const user = userEvent.setup();
    const { onProvision } = renderRelayRow();

    const refetch = screen.getByRole("button", { name: "更新可用分组" });
    expect(refetch).toBeInTheDocument();

    await user.click(refetch);
    expect(onProvision).toHaveBeenCalledTimes(1);
  });

  it("官网行把「重新应用密钥配置」直接摆出来", async () => {
    const user = userEvent.setup();
    const { onProvision } = renderVendorRow();

    const reprovision = screen.getByRole("button", {
      name: "重新应用密钥配置",
    });
    expect(reprovision).toBeInTheDocument();

    await user.click(reprovision);
    expect(onProvision).toHaveBeenCalledTimes(1);
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
    renderRelayRow(vi.fn(), { loggedIn: false, sessionExpired: true });

    // 这个文件的 `t` mock 不做插值，所以档位数那句拿到的是原样模板 ——
    // 断言它出现即可证明走的是「有档位」那一支。
    expect(screen.getByText("{{count}} 个接入配置")).toBeInTheDocument();
    expect(screen.queryByText("没有可用分组")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "更新可用分组" }),
    ).not.toBeInTheDocument();

    const reLogin = screen.getByRole("button", { name: "重新登录" });
    expect(reLogin).toHaveAttribute(
      "title",
      "登录已过期，但 API 密钥仍可使用；余额暂时无法查询。点击重新登录。",
    );
  });
});
