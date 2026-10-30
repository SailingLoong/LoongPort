import type {
  WorkBuddyNativeCaptureList,
  WorkBuddyNativeSelectionKey,
} from "@/lib/api/workbuddy";
import type { PlatformAccountSummary } from "./useAccountPlatformPresence";
/** UI read observation only, never admission or actual client-current evidence. */
export type WorkBuddyNativeRowIdentity =
  | {
      status:
        | "unsupported"
        | "unavailable"
        | "loading"
        | "stale"
        | "error"
        | "ambiguous";
      selection: null;
    }
  | { status: "ready"; selection: WorkBuddyNativeSelectionKey };
export interface WorkBuddyNativeCatalogObservation {
  state: "disabled" | "loading" | "stale" | "error" | "success";
  data?: WorkBuddyNativeCaptureList;
  isFetching: boolean;
}
function generation(account: PlatformAccountSummary): number | null {
  if (
    account.source !== "workbuddy" ||
    account.platform !== "workbuddy" ||
    account.snapshot?.kind !== "workbuddy" ||
    account.snapshot.value.id !== account.id ||
    !account.id.trim()
  )
    return null;
  const value = account.snapshot.value.generation;
  return Number.isSafeInteger(value) && value >= 0 ? value : null;
}
export function workbuddyInventorySignature(
  accounts: readonly PlatformAccountSummary[],
): string {
  return JSON.stringify(
    accounts
      .flatMap((account) => {
        const value = generation(account);
        return value === null ? [] : [[account.id, value] as const];
      })
      .sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : a[1] - b[1])),
  );
}
export function resolveWorkBuddyNativeIdentity(
  account: PlatformAccountSummary,
  accounts: readonly PlatformAccountSummary[],
  catalog: WorkBuddyNativeCatalogObservation,
): WorkBuddyNativeRowIdentity {
  const value = generation(account);
  if (value === null) return { status: "unsupported", selection: null };
  if (catalog.state !== "success")
    return {
      status: catalog.state === "disabled" ? "unavailable" : catalog.state,
      selection: null,
    };
  const ordinary = accounts.filter(
    (row) =>
      row.source === "workbuddy" &&
      row.platform === "workbuddy" &&
      row.id === account.id,
  );
  const captures =
    catalog.data?.accounts.filter((row) => row.selection.id === account.id) ??
    [];
  if (ordinary.length !== 1 || captures.length > 1)
    return { status: "ambiguous", selection: null };
  const match = captures[0];
  if (!match || match.selection.generation !== value)
    return { status: "unavailable", selection: null };
  return { status: "ready", selection: { ...match.selection } };
}
