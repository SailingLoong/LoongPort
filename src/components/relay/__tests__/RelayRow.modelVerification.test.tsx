import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import type { ComponentProps, ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

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
          "loongport.row.removeInUseHint": "无法删除",
        }) as Record<string, string>
      )[key] ?? key,
  }),
}));

// 这里测的是模块启用时的行内呈现；「下线即不渲染」的契约在
// model-verification/__tests__/offline.test.tsx 单独钉。
vi.mock("../model-verification/availability", () => ({
  MODEL_VERIFICATION_ENABLED: true,
}));

// summaries 由 Provider 拉；用可控 verdict 喂行内 chip。
const summaries = vi.hoisted(() => ({
  verdict: null as string | null,
}));

vi.mock("@/lib/api/modelVerification", () => ({
  modelVerificationApi: {
    listSummaries: vi.fn(async () => [
      {
        providerId: "provider-a",
        appType: "codex",
        badgeVerdict: summaries.verdict,
        representativeReport: null,
      },
    ]),
  },
}));

// 弹窗 stub：完整弹窗交互由它自己的测试文件管，这里只需要「开了没」
// 与「汇报 running」两个钩子（行内 spinner 与操作组钉住都由它驱动）。
vi.mock("../model-verification/ModelVerificationDialog", () => ({
  ModelVerificationDialog: ({ open, onRunningChange }: any) => (
    <div>
      <div data-testid="verification-dialog" data-open={String(open)} />
      <button type="button" onClick={() => onRunningChange(true)}>
        emit-running
      </button>
    </div>
  ),
}));

import { createTestQueryClient } from "../../../../tests/utils/testQueryClient";

import { RelayRow } from "../RelayRow";
import { TierVerificationProvider } from "../model-verification/TierVerificationProvider";
import type { TierInfo } from "@/lib/api/relay";

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
      <TierVerificationProvider appId="codex" providerIds={["provider-a"]}>
        {ui}
      </TierVerificationProvider>
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

function relayWithTier(
  tierOverrides: Partial<TierInfo>,
): ComponentProps<typeof RelayRow>["relay"] {
  return {
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
    tiers: [{ ...tier, ...tierOverrides }],
  };
}

function rowProps(
  overrides: Partial<ComponentProps<typeof RelayRow>> = {},
): ComponentProps<typeof RelayRow> {
  return {
    relay: relayWithTier({}),
    open: true,
    onOpenChange: vi.fn(),
    busy: new Set(),
    onLogin: vi.fn(),
    onProvision: vi.fn(),
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
    ...overrides,
  };
}

describe("RelayRow model verification", () => {
  beforeEach(() => {
    summaries.verdict = null;
  });

  it("shows the managed Codex action immediately after connectivity in the existing hover group", () => {
    renderWithQuery(<RelayRow {...rowProps()} />);

    const connectivity = screen.getByTitle("测试连接");
    const verify = screen.getByTitle("模型验证");
    expect(connectivity.nextElementSibling).toBe(verify);
    expect(verify.parentElement?.className).toContain(
      "group-hover/tier:opacity-100",
    );

    fireEvent.click(verify);
    expect(screen.getByTestId("verification-dialog")).toHaveAttribute(
      "data-open",
      "true",
    );
  });

  it("does not expose verification for unmanaged app tiers", () => {
    renderWithQuery(
      <RelayRow
        {...rowProps({
          relay: relayWithTier({ appId: "gemini", canVerifyModels: false }),
        })}
      />,
    );

    expect(screen.queryByTitle("模型验证")).not.toBeInTheDocument();
  });

  it("pins a spinner for the running tier and keeps problem labels independent from manual maintenance", async () => {
    summaries.verdict = "suspicious";
    renderWithQuery(
      <RelayRow
        {...rowProps({ relay: relayWithTier({ userEdited: true }) })}
      />,
    );
    expect(screen.getByText("手动维护")).toBeInTheDocument();
    expect(await screen.findByText("需要复核")).toBeInTheDocument();
    expect(
      screen.getByTitle("现有结果需要人工复核。打开“模型验证”查看详情。"),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByTitle("模型验证"));
    fireEvent.click(await screen.findByText("emit-running"));

    const verify = screen.getByTitle("模型验证");
    expect(verify).toBeEnabled();
    expect(verify.querySelector(".animate-spin")).toBeInTheDocument();
    // 运行中操作组钉住可见：不再靠 hover 才出现。
    expect(verify.parentElement?.className).not.toContain(
      "group-hover/tier:opacity-100",
    );
  });

  it("shows a success-colored label for a verified model", async () => {
    summaries.verdict = "trusted";
    renderWithQuery(<RelayRow {...rowProps()} />);

    const label = (await screen.findByText("验证通过")).closest("span");
    expect(label).toHaveClass("text-emerald-600");
    expect(
      screen.getByTitle("现有证据验证通过。打开“模型验证”查看详情。"),
    ).toBeInTheDocument();
    expect(screen.queryByText("需要复核")).not.toBeInTheDocument();
  });

  it("does not render a tier label for an inconclusive result", async () => {
    summaries.verdict = "inconclusive";
    renderWithQuery(<RelayRow {...rowProps()} />);

    // summaries 拉完之后仍不应出现任何结论 chip。
    await screen.findByTitle("模型验证");
    expect(screen.queryByText("验证通过")).not.toBeInTheDocument();
    expect(screen.queryByText("需要复核")).not.toBeInTheDocument();
    expect(screen.queryByText("检测到异常")).not.toBeInTheDocument();
  });
});
