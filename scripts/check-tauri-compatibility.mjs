import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const mismatchDiagnostic = "Found version mismatched Tauri packages";

export function evaluateTauriInfo(report, declared) {
  if (report.error || report.status !== 0) {
    return {
      ok: false,
      reason: "Official Tauri diagnostics failed or timed out.",
    };
  }
  const output = `${report.stdout ?? ""}\n${report.stderr ?? ""}`.replace(
    /\u001b\[[0-?]*[ -/]*[@-~]/g,
    "",
  );
  if (output.includes(mismatchDiagnostic)) {
    return {
      ok: false,
      reason: "Official Tauri diagnostics found incompatible packages.",
    };
  }
  if (!declared.includes("@tauri-apps/api")) {
    return {
      ok: false,
      reason: "The desktop project must declare @tauri-apps/api.",
    };
  }
  for (const npmName of declared) {
    const rustName =
      npmName === "@tauri-apps/api"
        ? "tauri"
        : npmName.replace("@tauri-apps/plugin-", "tauri-plugin-");
    for (const name of [npmName, rustName]) {
      const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      const reported = new RegExp(
        `^[ \\t-]*${escaped}\\s+[^:\\n]*:\\s+\\d+\\.\\d+\\.\\d+`,
        "m",
      );
      if (!reported.test(output)) {
        return {
          ok: false,
          reason: `Official Tauri diagnostics did not resolve ${name}.`,
        };
      }
    }
  }
  return {
    ok: true,
    reason: "Official Tauri package compatibility check passed.",
  };
}

function main() {
  const root = fileURLToPath(new URL("../", import.meta.url));
  const manifest = JSON.parse(
    readFileSync(path.join(root, "package.json"), "utf8"),
  );
  const declared = Object.keys({
    ...manifest.dependencies,
    ...manifest.devDependencies,
    ...manifest.optionalDependencies,
  }).filter(
    (name) =>
      name === "@tauri-apps/api" || name.startsWith("@tauri-apps/plugin-"),
  );
  const report = spawnSync(
    process.execPath,
    [path.join(root, "node_modules/@tauri-apps/cli/tauri.js"), "info"],
    {
      cwd: root,
      encoding: "utf8",
      timeout: 120_000,
      maxBuffer: 4 * 1024 * 1024,
      env: {
        ...process.env,
        NO_COLOR: "1",
        // info also looks up latest registry versions for display. Those lookups
        // are irrelevant to its shared build compatibility check; use installed
        // packages and Cargo.lock, without a network-dependent gate.
        npm_config_offline: "true",
        npm_config_fetch_retries: "0",
        npm_config_fetch_timeout: "5000",
      },
    },
  );
  process.stdout.write(report.stdout ?? "");
  process.stderr.write(report.stderr ?? "");
  const result = evaluateTauriInfo(report, declared);
  console.log(`\n[Tauri compatibility] ${result.reason}`);
  process.exitCode = result.ok ? 0 : 1;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main();
}
