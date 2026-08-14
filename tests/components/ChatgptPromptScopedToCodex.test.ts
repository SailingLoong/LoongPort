import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const source = readFileSync(
  resolve(__dirname, "../../src/components/relay/RelaySection.tsx"),
  "utf-8",
);

describe("ChatGPT 切换确认由后端按动作裁决", () => {
  it("前端不读取或重建 ChatGPT 业务状态", () => {
    expect(source).not.toContain("chatgptNeedsAttention");
    expect(source).not.toContain("touchesCodexConfig");
  });

  it("页面只消费命令返回的 confirmationRequired", () => {
    expect(source).toContain('result.status === "confirmationRequired"');
  });
});
