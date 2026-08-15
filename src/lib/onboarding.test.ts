import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

describe("promptOnboardingStarReward", () => {
  beforeEach(() => {
    // 单例是模块级状态：每个用例重置模块注册表，拿一份干净实例。
    vi.resetModules();
    invokeMock.mockReset();
  });

  it("一次进程内多次调用收敛成一次 invoke（多处首启判定都会碰它）", async () => {
    invokeMock.mockResolvedValue(undefined);
    const { promptOnboardingStarReward } = await import("./onboarding");
    await Promise.all([
      promptOnboardingStarReward(),
      promptOnboardingStarReward(),
    ]);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("onboarding_prompt_star_reward");
  });

  it("invoke 失败时静默降级（不抛错 —— 引导是加分项，别打扰新用户）", async () => {
    invokeMock.mockRejectedValue(new Error("backend unavailable"));
    const { promptOnboardingStarReward } = await import("./onboarding");
    await expect(promptOnboardingStarReward()).resolves.toBeUndefined();
  });
});
