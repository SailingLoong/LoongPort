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

  it("后端按 VENDOR_APPS 过滤当前平台", () => {
    const backend = read("src-tauri/src/commands/vendor.rs");
    expect(backend).toContain("vendor_supports_app");
    expect(backend).toContain("provision::VENDOR_APPS.contains(app_type)");
  });

  it("官网行切换资格由后端 DTO 给出", () => {
    const api = read("src/lib/api/vendor.ts");
    const row = read("src/components/relay/VendorRow.tsx");

    // 资格是**按 plan** 给的（多 plan 厂商一行两档，各自判）；行组件只消费
    // DTO 上的值，不自己推导。
    expect(api).toContain("canSwitch: boolean");
    expect(api).toContain("plans: VendorPlanInfo[]");
    // 行级主按钮在单 plan 分支读 `plan?.canSwitch`（可选链：多 plan 行传 null）。
    expect(row).toContain("plan?.canSwitch");
  });
});
