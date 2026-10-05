import type { ContextSelection } from "@/lib/api/zcodeAccounts";

const KEY = "loongport:zcode-source-v1";
type SourcePreference = Pick<ContextSelection, "installPath" | "dataRoot">;
const absolute = (value: unknown): value is string =>
  typeof value === "string" &&
  value.startsWith("/") &&
  value.length <= 4096 &&
  !value.includes("\0") &&
  !value.split("/").some((part) => part === "." || part === "..");

/** Paths are untrusted preferences. Never restore admission, key mode or credentials. */
export function readZCodeSourcePreference(): SourcePreference | null {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(KEY) ?? "null");
    if (!value || typeof value !== "object") return null;
    const candidate = value as Record<string, unknown>;
    if (
      candidate.version !== 1 ||
      !absolute(candidate.installPath) ||
      !absolute(candidate.dataRoot)
    )
      return null;
    return { installPath: candidate.installPath, dataRoot: candidate.dataRoot };
  } catch {
    return null;
  }
}
export function saveZCodeSourcePreference(source: SourcePreference): void {
  if (!absolute(source.installPath) || !absolute(source.dataRoot)) return;
  try {
    localStorage.setItem(
      KEY,
      JSON.stringify({
        version: 1,
        installPath: source.installPath,
        dataRoot: source.dataRoot,
      }),
    );
  } catch {
    /* Preferences are optional; persistence failure cannot authorize an action. */
  }
}
