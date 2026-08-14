import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const read = (path: string) =>
  readFileSync(resolve(process.cwd(), path), "utf8");

describe("官网账号支持平台的唯一数据源", () => {
  it("前端不复制支持平台列表，只展示后端过滤后的行", () => {
    const api = read("src/lib/api/vendor.ts");
    const section = read("src/components/relay/RelaySection.tsx");

    expect(api).not.toContain("VENDOR_APPS");
    expect(api).not.toContain("vendorSupportsApp");
    expect(section).not.toContain("vendorSupportsApp");
  });

  it("后端按 DEEPSEEK_APPS 过滤当前平台", () => {
    const backend = read("src-tauri/src/commands/vendor.rs");
    expect(backend).toContain("vendor_supports_app");
    expect(backend).toContain("provision::DEEPSEEK_APPS.contains(app_type)");
  });

  it("官网行切换资格由后端 DTO 给出", () => {
    const api = read("src/lib/api/vendor.ts");
    const row = read("src/components/relay/VendorRow.tsx");

    expect(api).toContain("canSwitch: boolean");
    expect(row).toContain("account.canSwitch");
  });
});
