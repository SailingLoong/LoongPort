import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import { ReconcileDialog } from "@/components/relay/ReconcileDialog";
import { RelayRow } from "@/components/relay/RelayRow";
import type {
  RelayRow as RelayRowData,
  ReconciliationReport,
  ReconciliationWindow,
  TierInfo,
} from "@/lib/api/relay";

import { createTestQueryClient } from "../utils/testQueryClient";

// 对账弹窗的数据链路是 `useReconciliationQuery → relayApi → invoke`，在 invoke
// 这一层 mock 可以同时喂「对账报告」与「立即采样」两条命令，让真实 API 层参与测试。
// （全局 setup 的 i18n 资源为空 ⇒ 文案渲染成 key 本身，断言直接用 key 定位。）
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => false,
  invoke,
}));

function win(overrides: Partial<ReconciliationWindow>): ReconciliationWindow {
  return {
    startSecs: 1_700_000_000,
    endSecs: 1_700_000_600,
    startBalanceUsd: 10,
    endBalanceUsd: 9.5,
    balanceDeltaUsd: -0.5,
    estimatedCostUsd: 0.45,
    ratio: 0.9,
    flag: "normal",
    ...overrides,
  };
}

/** 一份覆盖全部四种 flag 的报告。四个窗口时间互不重叠（行 key 用起止时刻）。 */
const flagsReport: ReconciliationReport = {
  relayId: 7,
  snapshotCount: 9,
  hasLocalTraffic: true,
  baselineRatio: 0.95,
  windows: [
    win({ flag: "suspicious", ratio: 0.4 }),
    win({ flag: "normal", startSecs: 1_699_999_400, endSecs: 1_700_000_000 }),
    win({
      flag: "skippedTopUp",
      balanceDeltaUsd: 5,
      ratio: null,
      startSecs: 1_699_998_800,
      endSecs: 1_699_999_400,
    }),
    win({
      flag: "insufficientData",
      ratio: null,
      startSecs: 1_699_998_200,
      endSecs: 1_699_998_800,
    }),
  ],
};

function stubInvoke(report: ReconciliationReport, easyModeEnabled = true) {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "relay_reconciliation") return report;
    // RelayRow 的对账资格要读省心模式状态（useEasyModeApps → 4 个 app）。
    if (cmd === "get_auto_mode_status")
      return {
        enabled: easyModeEnabled,
        strategy: "cheapest",
        model: null,
        availableModels: [],
        hasCandidates: true,
        cliInstalled: true,
      };
    throw new Error(`command not stubbed in this test: ${cmd}`);
  });
}

function renderDialog(report: ReconciliationReport) {
  stubInvoke(report);
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <ReconcileDialog
        relayId={report.relayId}
        relayLabel="Example · user@example.com"
        open
        onOpenChange={vi.fn()}
      />
    </QueryClientProvider>,
  );
}

/** 一行带 codex 托管档位、可查余额的中转站（对账入口的资格原料齐备）。 */
function relayWithCodexTier(): RelayRowData {
  const tier: TierInfo = {
    providerId: "loongport-test",
    appId: "codex",
    groupName: "Pro",
    displayName: "Example · Pro",
    model: "gpt-a",
    models: ["gpt-a"],
    rateMultiplier: 1,
    isCurrent: true,
    canVerifyModels: true,
    userEdited: false,
    allowImageGeneration: false,
  };
  return {
    id: 7,
    siteOrigin: "https://api.example.com",
    siteName: "Example",
    accountLabel: "user@example.com",
    status: "ready",
    isCurrent: true,
    canQueryBalance: true,
    canPurchase: true,
    canViewUsage: false,
    canRefresh: true,
    usageBlockers: [{ app: "codex", tierName: "Example · Pro" }],
    removeConfirmation: "configured",
    tiers: [tier],
  };
}

describe("ReconcileDialog", () => {
  it("renders every flag state; the suspicious window carries a red, identifiable badge", async () => {
    renderDialog(flagsReport);

    await waitFor(() =>
      expect(document.querySelector("tbody tr")).toBeTruthy(),
    );

    // Suspicious 行的可识别标识：徽标 + 红色（brief 钉过「Suspicious 标红」，
    // 只断文案不断颜色的话，渲染成灰色也能过）。
    const suspicious = screen.getByText("loongport.reconcile.flagSuspicious");
    expect(suspicious.className).toMatch(/red/);

    // SkippedTopUp 灰色标「充值」。
    expect(
      screen.getByText("loongport.reconcile.flagSkippedTopUp"),
    ).toBeTruthy();

    // 四种 flag 各一行；normal / insufficientData 不出徽标（后者由「比值留空」表达）。
    expect(document.querySelectorAll("tbody tr")).toHaveLength(4);
    expect(document.querySelectorAll("tbody span")).toHaveLength(2);
  });

  it("leaves ratio and baselineRatio blank when the backend could not compute them", async () => {
    renderDialog({
      relayId: 7,
      snapshotCount: 2,
      hasLocalTraffic: true,
      baselineRatio: null,
      windows: [
        win({ flag: "insufficientData", ratio: null, estimatedCostUsd: 0 }),
      ],
    });

    await waitFor(() =>
      expect(document.querySelector("tbody tr")).toBeTruthy(),
    );

    // 比值列（第 4 列）留空 —— 不猜 0、不算、不填 "--"。
    const cells = document.querySelectorAll("tbody tr td");
    expect(cells[3].textContent).toBe("");
    // 留空只针对判不了的比值：成本照常显示（fmtUsd 4 位，与用量页同约定）。
    expect(cells[1].textContent).toBe("$0.0000");
    // 有本地流量原料 ⇒ 不出「无本地路由」提示（这里估算 0 是窗口内没数据，
    // 不是整段回看期没原料）。
    expect(
      screen.queryByText("loongport.reconcile.noLocalTrafficHint"),
    ).toBeNull();
    // 基线比值（不足 3 个有效窗口）同样留空。值是标签所在容器里的子 span，
    // 不是兄弟节点（标签与值包在同一个外层 span 里）。
    const baselineValue = screen
      .getByText("loongport.reconcile.baselineRatioLabel")
      .querySelector(".tabular-nums");
    expect(baselineValue?.textContent).toBe("");
  });

  it("explains all-zero estimates when there is no locally routed traffic", async () => {
    renderDialog({
      relayId: 7,
      snapshotCount: 3,
      hasLocalTraffic: false,
      baselineRatio: null,
      windows: [
        win({
          flag: "insufficientData",
          ratio: null,
          estimatedCostUsd: 0,
          balanceDeltaUsd: -0.05,
        }),
      ],
    });

    await waitFor(() =>
      expect(
        screen.getByText("loongport.reconcile.noLocalTrafficHint"),
      ).toBeTruthy(),
    );
  });

  it("samples via the existing balance command, then refetches the report", async () => {
    let resolveBalance!: (value: unknown) => void;
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "relay_reconciliation") return flagsReport;
      if (cmd === "relay_balance")
        return new Promise((resolve) => {
          resolveBalance = resolve;
        });
      throw new Error(`command not stubbed in this test: ${cmd}`);
    });
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <ReconcileDialog
          relayId={7}
          relayLabel="Example"
          open
          onOpenChange={vi.fn()}
        />
      </QueryClientProvider>,
    );
    const reportCalls = () =>
      invoke.mock.calls.filter(([cmd]) => cmd === "relay_reconciliation");

    await waitFor(() => expect(reportCalls()).toHaveLength(1));

    fireEvent.click(
      screen.getByRole("button", { name: "loongport.reconcile.sampleNow" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("relay_balance", { relayId: 7 }),
    );
    // 采样完成前不重拉对账（先采样、后 invalidate 的顺序）。
    expect(reportCalls()).toHaveLength(1);

    resolveBalance({ usage: { success: true }, shouldPromptTopUp: false });
    await waitFor(() => expect(reportCalls().length).toBeGreaterThanOrEqual(2));
  });

  it("exposes the entry on RelayRow and opens the dialog from there", async () => {
    stubInvoke(flagsReport);
    const relay = relayWithCodexTier();
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <RelayRow
          relay={relay}
          open
          onOpenChange={vi.fn()}
          busy={new Set()}
          onLogin={vi.fn()}
          onProvision={vi.fn()}
          onSwitchTier={vi.fn()}
          onSelectTierModel={vi.fn()}
          onPurchase={vi.fn()}
          onOpenUsage={undefined}
          onCheckTier={vi.fn()}
          isCheckingTier={() => false}
          onResetTier={vi.fn()}
          onEditTier={vi.fn()}
          onDelete={vi.fn()}
        />
      </QueryClientProvider>,
    );

    // 入口在行上（hover 操作区）；省心模式状态异步回来后才具备资格，先等它。
    const entry = await screen.findByRole("button", {
      name: "loongport.reconcile.entry",
    });
    fireEvent.click(entry);
    await waitFor(() =>
      expect(screen.getByText("loongport.reconcile.title")).toBeTruthy(),
    );
    await waitFor(() =>
      expect(
        screen.getByText("loongport.reconcile.flagSuspicious"),
      ).toBeTruthy(),
    );
  });

  it("hides the entry on RelayRow when Easy Mode is off (no attributed traffic to reconcile)", async () => {
    // 省心模式全关：估算没有原料（带档位归因的本地路由流量），入口不出现。
    stubInvoke(flagsReport, false);
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <RelayRow
          relay={relayWithCodexTier()}
          open
          onOpenChange={vi.fn()}
          busy={new Set()}
          onLogin={vi.fn()}
          onProvision={vi.fn()}
          onSwitchTier={vi.fn()}
          onSelectTierModel={vi.fn()}
          onPurchase={vi.fn()}
          onOpenUsage={undefined}
          onCheckTier={vi.fn()}
          isCheckingTier={() => false}
          onResetTier={vi.fn()}
          onEditTier={vi.fn()}
          onDelete={vi.fn()}
        />
      </QueryClientProvider>,
    );

    // 省心模式状态先回来，入口资格才会判定；稳态后入口应当不存在。
    await waitFor(() => {
      const calls = invoke.mock.calls.filter(
        ([cmd]) => cmd === "get_auto_mode_status",
      );
      expect(calls.length).toBeGreaterThanOrEqual(4);
    });
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "loongport.reconcile.entry" }),
      ).toBeNull(),
    );
  });
});
