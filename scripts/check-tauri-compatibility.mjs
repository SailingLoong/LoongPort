import { spawnSync } from "node:child_process";
import {
  readFileSync,
  mkdtempSync,
  writeFileSync,
  rmSync,
  existsSync,
} from "node:fs";
import os from "node:os";
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

export function runTauriDiagnostics({ root, declared, pnpmEntry }) {
  const emptyReport = { status: null, stdout: "", stderr: "" };
  if (!pnpmEntry || !existsSync(pnpmEntry))
    return {
      ok: false,
      reason: "Pinned pnpm entry is unavailable; run pnpm check:tauri.",
      report: emptyReport,
    };
  const temporary = mkdtempSync(
    path.join(os.tmpdir(), "loongport-tauri-info-"),
  );
  try {
    const forwarder = fileURLToPath(
      new URL("./check-tauri-pnpm.mjs", import.meta.url),
    );
    const receipt = path.join(temporary, "queries.jsonl");
    const quote = (value) => "'" + value.replaceAll("'", "'\"'\"'") + "'";
    if (process.platform === "win32") {
      writeFileSync(
        path.join(temporary, "pnpm.cmd"),
        `@echo off\r\n"${process.execPath}" "${forwarder}" %*\r\n`,
      );
    } else {
      writeFileSync(
        path.join(temporary, "pnpm"),
        `#!/bin/sh\nexec ${quote(process.execPath)} ${quote(forwarder)} "$@"\n`,
        { mode: 0o700 },
      );
    }
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
          PATH: temporary + path.delimiter + process.env.PATH,
          LOONGPORT_TAURI_PNPM_ENTRY: pnpmEntry,
          LOONGPORT_TAURI_QUERY_RECEIPT: receipt,
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
    const parsed = evaluateTauriInfo(report, declared);
    if (!parsed.ok) return { ...parsed, report };
    let queries;
    try {
      queries = readFileSync(receipt, "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line));
    } catch {
      return {
        ok: false,
        reason:
          "Official bulk package comparison did not provide execution evidence.",
        report,
      };
    }
    if (
      !queries.length ||
      queries.some(
        (query) =>
          query.error ||
          query.status !== 0 ||
          !query.dependencies ||
          Object.values(query.dependencies).some(
            (item) =>
              typeof item?.version !== "string" ||
              !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(item.version),
          ) ||
          declared.some(
            (name) =>
              typeof query.dependencies[name]?.version !== "string" ||
              !/^\d+\.\d+\.\d+/.test(query.dependencies[name].version),
          ),
      )
    ) {
      return {
        ok: false,
        reason: "Official bulk package resolution failed or was incomplete.",
        report,
      };
    }
    return { ...parsed, report };
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
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
  const result = runTauriDiagnostics({
    root,
    declared,
    pnpmEntry: process.env.npm_execpath,
  });
  const report = result.report;
  process.stdout.write(report.stdout ?? "");
  process.stderr.write(report.stderr ?? "");
  console.log(`\n[Tauri compatibility] ${result.reason}`);
  process.exitCode = result.ok ? 0 : 1;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main();
}
