import { describe, expect, it } from "vitest";

import {
  balanceRowsKey,
  parseBalanceRowsKey,
  type BalanceRow,
} from "../balanceRowsKey";

/** 往返一次，模拟「编码进 effect 依赖 → 在 effect 里解析出来」这条真实链路。 */
const roundTrip = (rows: BalanceRow[]) =>
  parseBalanceRowsKey(balanceRowsKey(rows));

describe("balanceRowsKey", () => {
  it("普通昵称能往返", () => {
    const rows: BalanceRow[] = [
      [1, "alice@example.com"],
      [2, "张三"],
    ];
    expect(roundTrip(rows)).toEqual(rows);
  });

  /**
   * ⭐ 这条是这个模块存在的理由。
   *
   * 原实现 `map(x => `${x.id}:${x.label}`).join(",")` + 按逗号/冒号解析，在昵称
   * 含逗号时会把一行拆成两条 ⇒ `Number("有限公司")` → `NaN`，且标签与真实值不符
   * ⇒ 余额结果被 `stillSameAccount()` 丢掉 ⇒ 那一行永远没有余额、也就没有充值入口。
   *
   * **会红的改法**：把 `balanceRowsKey` 改回 `rows.map(([id, l]) => id + ":" + l).join(",")`
   * 配 `split(",")`。
   */
  it("⭐ 昵称里含逗号时不会被拆成两条", () => {
    const rows: BalanceRow[] = [[7, "北京,有限公司"]];
    const parsed = roundTrip(rows);

    expect(parsed).toHaveLength(1);
    expect(parsed[0][0]).toBe(7);
    expect(Number.isNaN(parsed[0][0])).toBe(false);
    expect(parsed[0][1]).toBe("北京,有限公司");
  });

  it("昵称里含冒号也能原样还原", () => {
    // 冒号是原实现用来切 id 与标签的分隔符 —— 标签自己带冒号时切点就错了。
    const rows: BalanceRow[] = [[3, "http://a:b"]];
    expect(roundTrip(rows)).toEqual(rows);
  });

  it('空清单是合法输入，解析出空数组（不是抛错、不是 [""]）', () => {
    // 原实现里空清单会 join 成空串，于是 effect 得专门 `if (!key) return` 挡一道 ——
    // 否则 `"".split(",")` 给出 `[""]`，那是一个 id 为 NaN 的伪造条目。
    expect(roundTrip([])).toEqual([]);
  });

  it("同内容不同引用的两份清单给出相同的键（值相等才防得住重复请求）", () => {
    // 这正是它当 effect 依赖的前提：`relays` 每次 reload 都是新引用。
    expect(balanceRowsKey([[1, "a"]])).toBe(balanceRowsKey([[1, "a"]]));
  });

  it("账号变了键就变（同一行登出 A 登录 B 时必须重拉）", () => {
    expect(balanceRowsKey([[1, "A"]])).not.toBe(balanceRowsKey([[1, "B"]]));
  });
});
