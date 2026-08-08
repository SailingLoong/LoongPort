import { useCallback, useState } from "react";

/**
 * 「哪些操作正在进行」——**按行独立**，不是一个全局的 `busy: string | null`。
 *
 * ## 为什么必须是集合
 *
 * 原来是 `const [busy, setBusy] = useState<string | null>(null)`，于是任何一个操作
 * 进行中，所有行的按钮都 `disabled={busy !== null}` ⇒ **用户点 A 的「获取密钥」，
 * B / C 的按钮全灰了**（他明确指出过：中转站之间、账号之间本来没有依赖）。
 *
 * 那个全局禁用当时是在兜一个**真实的并发正确性问题**：后端 `relay_provision`
 * 靠 `creds::load()` 读「`is_current = 1` 的那一行」，前端得先 `set_current(id)`
 * 才能让它作用到对的账号上 —— 而 `is_current` 是全局单例，两个 provision 并发时
 * 会互相改写对方的目标。
 *
 * 根因已经修掉（命令改吃 `relay_id`，全局状态不再参与定位），所以禁用可以
 * 收回到「自己那一行」。**顺序不能反**：先放开禁用、后修后端，中间那一版会让
 * 并发操作静默串账号。
 *
 * ## key 的形状
 *
 * `"<动作>:<id>"`，如 `"provision:3"` / `"switch:<providerId>"` —— 与原来一致，
 * 所以 `RelayRow` 里那些 `busy === \`login:${relay.id}\`` 的判断不用改。
 */
export interface RowBusy {
  /** 正在进行的操作集合。传给行组件判「我这一行忙不忙」。 */
  busy: ReadonlySet<string>;
  /** 某个 key 是否正在进行。 */
  isBusy: (key: string) => boolean;
  /**
   * 跑一个带 busy 标记的异步操作。
   *
   * 用 `finally` 清掉标记 —— 抛异常时也要清，否则那一行永久卡在转圈状态。
   * 同一个 key 重复调用不做去重：按钮本身已经 disabled 了，再加一层是多余的。
   */
  run: (key: string, fn: () => Promise<void>) => Promise<void>;
}

export function useRowBusy(): RowBusy {
  const [busy, setBusy] = useState<ReadonlySet<string>>(() => new Set());

  const isBusy = useCallback((key: string) => busy.has(key), [busy]);

  const run = useCallback(async (key: string, fn: () => Promise<void>) => {
    // 用函数式更新而不是 `new Set(busy)` —— 并发的两个 run 拿到的是同一个旧
    // 闭包快照，后者会把前者的 key 覆盖掉（这正是「允许并发」要防的）。
    setBusy((prev) => new Set(prev).add(key));
    try {
      await fn();
    } finally {
      setBusy((prev) => {
        const next = new Set(prev);
        next.delete(key);
        return next;
      });
    }
  }, []);

  return { busy, isBusy, run };
}
