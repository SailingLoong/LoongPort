import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) => {
      const strings: Record<string, string> = {
        "loongport.modelVerification.evidence.title": "验证依据",
        "loongport.modelVerification.evidence.level.insufficient": "证据不足",
        "loongport.modelVerification.evidence.fact.modelMatch": "模型标识",
        "loongport.modelVerification.evidence.fact.toolCallShape":
          "工具调用结构",
        "loongport.modelVerification.evidence.outcome.passed": "通过",
        "loongport.modelVerification.evidence.outcome.failed": "未通过",
        "loongport.modelVerification.evidence.diagnose": "查看原因",
        "loongport.modelVerification.diagnostic.title": "未通过原因",
        "loongport.modelVerification.diagnostic.empty": "这条没有留存。",
        "loongport.modelVerification.diagnostic.request": "发出的请求",
        "loongport.modelVerification.diagnostic.response": "收到的响应",
        "loongport.modelVerification.diagnostic.copy": "复制",
        "loongport.modelVerification.diagnostic.probe.core": "基础请求",
      };
      return strings[key] ?? (options?.defaultValue as string) ?? key;
    },
  }),
}));

import type { VerificationReport } from "@/lib/api/modelVerification";
import { VerificationEvidenceList } from "../VerificationEvidenceList";

const report = (
  diagnostics?: VerificationReport["diagnostics"],
): VerificationReport => ({
  target: { providerId: "provider-a", appType: "codex", model: "gpt-5.6-sol" },
  verdict: "suspicious",
  evidenceLevel: "insufficient",
  facts: [
    { code: "modelMatch", outcome: "failed" },
    { code: "toolCallShape", outcome: "passed" },
  ],
  diagnostics,
  rulesVersion: 2,
  checkedAt: 1,
});

describe("VerificationEvidenceList diagnostics", () => {
  it("expands raw request/response of the failed fact from the report itself", () => {
    render(
      <VerificationEvidenceList
        report={report([
          {
            probe: "core",
            code: "modelMatch",
            request: '{ "model": "gpt-5.6-sol" }',
            response: '{ "model": "gpt-5.6-terra" }',
          },
        ])}
      />,
    );

    expect(screen.queryByText("未通过原因")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTitle("查看原因"));

    expect(screen.getByText("未通过原因")).toBeInTheDocument();
    expect(screen.getByText(/gpt-5.6-terra/)).toBeInTheDocument();
    // 通过项没有叹号入口。
    expect(screen.getAllByTitle("查看原因")).toHaveLength(1);
  });

  it("shows the empty hint for legacy reports without diagnostics", () => {
    render(<VerificationEvidenceList report={report()} />);

    fireEvent.click(screen.getByTitle("查看原因"));

    expect(screen.getByText("这条没有留存。")).toBeInTheDocument();
  });
});
