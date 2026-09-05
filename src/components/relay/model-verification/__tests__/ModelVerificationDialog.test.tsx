import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  listModels: vi.fn(),
  listHistory: vi.fn(),
  start: vi.fn(),
  cancel: vi.fn(),
  onProgress: vi.fn(),
  diagnostics: vi.fn(),
}));

vi.mock("@/lib/api/modelVerification", () => ({ modelVerificationApi: api }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) => {
      const strings: Record<string, string> = {
        "loongport.modelVerification.title": "模型验证",
        "loongport.modelVerification.titleWithTier": "验证模型 · {{tierName}}",
        "loongport.modelVerification.model.label": "选择模型",
        "loongport.modelVerification.model.loading": "正在加载模型…",
        "loongport.modelVerification.model.empty": "没有可验证的模型",
        "loongport.modelVerification.model.error": "模型列表加载失败",
        "loongport.modelVerification.actions.start": "开始验证",
        "loongport.modelVerification.actions.stop": "停止验证",
        "loongport.modelVerification.actions.retry": "重试",
        "loongport.modelVerification.status.running":
          "正在验证 {{completed}} / {{total}}",
        "loongport.modelVerification.verdict.trusted": "验证通过",
        "loongport.modelVerification.verdict.suspicious": "需要复核",
        "loongport.modelVerification.verdict.anomaly": "检测到异常",
        "loongport.modelVerification.verdict.inconclusive": "证据不足",
        "loongport.modelVerification.failure.network": "网络连接失败",
        "loongport.modelVerification.history.title": "最近 5 条验证记录",
        "loongport.modelVerification.history.empty": "暂无验证记录",
        "loongport.modelVerification.history.source.active": "当前验证",
        "loongport.modelVerification.evidence.fact.modelMatch": "模型身份",
        "loongport.modelVerification.evidence.diagnose": "查看原因",
        "loongport.modelVerification.diagnostic.title": "未通过原因",
        "loongport.modelVerification.diagnostic.loading": "读取中…",
        "loongport.modelVerification.diagnostic.empty": "这条没有留存。",
        "loongport.modelVerification.diagnostic.request": "发出的请求",
        "loongport.modelVerification.diagnostic.response": "收到的响应",
        "loongport.modelVerification.evidence.fact.streamLifecycle":
          "流式响应生命周期",
        "loongport.modelVerification.evidence.outcome.passed": "通过",
        "loongport.modelVerification.evidence.outcome.failed": "未通过",
      };
      return (strings[key] ?? key).replace(/{{(\w+)}}/g, (_, name) =>
        String(options?.[name] ?? ""),
      );
    },
  }),
}));

vi.mock("@/components/ui/select", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  const SelectContext = React.createContext<{
    onValueChange: (value: string) => void;
    disabled?: boolean;
  } | null>(null);

  return {
    Select: ({
      children,
      onValueChange,
      disabled,
    }: {
      children: React.ReactNode;
      onValueChange: (value: string) => void;
      disabled?: boolean;
    }) => (
      <SelectContext.Provider value={{ onValueChange, disabled }}>
        {children}
      </SelectContext.Provider>
    ),
    SelectTrigger: ({ children }: { children: React.ReactNode }) => (
      <button type="button" role="combobox">
        {children}
      </button>
    ),
    SelectValue: ({ placeholder }: { placeholder?: string }) => (
      <span>{placeholder}</span>
    ),
    SelectContent: ({ children }: { children: React.ReactNode }) => (
      <div>{children}</div>
    ),
    SelectItem: ({
      children,
      value,
      disabled: itemDisabled,
    }: {
      children: React.ReactNode;
      value: string;
      disabled?: boolean;
    }) => {
      const context = React.useContext(SelectContext);
      return (
        <button
          type="button"
          role="option"
          // 与 radix 语义一致：单选项自身的 disabled 与整组 disabled 取或。
          disabled={itemDisabled || context?.disabled}
          onClick={() => {
            if (itemDisabled) return;
            context?.onValueChange(value);
          }}
        >
          {children}
        </button>
      );
    },
  };
});

import { ModelVerificationDialog } from "../ModelVerificationDialog";
import type {
  VerificationHistoryEntry,
  VerificationReport,
  VerificationVerdict,
} from "@/lib/api/modelVerification";

let progressListener: ((event: unknown) => void) | undefined;

function DialogHarness({
  open = true,
  tierName = "旗舰",
  report = null,
}: {
  open?: boolean;
  tierName?: string;
  report?: VerificationReport | null;
}) {
  return (
    <ModelVerificationDialog
      providerId="provider-a"
      appType="codex"
      tierDisplayName={tierName}
      open={open}
      onOpenChange={() => {}}
      report={report}
    />
  );
}

function ReopenHarness() {
  const [open, setOpen] = useState(true);

  return (
    <>
      <ModelVerificationDialog
        providerId="provider-a"
        appType="codex"
        tierDisplayName="旗舰"
        open={open}
        onOpenChange={setOpen}
        report={null}
      />
      {!open && (
        <button type="button" onClick={() => setOpen(true)}>
          reopen dialog
        </button>
      )}
    </>
  );
}

const report = (verdict: VerificationVerdict): VerificationReport => ({
  target: { providerId: "provider-a", appType: "codex", model: "gpt-5" },
  verdict,
  evidenceLevel: "protocolBehavior",
  facts: [{ code: "modelMatch", outcome: "passed" }],
  rulesVersion: 1,
  checkedAt: 1,
});

const history: VerificationHistoryEntry[] = [
  { source: "active", report: report("trusted") },
  {
    source: "active",
    report: {
      target: {
        providerId: "provider-a",
        appType: "codex",
        model: "gpt-5.6-sol",
      },
      verdict: "anomaly",
      evidenceLevel: "insufficient",
      facts: [{ code: "streamLifecycle", outcome: "failed" }],
      rulesVersion: 1,
      checkedAt: 2,
    },
  },
];

async function openModelOptions() {
  await screen.findByRole("combobox");
  return screen.findByRole("option", { name: "gpt-5" });
}

describe("ModelVerificationDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    progressListener = undefined;
    api.listModels.mockResolvedValue([
      { name: "gpt-5", fitness: "unknown" as const },
    ]);
    api.listHistory.mockResolvedValue([]);
    api.start.mockResolvedValue({ runId: "run-1", state: "queued" });
    api.cancel.mockResolvedValue(undefined);
    api.onProgress.mockImplementation(
      async (listener: (event: unknown) => void) => {
        progressListener = listener;
        return () => {};
      },
    );
  });

  it("shows raw request/response when clicking the warning icon on a failed fact", async () => {
    api.listModels.mockResolvedValue([
      { name: "gpt-5.6-sol", fitness: "unknown" as const },
    ]);
    api.listHistory.mockResolvedValue([
      {
        source: "active" as const,
        report: {
          target: {
            providerId: "provider-a",
            appType: "codex",
            model: "gpt-5.6-sol",
          },
          verdict: "suspicious",
          evidenceLevel: "insufficient",
          facts: [{ code: "modelMatch", outcome: "failed" }],
          rulesVersion: 2,
          checkedAt: 1,
        },
      },
    ]);
    api.diagnostics.mockResolvedValue([
      {
        probe: "core",
        code: "modelMatch",
        request: '{ "model": "gpt-5.6-sol" }',
        response: '{ "model": "gpt-5.6-terra" }',
      },
    ]);
    render(<DialogHarness />);

    const trigger = await screen.findByTitle("查看原因");
    fireEvent.click(trigger);

    expect(await screen.findByText("未通过原因")).toBeInTheDocument();
    expect(await screen.findByText(/gpt-5.6-terra/)).toBeInTheDocument();
    expect(api.diagnostics).toHaveBeenCalledWith(
      "provider-a",
      "codex",
      "gpt-5.6-sol",
    );
  });

  it("identifies the selected tier in the localized dialog title", async () => {
    render(<DialogHarness tierName="旗舰分组" />);

    expect(
      await screen.findByRole("heading", { name: "验证模型 · 旗舰分组" }),
    ).toBeInTheDocument();
  });

  it("marks positively-unsupported models with a reason and blocks selection", async () => {
    // 快照正向覆盖且协议不符 ⇒ 禁选 + 原因标签；未覆盖（unknown）照常可选。
    api.listModels.mockResolvedValue([
      { name: "bad-model", fitness: "unsupported_protocol" as const },
      { name: "gpt-5", fitness: "unknown" as const },
    ]);
    render(<DialogHarness />);

    await screen.findByRole("combobox");
    await userEvent.click(screen.getByRole("combobox"));

    const unsupported = await screen.findByRole("option", {
      name: /bad-model/,
    });
    // 原因标签随行渲染（title 即完整说明）。
    expect(
      screen.getByTitle("loongport.modelVerification.model.unsupportedHint"),
    ).toHaveTextContent("loongport.modelVerification.model.unsupportedTag");

    // 行为契约：点「不支持」的项不产生选择——开始按钮保持禁用；
    // 点未覆盖（unknown）的项正常选中解禁。
    const startButton = () => screen.getByRole("button", { name: "开始验证" });
    await userEvent.click(unsupported);
    expect(startButton()).toBeDisabled();

    await userEvent.click(screen.getByRole("combobox"));
    await userEvent.click(await screen.findByRole("option", { name: "gpt-5" }));
    expect(startButton()).toBeEnabled();
  });

  it("requires an explicit model selection", async () => {
    render(<DialogHarness />);

    await openModelOptions();
    expect(screen.getByRole("button", { name: "开始验证" })).toBeDisabled();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("shows loading, error, and empty model-list states", async () => {
    let resolveModels!: (models: unknown[]) => void;
    api.listModels.mockReturnValue(
      new Promise((resolve) => (resolveModels = resolve)),
    );
    const { rerender } = render(<DialogHarness />);
    expect(screen.getByText("正在加载模型…")).toBeInTheDocument();

    resolveModels([]);
    await screen.findByText("没有可验证的模型");

    api.listModels.mockRejectedValueOnce(new Error("offline"));
    rerender(<DialogHarness open={false} />);
    rerender(<DialogHarness />);
    await screen.findByText("模型列表加载失败");
  });

  it("does not mutate when closed before starting", async () => {
    const onOpenChange = vi.fn();
    render(
      <ModelVerificationDialog
        providerId="provider-a"
        appType="codex"
        tierDisplayName="旗舰"
        open
        onOpenChange={onOpenChange}
        report={null}
      />,
    );
    await openModelOptions();
    fireEvent.click(screen.getByRole("button", { name: "common.close" }));

    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(api.start).not.toHaveBeenCalled();
    expect(api.cancel).not.toHaveBeenCalled();
  });

  it("shows progress and can stop the current run", async () => {
    render(<DialogHarness />);
    const option = await openModelOptions();
    fireEvent.click(option);
    fireEvent.click(screen.getByRole("button", { name: "开始验证" }));

    await waitFor(() => expect(progressListener).toBeDefined());
    act(() => {
      progressListener?.({
        runId: "run-1",
        providerId: "provider-a",
        appType: "codex",
        model: "gpt-5",
        state: "running",
        completedChecks: 2,
        totalChecks: 4,
        failure: null,
      });
    });

    await screen.findByText("正在验证 2 / 4");
    fireEvent.click(screen.getByRole("button", { name: "停止验证" }));
    await waitFor(() => expect(api.cancel).toHaveBeenCalledWith("run-1"));
  });

  it("renders sanitized failures and every finite report verdict", async () => {
    const { rerender } = render(<DialogHarness />);
    const option = await openModelOptions();
    fireEvent.click(option);
    fireEvent.click(screen.getByRole("button", { name: "开始验证" }));
    await waitFor(() => expect(progressListener).toBeDefined());

    act(() => {
      progressListener?.({
        runId: "run-1",
        providerId: "provider-a",
        appType: "codex",
        model: "gpt-5",
        state: "failed",
        completedChecks: 0,
        totalChecks: 4,
        failure: "network",
      });
    });
    await screen.findByText("网络连接失败");

    for (const [verdict, label] of [
      ["trusted", "验证通过"],
      ["suspicious", "需要复核"],
      ["anomaly", "检测到异常"],
      ["inconclusive", "证据不足"],
    ] as const) {
      rerender(<DialogHarness report={report(verdict)} />);
      await screen.findByText(label);
    }
  });

  it("exposes only the user-triggered verification controls", async () => {
    render(<DialogHarness />);

    await screen.findByRole("combobox");
    expect(
      screen.getByRole("button", { name: "开始验证" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });

  it("shows the latest active verification records for this tier", async () => {
    api.listHistory.mockResolvedValue(history);

    render(<DialogHarness />);

    expect(await screen.findByText("最近 5 条验证记录")).toBeInTheDocument();
    expect(screen.getAllByText("当前验证")).toHaveLength(2);
    expect(screen.getByText("gpt-5.6-sol")).toBeInTheDocument();
    expect(screen.getByText("流式响应生命周期")).toBeInTheDocument();
    expect(screen.getByText("未通过")).toBeInTheDocument();
    expect(api.listHistory).toHaveBeenCalledWith("provider-a", "codex");
  });

  it("requests a fresh model list every time the dialog reopens", async () => {
    const { rerender } = render(<DialogHarness />);
    await screen.findByRole("combobox");
    rerender(<DialogHarness open={false} />);
    rerender(<DialogHarness />);

    await waitFor(() => expect(api.listModels).toHaveBeenCalledTimes(2));
  });

  it("reopens the same active run without starting another request", async () => {
    render(<ReopenHarness />);
    fireEvent.click(await openModelOptions());
    fireEvent.click(screen.getByRole("button", { name: "开始验证" }));
    await waitFor(() => expect(api.start).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole("button", { name: "common.close" }));
    fireEvent.click(
      await screen.findByRole("button", { name: "reopen dialog" }),
    );
    await waitFor(() => expect(api.start).toHaveBeenCalledTimes(1));

    act(() => {
      progressListener?.({
        runId: "run-1",
        providerId: "provider-a",
        appType: "codex",
        model: "gpt-5",
        state: "running",
        completedChecks: 1,
        totalChecks: 4,
        failure: null,
      });
    });
    await screen.findByText("正在验证 1 / 4");
  });
});
