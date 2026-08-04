import { describe, expect, it } from "vitest";

import { parseRowKey, rowKey } from "../rowKey";

describe("rowKey", () => {
  it("两类行的同一个数字 id 不会撞", () => {
    // 两张表的自增 id 都从 1 起，必然重叠 —— 这是混列的核心风险。
    expect(rowKey("operator", 3)).not.toBe(rowKey("vendor", 3));
  });

  it("可以往返解析", () => {
    expect(parseRowKey(rowKey("vendor", 42))).toEqual({
      kind: "vendor",
      id: 42,
    });
    expect(parseRowKey(rowKey("operator", 1))).toEqual({
      kind: "operator",
      id: 1,
    });
  });

  it("是字符串，能直接当 Record 键与 React key", () => {
    expect(typeof rowKey("vendor", 1)).toBe("string");
  });
});
