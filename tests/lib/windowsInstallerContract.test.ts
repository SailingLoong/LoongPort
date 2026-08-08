import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const read = (path: string) =>
  readFileSync(resolve(process.cwd(), path), "utf8");

/**
 * Windows 普通安装与应用内更新必须使用同一种 NSIS 产物。
 *
 * latest.json 每个 Windows 架构只能放一个 URL；如果 build 改成 NSIS 而清单仍筛 MSI，
 * 发布会静默失去 Windows 自动更新。反过来，清单继续指向 MSI 就会让每次自动升级重新
 * 进入 Config.Msi rollback 路径，恢复 Error 1926。
 */
describe("Windows NSIS 安装与自动更新契约", () => {
  it("Tauri 默认 bundle 是 NSIS，并挂载旧 MSI 迁移 hook", () => {
    const config = JSON.parse(read("src-tauri/tauri.conf.json"));
    expect(config.bundle.targets).toEqual(["nsis"]);
    // Tauri 模板会先用 ProductName + Publisher 模糊命中 WiX，然后执行
    // 不带静默参数的 UninstallString。与历史 MSI 的 loongport 区分开，
    // 才会让下面按 UpgradeCode 精确识别的 hook 接管迁移。
    expect(config.bundle.publisher).toBe("SailingLoong");
    expect(config.bundle.windows.nsis.installerHooks).toBe(
      "nsis/installer-hooks.nsh",
    );
  });

  it("迁移按已发布 MSI 的 UpgradeCode 精确识别，而不是按显示名模糊删除", () => {
    const hooks = read("src-tauri/nsis/installer-hooks.nsh");
    expect(hooks).toContain("{f6ae9451-300e-59b9-9081-beb400b6cde1}");
    expect(hooks).toContain("MsiEnumRelatedProductsW");
    expect(hooks).toContain("msiexec.exe");
    expect(hooks).toContain("/x $R0 /passive /norestart");
    expect(hooks).toContain("$UpdateMode != 1");
  });

  it("正式发布、产物收集与 latest.json 全部只认 NSIS Setup", () => {
    const workflow = read(".github/workflows/release.yml");
    expect(workflow).toContain("--bundles nsis");
    expect(workflow).toContain("bundle/nsis");
    expect(workflow).toContain("-Windows$assetSuffix-Setup.exe");
    expect(workflow).toContain("*-Windows-Setup.exe)");
    expect(workflow).toContain("*-Windows-arm64-Setup.exe)");
    expect(workflow).not.toContain("--bundles msi");
  });

  it("手动 Windows workflow 也出 Setup，但不要求 release 签名密钥", () => {
    const workflow = read(".github/workflows/windows-build.yml");
    expect(workflow).toContain("--bundles nsis");
    expect(workflow).toContain('"createUpdaterArtifacts":false');
    expect(workflow).toContain("bundle/nsis/*-setup.exe");
    expect(workflow).not.toContain("--bundles msi");
  });
});
