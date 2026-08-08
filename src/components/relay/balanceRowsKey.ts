/**
 * 「哪些行该拉余额」这份清单的 effect 依赖键，以及它的往返解析。
 *
 * ## 为什么需要一个「键」而不是直接依赖数组
 *
 * 余额是渲染后异步补的：`relays` / `vendors` 每次 reload 都是**新对象引用**，
 * 拿它当 `useEffect` 依赖会让 effect 每次都跑、把 N 个余额请求重发一遍。
 * 所以依赖必须是一个**值相等**的摘要字符串。
 *
 * 摘要里除 id 还要含账号标签：同一行登出 A 再登录 B 时 id 不变 ⇒ 只用 id 的话
 * effect 不重跑，那一行会一直显示 A 的余额。
 *
 * ## ⚠️ 为什么用 `JSON` 而不是 `id:label` 逗号拼接（review 抓出的 bug）
 *
 * 原来两处 effect 各自写的是 `map(x => `${x.id}:${x.accountLabel}`).join(",")`，
 * 解析时按逗号分段、按第一个冒号切。而 **`accountLabel` 是用户在中转站网站上自己设的
 * 昵称，里面可以有逗号和冒号**。昵称含逗号时：
 *
 * 1. 一行被拆成两条 ⇒ 多出一个伪造条目；
 * 2. `Number("张三")` → `NaN` ⇒ 拿 `NaN` 当 `relayId` 去请求余额；
 * 3. 拆出来的标签与真实值不符 ⇒ 请求回来时 `stillSameAccount()` 判否 ⇒ **结果被丢掉**。
 *
 * 症状是「这一行永远没有余额」，而余额是充值入口的前置（`RowBalance` 在
 * `balance === null` 时整块不渲染）⇒ 用户连充值按钮都看不到，且没有任何报错。
 *
 * **不自己设计转义**（把分隔符换成 `\0` 之类）：那要同时改序列化与解析两处，
 * 而「两处各写一份、其中一处没跟上」正是这个 bug 的形状。`JSON` 是现成且正确的。
 */

/** 一行的余额身份：`[行 id, 账号标签]`。 */
export type BalanceRow = [id: number, accountLabel: string];

/**
 * 把「已登录的行」编码成 effect 依赖键。
 *
 * 顺序即入参顺序（不排序）—— 行序本身是用户拖出来的、会变，而它变了确实该重拉。
 */
export function balanceRowsKey(rows: readonly BalanceRow[]): string {
  return JSON.stringify(rows);
}

/** 从依赖键还原出那份清单。与 {@link balanceRowsKey} 严格往返。 */
export function parseBalanceRowsKey(key: string): BalanceRow[] {
  return JSON.parse(key) as BalanceRow[];
}
