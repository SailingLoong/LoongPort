import { describe, expect, it } from "vitest";

import {
  buildIssueBody,
  buildIssueTitle,
  dayUtc,
  MAX_DESCRIPTION_CHARS,
  MAX_META_BYTES,
  MAX_SCREENSHOTS,
  parseFeedbackForm,
  type FeedbackForm,
} from "./feedback";

const SOURCE = "0123456789abcdef0123456789abcdef";

function makeFile(
  name: string,
  size: number,
  type = "image/png",
): File {
  return new File([new Uint8Array(size)], name, { type });
}

function makeForm(
  overrides: Record<string, string | File | (string | File)[]> = {},
): FormData {
  const form = new FormData();
  form.set("sourceId", SOURCE);
  form.set("appVersion", "6.25.0");
  form.set("description", "示例反馈：切换档位后托盘没有刷新。");
  form.set("meta", JSON.stringify({ appVersion: "6.25.0", os: "windows" }));
  for (const [key, value] of Object.entries(overrides)) {
    if (Array.isArray(value)) {
      form.delete(key);
      for (const v of value) form.append(key, v);
    } else {
      form.set(key, value);
    }
  }
  return form;
}

function expectOk(result: FeedbackForm): Extract<FeedbackForm, { ok: true }> {
  expect(result.ok).toBe(true);
  if (!result.ok) throw new Error("unreachable");
  return result;
}

describe("parseFeedbackForm", () => {
  it("接受完整合法表单（文本 + 截图 + 诊断包）", () => {
    const form = makeForm({
      screenshots: [makeFile("a.png", 8), makeFile("b.jpg", 8, "image/jpeg")],
      bundle: makeFile("diagnostics.zip", 16, "application/zip"),
    });
    const parsed = expectOk(parseFeedbackForm(form));
    expect(parsed.sourceId).toBe(SOURCE);
    expect(parsed.screenshots).toHaveLength(2);
    expect(parsed.bundle).toBeInstanceOf(File);
    expect(parsed.meta).toEqual({ appVersion: "6.25.0", os: "windows" });
  });

  it("无截图无诊断包也合法（纯文字反馈）", () => {
    const parsed = expectOk(parseFeedbackForm(makeForm()));
    expect(parsed.screenshots).toHaveLength(0);
    expect(parsed.bundle).toBeNull();
  });

  it("sourceId 只认 32 位小写 hex", () => {
    for (const bad of ["", "XYZ", "0123456789ABCDEF0123456789ABCDEF", "0".repeat(31)]) {
      expect(parseFeedbackForm(makeForm({ sourceId: bad })).ok).toBe(false);
    }
  });

  it("appVersion 与 ping 同一套形状闸（拒任意文本/超长）", () => {
    expect(parseFeedbackForm(makeForm({ appVersion: "6.25.0-beta.1" })).ok).toBe(true);
    for (const bad of ["", "has space", "a".repeat(40), "6.20.0/../../etc"]) {
      expect(parseFeedbackForm(makeForm({ appVersion: bad })).ok).toBe(false);
    }
  });

  it("description 拒空串与超长", () => {
    expect(parseFeedbackForm(makeForm({ description: "" })).ok).toBe(false);
    expect(
      parseFeedbackForm(makeForm({ description: "字".repeat(MAX_DESCRIPTION_CHARS + 1) })).ok,
    ).toBe(false);
  });

  it("meta 必须是体积受限的 JSON 对象", () => {
    expect(parseFeedbackForm(makeForm({ meta: "not-json" })).ok).toBe(false);
    expect(parseFeedbackForm(makeForm({ meta: "[1,2]" })).ok).toBe(false);
    expect(parseFeedbackForm(makeForm({ meta: '"text"' })).ok).toBe(false);
    expect(
      parseFeedbackForm(makeForm({ meta: JSON.stringify({ k: "x".repeat(MAX_META_BYTES) }) })).ok,
    ).toBe(false);
  });

  it("截图超张数 / 超单张体积 / 非图片一律拒", () => {
    const tooMany = Array.from({ length: MAX_SCREENSHOTS + 1 }, (_, i) =>
      makeFile(`s${i}.png`, 8),
    );
    expect(parseFeedbackForm(makeForm({ screenshots: tooMany })).ok).toBe(false);

    expect(
      parseFeedbackForm(makeForm({ screenshots: [makeFile("big.png", 5 * 1024 * 1024 + 1)] })).ok,
    ).toBe(false);

    expect(
      parseFeedbackForm(makeForm({ screenshots: [makeFile("a.txt", 8, "text/plain")] })).ok,
    ).toBe(false);
  });

  it("bundle 超过整包上限拒，缺省为 null", () => {
    expect(
      parseFeedbackForm(
        makeForm({ bundle: makeFile("z.zip", 10 * 1024 * 1024 + 1, "application/zip") }),
      ).ok,
    ).toBe(false);
    expect(expectOk(parseFeedbackForm(makeForm())).bundle).toBeNull();
  });
});

describe("issue 组装", () => {
  const base = expectOk(parseFeedbackForm(makeForm()));

  it("标题取描述首行并截断", () => {
    expect(buildIssueTitle("第一行\n第二行")).toBe("[App 反馈] 第一行");
    expect(buildIssueTitle("长".repeat(80))).toBe(`[App 反馈] ${"长".repeat(50)}`);
  });

  it("正文含描述、截图内联、诊断包链接与环境摘要", () => {
    const body = buildIssueBody(base, ["https://metrics.example/feedback-asset/x.png"], "https://metrics.example/feedback-asset/y.zip");
    expect(body).toContain("示例反馈");
    expect(body).toContain("![截图](https://metrics.example/feedback-asset/x.png)");
    expect(body).toContain("[diagnostics.zip](https://metrics.example/feedback-asset/y.zip)");
    expect(body).toContain("```json");
    expect(body).toContain(`sourceId: \`${SOURCE}\``);
  });

  it("无附件时正文不含截图与诊断包段", () => {
    const body = buildIssueBody(base, [], null);
    expect(body).not.toContain("### 截图");
    expect(body).not.toContain("### 诊断包");
  });
});

describe("dayUtc", () => {
  it("钉死 UTC 日串口径（限流键与 KV key 共用）", () => {
    expect(dayUtc(0)).toBe("1970-01-01");
    // 2026-09-18T23:30Z = epoch 1789774200（跨日边界前的最后半小时）
    expect(dayUtc(1789774200)).toBe("2026-09-18");
  });
});
