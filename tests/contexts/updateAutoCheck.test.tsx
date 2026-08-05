import type { ReactNode } from "react";
import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { UpdateProvider } from "@/contexts/UpdateContext";

/**
 * 自动检查更新的三条行为，都是**改错了不会有编译错误、只会静默走歪**的那类。
 *
 * 为什么值得钉：
 *
 * 1. **轮询存在**。它是「用户挂着不关也能收到提醒」的唯一保证，而这个应用默认
 *    `minimize_to_tray_on_close`（点关闭是最小化到托盘、进程不退出）⇒ 挂几天是常态。
 *    2026-08-04 那版只在启动时查一次，那些用户永远收不到提醒。
 * 2. **失败不产生 unhandled rejection**。`checkUpdate` 失败时会 `throw`（主动点击那条路
 *    要据此弹提示），自动检查这条路没人接 ⇒ 写成 `void checkUpdate()` 的话，离线用户
 *    每 6 小时来一条 unhandled rejection。
 * 3. **卸载后不再触发**。忘了 `clearInterval` 的计时器会在组件卸载后继续跑。
 */

const updaterMocks = vi.hoisted(() => ({
  checkForUpdate: vi.fn(),
}));

vi.mock("@/lib/updater", () => ({
  checkForUpdate: (...args: unknown[]) => updaterMocks.checkForUpdate(...args),
}));

const wrap = (children: ReactNode) => (
  <UpdateProvider>{children}</UpdateProvider>
);

describe("自动检查更新", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    updaterMocks.checkForUpdate.mockReset();
    localStorage.clear();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("启动后查一次，之后每 6 小时再查", async () => {
    updaterMocks.checkForUpdate.mockResolvedValue({ status: "up-to-date" });
    render(wrap(null));

    // 启动那次有意延迟几秒（不跟首屏渲染争资源）——所以刚挂载时还没查。
    expect(updaterMocks.checkForUpdate).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(updaterMocks.checkForUpdate).toHaveBeenCalledTimes(1);

    // 再过 6 小时应该有第二次。**这条是「挂着不关也能收到提醒」的判据。**
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6 * 60 * 60 * 1000);
    });
    expect(updaterMocks.checkForUpdate).toHaveBeenCalledTimes(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(6 * 60 * 60 * 1000);
    });
    expect(updaterMocks.checkForUpdate).toHaveBeenCalledTimes(3);
  });

  it("检查失败不产生 unhandled rejection", async () => {
    // 离线、GitHub 不可达、公司网络拦截 —— 这条路是常态而非异常。
    updaterMocks.checkForUpdate.mockRejectedValue(new Error("fetch failed"));

    // ⚠️ **必须听 `process` 而不是 `window`。** jsdom 环境下 unhandled rejection 走的是
    // Node 的 `process` 事件，`window.addEventListener("unhandledrejection")` 收不到
    // ——2026-08-05 实测：把实现改回 `void checkUpdate()`，监听 window 的版本仍然全绿
    // （那是个假的保险），换成 process 才真的变红。
    const onUnhandled = vi.fn();
    process.on("unhandledRejection", onUnhandled);

    try {
      render(wrap(null));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_000);
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(6 * 60 * 60 * 1000);
      });
      // 让 microtask 队列跑完 —— unhandled rejection 的判定发生在那之后。
      await new Promise((resolve) => {
        vi.useRealTimers();
        setTimeout(resolve, 20);
        vi.useFakeTimers();
      });

      // 两次都失败了，但都被 `.catch()` 接住。
      expect(updaterMocks.checkForUpdate).toHaveBeenCalledTimes(2);
      expect(onUnhandled).not.toHaveBeenCalled();
    } finally {
      process.off("unhandledRejection", onUnhandled);
    }
  });

  it("卸载之后不再检查", async () => {
    updaterMocks.checkForUpdate.mockResolvedValue({ status: "up-to-date" });
    const { unmount } = render(wrap(null));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(updaterMocks.checkForUpdate).toHaveBeenCalledTimes(1);

    unmount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1000);
    });
    expect(updaterMocks.checkForUpdate).toHaveBeenCalledTimes(1);
  });
});
