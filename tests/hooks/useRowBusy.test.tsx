import { describe, it, expect } from "vitest";
import { act, renderHook } from "@testing-library/react";

import { useRowBusy } from "@/components/relay/useRowBusy";

/**
 * 闸：**两个操作并发时不能互相覆盖 busy 标记**。
 *
 * 这是 `useRowBusy` 存在的全部理由 —— 中转站之间没有依赖，A 在获取密钥时
 * B 也该能点。而它有一个极易写错的退化写法：
 *
 * ```ts
 * setBusy(new Set(busy).add(key))   // ❌ 用闭包里的 busy
 * ```
 *
 * 那样两个并发的 `run` 拿到的是**同一个旧快照**，后者会把前者的 key 抹掉 ⇒
 * 前者的按钮提前恢复可点、转圈消失，而它其实还在跑。必须用函数式更新
 * （`setBusy(prev => ...)`）。
 *
 * 用 mutation 验过这道闸真的有效：把实现改成闭包写法，第一个用例失败。
 */
describe("useRowBusy", () => {
  it("并发的两个操作各自记账，互不覆盖", async () => {
    const { result } = renderHook(() => useRowBusy());

    // 两个手工控制的 promise —— 模拟两个中转站同时在 provision。
    let releaseA!: () => void;
    let releaseB!: () => void;
    const a = new Promise<void>((r) => (releaseA = r));
    const b = new Promise<void>((r) => (releaseB = r));

    // 不 await：让它们同时处于进行中。
    let runA!: Promise<void>;
    let runB!: Promise<void>;
    await act(async () => {
      runA = result.current.run("provision:1", () => a);
      runB = result.current.run("provision:2", () => b);
    });

    expect(result.current.isBusy("provision:1")).toBe(true);
    expect(result.current.isBusy("provision:2")).toBe(true);

    // A 结束，B 还在跑 —— B 的标记必须留着。
    await act(async () => {
      releaseA();
      await runA;
    });
    expect(result.current.isBusy("provision:1")).toBe(false);
    expect(result.current.isBusy("provision:2")).toBe(true);

    await act(async () => {
      releaseB();
      await runB;
    });
    expect(result.current.isBusy("provision:2")).toBe(false);
  });

  it("操作抛异常时也清掉标记（否则那一行永久卡在转圈）", async () => {
    const { result } = renderHook(() => useRowBusy());

    await act(async () => {
      await result.current
        .run("login:3", () => Promise.reject(new Error("boom")))
        // run 不吞异常（调用方自己 try/catch 报 toast），这里只是别让测试挂掉。
        .catch(() => {});
    });

    expect(result.current.isBusy("login:3")).toBe(false);
    expect(result.current.busy.size).toBe(0);
  });

  it("初始状态是空集合", () => {
    const { result } = renderHook(() => useRowBusy());
    expect(result.current.busy.size).toBe(0);
    expect(result.current.isBusy("anything")).toBe(false);
  });
});
