import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

describe("promptOnboardingRegister", () => {
  beforeEach(() => {
    // 单例是模块级状态：每个用例重置模块注册表，拿一份干净实例。
    vi.resetModules();
    invokeMock.mockReset();
  });

  it("一次进程内多次调用收敛成一次 invoke（App 挂载与首启判定都会碰它）", async () => {
    invokeMock.mockResolvedValue(true);
    const { promptOnboardingRegister } = await import("./onboarding");
    const [a, b] = await Promise.all([
      promptOnboardingRegister(),
      promptOnboardingRegister(),
    ]);
    expect(a).toBe(true);
    expect(b).toBe(true);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("onboarding_prompt_register");
  });

  it("invoke 失败时静默降级为 false（回落到既有的首启目录提示）", async () => {
    invokeMock.mockRejectedValue(new Error("backend unavailable"));
    const { promptOnboardingRegister } = await import("./onboarding");
    await expect(promptOnboardingRegister()).resolves.toBe(false);
  });
});
