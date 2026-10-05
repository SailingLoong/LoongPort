// Only used in the temporary PATH of the official Tauri info child process.
import { spawnSync } from "node:child_process";
import { appendFileSync } from "node:fs";
const args = process.argv.slice(2);
const result = spawnSync(
  process.execPath,
  [process.env.LOONGPORT_TAURI_PNPM_ENTRY, ...args],
  {
    encoding: "utf8",
    timeout: 20_000,
    maxBuffer: 4 * 1024 * 1024,
  },
);
if (args[0] === "list" && args.includes("--json")) {
  let dependencies = null;
  try {
    const rows = JSON.parse(result.stdout);
    if (Array.isArray(rows) && rows[0] && typeof rows[0] === "object") {
      dependencies = { ...rows[0].dependencies, ...rows[0].devDependencies };
    }
  } catch {
    /* Malformed official query output is recorded as unresolved. */
  }
  appendFileSync(
    process.env.LOONGPORT_TAURI_QUERY_RECEIPT,
    JSON.stringify({
      status: result.status,
      error: Boolean(result.error),
      dependencies,
    }) + "\n",
  );
}
process.stdout.write(result.stdout ?? "");
process.stderr.write(result.stderr ?? "");
process.exitCode = result.error ? 1 : (result.status ?? 1);
