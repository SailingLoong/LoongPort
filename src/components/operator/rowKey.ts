/**
 * 混列列表里的行标识。
 *
 * ## 为什么必须有它
 *
 * `loongport_operator` 与 `loongport_vendor` 的 `id` 都是自增主键、都从 1 开始
 * ⇒ **必然重叠**。而这个列表处处拿它当唯一键（React `key`、dnd-kit 的 `items`、
 * `openState` / `balances` 的 Record 键）—— 撞了会让**官网行的余额显示到同 id 的
 * 中转站行上**、展开态互相干扰、拖动错位，且**没有任何报错**。
 *
 * ## 为什么是字符串而不是 `{kind, id}` 对象
 *
 * 上面那几处里有三处要求它能当 Record 键与 React key，对象得先序列化 ——
 * 那等于同一个东西存两种形态。字符串在三处都是一等公民。
 *
 * ## 只用在**会同时装两类行**的地方
 *
 * 只在 operator 自己内部流转的 id（`onLogin(op.id)` 传出去、回调里再 find 回来）
 * **仍是 number**：那些不参与跨类索引，改它们只是扩大改动面。
 */
export type RowKind = "operator" | "vendor";
export type RowKey = string;

export function rowKey(kind: RowKind, id: number): RowKey {
  return `${kind}:${id}`;
}

export function parseRowKey(key: RowKey): { kind: RowKind; id: number } {
  const [kind, rest] = key.split(":", 2);
  return { kind: kind as RowKind, id: Number(rest) };
}
