/**
 * 模块下线契约：`MODEL_VERIFICATION_ENABLED = false`（availability.ts 当前值）时，
 * Provider 不拉取 summaries，入口按钮 / 行内 chip / 看板圆点全部不渲染。
 * 模块恢复上线时，这个文件的断言要跟着翻转。
 */
import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ listSummaries: vi.fn(async () => []) }));

vi.mock("@/lib/api/modelVerification", () => ({
  modelVerificationApi: { listSummaries: api.listSummaries },
}));
vi.mock("@/hooks/useTauriEvent", () => ({ useTauriEvent: () => {} }));

import { MODEL_VERIFICATION_ENABLED } from "../availability";
import { TierVerificationProvider } from "../TierVerificationProvider";
import { TierVerifyButton } from "../TierVerifyButton";
import { BoardVerdictDot, TierVerdictChip } from "../TierVerdictChip";

const tier = { providerId: "provider-a", displayName: "Pro" };

describe("model verification offline contract", () => {
  it("module is currently disabled", () => {
    expect(MODEL_VERIFICATION_ENABLED).toBe(false);
  });

  it("provider does not fetch and every verification surface renders nothing", async () => {
    const { container } = render(
      <TierVerificationProvider appId="codex" providerIds={["provider-a"]}>
        <TierVerifyButton tier={tier} canVerify />
        <TierVerdictChip providerId="provider-a" />
        <BoardVerdictDot verdict="anomaly" />
      </TierVerificationProvider>,
    );

    // 等微任务队列清空，确认 effect 跑过也没有发出任何请求。
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(api.listSummaries).not.toHaveBeenCalled();
    expect(container.textContent).toBe("");
  });
});
