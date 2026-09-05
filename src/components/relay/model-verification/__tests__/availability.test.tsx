/**
 * 模块开关契约：`MODEL_VERIFICATION_ENABLED`（availability.ts 当前值）为
 * 模块启用状态的唯一事实源。判定规则 v2（RULES_VERSION=2）恢复上线后，
 * 启用契约 = Provider 拉取 summaries、入口按钮渲染。再次下线时本文件
 * 断言随之翻回。
 */
import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  listSummaries: vi.fn(async () => [
    {
      providerId: "provider-a",
      appType: "codex",
      badgeVerdict: "suspicious",
      representativeReport: null,
    },
  ]),
}));

vi.mock("@/lib/api/modelVerification", () => ({
  modelVerificationApi: { listSummaries: api.listSummaries },
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: () => {} }));

import { MODEL_VERIFICATION_ENABLED } from "../availability";
import { TierVerificationProvider } from "../TierVerificationProvider";
import { TierVerifyButton } from "../TierVerifyButton";
import { BoardVerdictDot, TierVerdictChip } from "../TierVerdictChip";

const tier = { providerId: "provider-a", displayName: "Pro" };

describe("model verification availability contract", () => {
  it("module is currently enabled", () => {
    expect(MODEL_VERIFICATION_ENABLED).toBe(true);
  });

  it("provider fetches summaries and verification surfaces render", async () => {
    render(
      <TierVerificationProvider appId="codex" providerIds={["provider-a"]}>
        <TierVerifyButton tier={tier} canVerify />
        <TierVerdictChip providerId="provider-a" />
        <BoardVerdictDot verdict="anomaly" />
      </TierVerificationProvider>,
    );

    await waitFor(() => expect(api.listSummaries).toHaveBeenCalled());
    // 入口按钮（title 未初始化 i18n 时回退为 key）与结论 chip 都出现。
    expect(
      screen.getByTitle("loongport.modelVerification.title"),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(
        screen.getByTitle(/modelVerification\.tierVerdict/),
      ).toBeInTheDocument(),
    );
    // 看板圆点：anomaly 染色节点在文档中。
    expect(document.querySelector(".bg-red-600")).toBeInTheDocument();
  });
});
