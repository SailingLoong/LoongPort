import { invoke } from "@tauri-apps/api/core";
export interface WorkBuddyNativeSelectionKey {
  id: string;
  generation: number;
  incarnation: string;
}
export interface WorkBuddyNativeCaptureAccount {
  selection: WorkBuddyNativeSelectionKey;
  label: string;
  realm: string;
}
export interface WorkBuddyNativeCaptureInput {
  label: string;
  expected?: WorkBuddyNativeSelectionKey | null;
}
export interface WorkBuddyNativeCaptureSaved {
  state: "saved";
  account: WorkBuddyNativeCaptureAccount;
  activationAvailable: false;
}
export interface WorkBuddyNativeCaptureList {
  accounts: WorkBuddyNativeCaptureAccount[];
  /** A catalog snapshot never proves the outcome of a lost request. */
  requestOutcome: "unknown";
  activationAvailable: false;
}
const nativeCaptureFailures = [
  "unsafePath",
  "io",
  "journal",
  "sourceChanged",
  "recoveryRequired",
  "requestConflict",
  "rolledBack",
] as const;
export type WorkBuddyNativeCaptureFailure =
  | (typeof nativeCaptureFailures)[number]
  | "invalidInput"
  | "invalidResponse"
  | "unknown"
  | "cancelled";
/** Local errors retain no transport payload or credentials. */
export class WorkBuddyNativeCaptureError extends Error {
  constructor(public readonly code: WorkBuddyNativeCaptureFailure) {
    super(code);
    this.name = "WorkBuddyNativeCaptureError";
  }
}
function nativeRecord(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}
function nativeSelection(value: unknown): WorkBuddyNativeSelectionKey {
  const v = nativeRecord(value);
  if (
    typeof v.id !== "string" ||
    !v.id.trim() ||
    typeof v.generation !== "number" ||
    !Number.isSafeInteger(v.generation) ||
    v.generation < 0 ||
    typeof v.incarnation !== "string" ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      v.incarnation,
    )
  )
    throw new WorkBuddyNativeCaptureError("invalidResponse");
  return { id: v.id, generation: v.generation, incarnation: v.incarnation };
}
function nativeCaptureAccount(value: unknown): WorkBuddyNativeCaptureAccount {
  const v = nativeRecord(value);
  if (typeof v.label !== "string" || typeof v.realm !== "string")
    throw new WorkBuddyNativeCaptureError("invalidResponse");
  return {
    selection: nativeSelection(v.selection),
    label: v.label,
    realm: v.realm,
  };
}
function nativeCaptureInput(value: WorkBuddyNativeCaptureInput) {
  const v = nativeRecord(value);
  try {
    if (
      typeof v.label !== "string" ||
      Object.keys(v).some((key) => !["label", "expected"].includes(key))
    )
      throw new Error();
    if (
      v.expected != null &&
      Object.keys(nativeRecord(v.expected)).some(
        (key) => !["id", "generation", "incarnation"].includes(key),
      )
    )
      throw new Error();
    return {
      label: v.label,
      expected: v.expected == null ? null : nativeSelection(v.expected),
    };
  } catch {
    throw new WorkBuddyNativeCaptureError("invalidInput");
  }
}
/** Abort stops this caller's wait only. It never cancels/replays the backend worker. */
function nativeCaptureRequest<T>(
  command: string,
  args: Record<string, unknown> | undefined,
  decode: (value: unknown) => T,
  signal?: AbortSignal,
): Promise<T> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (operation: () => void) => {
      if (settled) return;
      settled = true;
      signal?.removeEventListener("abort", cancel);
      operation();
    };
    const cancel = () =>
      finish(() => reject(new WorkBuddyNativeCaptureError("cancelled")));
    if (signal?.aborted) {
      cancel();
      return;
    }
    signal?.addEventListener("abort", cancel, { once: true });
    const failed = (cause: unknown) => {
      const message =
        typeof cause === "string"
          ? cause
          : cause instanceof Error
            ? cause.message
            : undefined;
      const code =
        nativeCaptureFailures.find((value) => value === message) ?? "unknown";
      finish(() => reject(new WorkBuddyNativeCaptureError(code)));
    };
    try {
      const pending =
        args === undefined
          ? invoke<unknown>(command)
          : invoke<unknown>(command, args);
      pending
        .then((value) => {
          if (settled) return;
          try {
            const decoded = decode(value);
            finish(() => resolve(decoded));
          } catch {
            finish(() =>
              reject(new WorkBuddyNativeCaptureError("invalidResponse")),
            );
          }
        })
        .catch(failed);
    } catch (cause) {
      failed(cause);
    }
  });
}

export type WorkBuddyClaimState =
  | "unconfirmed"
  | "available"
  | "claimed"
  | "alreadyClaimed"
  | "unavailable"
  | "needsVerification"
  | "checkinRestricted";
export interface WorkBuddyAccount {
  generation: number;
  autoClaimEnabled: boolean;
  autoClaimRevision: number;
  autoClaimPolicyVersion: number;
  canSetAutomatic: boolean;
  autoClaimBlocked: boolean;
  id: string;
  label: string;
  canRefresh: boolean;
  canClaim: boolean;
  claimState: WorkBuddyClaimState;
  credited: number | null;
  credits: {
    totalRemaining: number | null;
    nearestExpiry: number | null;
    updatedAt: number | null;
    packages: {
      id: string;
      name: string;
      remaining: number | null;
      expireAt: number | null;
    }[];
  };
}
export interface WorkBuddyLogin {
  flowId: string;
  verificationUri: string;
}
export type WorkBuddyFailure =
  | "unconfirmed"
  | "needsVerification"
  | "unsupportedContext"
  | "storageUnavailable"
  | "busy"
  | "expiredAuthorization"
  | "inferencePaused";
export interface WorkBuddyClaimResult {
  account: WorkBuddyAccount;
  error: WorkBuddyFailure | null;
}
export type WorkBuddyInferenceState =
  "needsAuthorization" | "ready" | "refreshRequired" | "expired" | "paused";
export interface WorkBuddyInferenceAccount {
  id: string;
  state: WorkBuddyInferenceState;
}
export interface WorkBuddyInferenceLogin extends WorkBuddyLogin {
  accountId: string;
}
export interface WorkBuddyInferenceModel {
  id: string;
  name: string;
}
export const workbuddyApi = {
  // Untrusted IPC data is validated and redacted by the observation consumer.
  previewNative: () => invoke<unknown>("preview_workbuddy_native_capture"),
  setInferenceConsent: (id: string, enabled: boolean) =>
    invoke<boolean>("set_workbuddy_inference_consent", { id, enabled }),
  setAutomatic: (account: WorkBuddyAccount, enabled: boolean) =>
    invoke<WorkBuddyAccount>("set_workbuddy_automatic_claim", {
      id: account.id,
      generation: account.generation,
      revision: account.autoClaimRevision,
      policyVersion: account.autoClaimPolicyVersion,
      enabled,
    }),
  listInferenceAccounts: () =>
    invoke<WorkBuddyInferenceAccount[]>("list_workbuddy_inference_accounts"),
  beginInferenceLogin: (id: string) =>
    invoke<WorkBuddyInferenceLogin>("begin_workbuddy_inference_authorization", {
      id,
    }),
  pollInferenceLogin: (flowId: string) =>
    invoke<{ state: "saved" | "waiting" }>(
      "poll_workbuddy_inference_authorization",
      { flowId },
    ),
  cancelInferenceLogin: (flowId: string) =>
    invoke<void>("cancel_workbuddy_inference_authorization", { flowId }),
  discoverModels: (id: string) =>
    invoke<WorkBuddyInferenceModel[]>("discover_workbuddy_inference_models", {
      id,
    }),
  list: () => invoke<WorkBuddyAccount[]>("list_workbuddy_accounts"),
  refresh: (id: string) =>
    invoke<WorkBuddyAccount>("refresh_workbuddy_account", { id }),
  refreshAll: () =>
    invoke<WorkBuddyAccount[]>("refresh_all_workbuddy_accounts"),
  claim: (id: string) =>
    invoke<WorkBuddyAccount>("claim_workbuddy_today", { id }),
  claimAll: () => invoke<WorkBuddyClaimResult[]>("claim_all_workbuddy_today"),
  cancelLogin: (flowId: string) =>
    invoke<void>("cancel_workbuddy_authorization", { flowId }),
  beginLogin: () => invoke<WorkBuddyLogin>("begin_workbuddy_authorization"),
  finishLogin: (flowId: string) =>
    invoke<{ state: "saved" | "waiting" }>("finish_workbuddy_authorization", {
      flowId,
    }),
  captureNative: async (
    input: WorkBuddyNativeCaptureInput,
    signal?: AbortSignal,
  ) =>
    nativeCaptureRequest<WorkBuddyNativeCaptureSaved>(
      "capture_workbuddy_native_account",
      { input: nativeCaptureInput(input) },
      (value) => {
        const v = nativeRecord(value);
        if (v.state !== "saved" || v.activationAvailable !== false)
          throw new Error();
        return {
          state: "saved",
          account: nativeCaptureAccount(v.account),
          activationAvailable: false,
        };
      },
      signal,
    ),
  listNativeCaptures: (signal?: AbortSignal) =>
    nativeCaptureRequest<WorkBuddyNativeCaptureList>(
      "list_workbuddy_native_captures",
      undefined,
      (value) => {
        const v = nativeRecord(value);
        if (
          !Array.isArray(v.accounts) ||
          v.requestOutcome !== "unknown" ||
          v.activationAvailable !== false
        )
          throw new Error();
        return {
          accounts: v.accounts.map(nativeCaptureAccount),
          requestOutcome: "unknown",
          activationAvailable: false,
        };
      },
      signal,
    ),
};
