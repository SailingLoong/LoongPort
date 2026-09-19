import { useCallback, useSyncExternalStore } from "react";

/**
 * 「哪些操作正在进行」——**按行独立**，不是一个全局的 `busy: string | null`。
 *
 * ## 为什么必须是集合
 *
 * 原来是 `const [busy, setBusy] = useState<string | null>(null)`，于是任何一个操作
 * 进行中，所有行的按钮都 `disabled={busy !== null}` ⇒ **用户点 A 的「获取密钥」，
 * B / C 的按钮全灰了**（他明确指出过：中转站之间、账号之间本来没有依赖）。
 *
 * 那个全局禁用当时是在兜一个**真实的并发正确性问题**：后端 provision 命令
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
 *
 * ## 状态在模块级 store 里，不在组件里（2026-09-20）
 *
 * 原来是 `RelaySection` 实例内的 `useState`，于是「登录后自动导入正在跑」这件事
 * 只有当前挂载的那个页面看得见 —— 用户切到「服务与账号」就两眼一抹黑（登录窗
 * 已关、卡片零反馈，分不清是卡了还是成了）。升到模块级（`useSyncExternalStore`）
 * 后，同一把 key 的进行态/失败态**跨页面共享**：应用页在跑，「服务与账号」的
 * 账号卡也能显示「正在获取密钥并导入档位」。
 *
 * ## 失败态的保留（同一批加的）
 *
 * toast 一闪就没；卡片需要一条**留下来**的失败行，直到用户重试（重跑同一把
 * key 时清除）。调用方在自己的 catch 里调 [`RowBusy.fail`] 记录 —— `run` 本身
 * 不代捞：现有调用方的 fn 全都自带 try/catch（toast 在那里发），`run` 再包一层
 * 只会得到一个永远不抛的 Promise。
 */
export interface RowBusy {
  /** 正在进行的操作集合。传给行组件判「我这一行忙不忙」。 */
  busy: ReadonlySet<string>;
  /** 某个 key 是否正在进行。 */
  isBusy: (key: string) => boolean;
  /** 某个 key 最近一次失败的原因；没失败过或已重跑则返回 null。 */
  errorOf: (key: string) => string | null;
  /** 记录一次失败。调用方 catch 到什么写什么（toast 照发，两条不冲突）。 */
  fail: (key: string, message: string) => void;
  /**
   * 跑一个带 busy 标记的异步操作。
   *
   * 用 `finally` 清掉标记 —— 抛异常时也要清，否则那一行永久卡在转圈状态。
   * 同一个 key 重复调用不做去重：按钮本身已经 disabled 了，再加一层是多余的。
   * 起跑时清掉这个 key 的失败记录（重试就是新的开始）。
   */
  run: (key: string, fn: () => Promise<void>) => Promise<void>;
}

interface ActivityState {
  busy: ReadonlySet<string>;
  errors: ReadonlyMap<string, string>;
}

let state: ActivityState = {
  busy: new Set(),
  errors: new Map(),
};
const listeners = new Set<() => void>();

function setActivity(next: ActivityState) {
  state = next;
  for (const notify of listeners) notify();
}

function subscribe(notify: () => void) {
  listeners.add(notify);
  return () => listeners.delete(notify);
}

const snapshot = () => state;

export function useRowBusy(): RowBusy {
  const current = useSyncExternalStore(subscribe, snapshot, snapshot);

  const isBusy = useCallback((key: string) => current.busy.has(key), [current]);
  const errorOf = useCallback(
    (key: string) => current.errors.get(key) ?? null,
    [current],
  );
  const fail = useCallback((key: string, message: string) => {
    setActivity({
      busy: state.busy,
      errors: new Map(state.errors).set(key, message),
    });
  }, []);

  const run = useCallback(async (key: string, fn: () => Promise<void>) => {
    // 整表重建而不是在旧 Set 上 mutate —— 快照必须不可变，
    // useSyncExternalStore 靠引用相等决定要不要重渲染。
    setActivity({
      busy: new Set(state.busy).add(key),
      errors: (() => {
        const next = new Map(state.errors);
        next.delete(key);
        return next;
      })(),
    });
    try {
      await fn();
    } finally {
      const nextBusy = new Set(state.busy);
      nextBusy.delete(key);
      setActivity({ busy: nextBusy, errors: state.errors });
    }
  }, []);

  return { busy: current.busy, isBusy, errorOf, fail, run };
}
