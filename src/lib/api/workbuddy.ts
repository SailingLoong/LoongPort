import { invoke } from "@tauri-apps/api/core";
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
  /** Optional persisted facts from the WorkBuddy owner; absent on older snapshots. */
  lastSuccessfulClaim?: {
    confirmedAtMs: number | null;
    timeSource: "localReconciled" | null;
    credited: number | null;
    unit: string | null;
  } | null;
  /** `known` requires an actual credit expiry, never a reset or activity deadline. */
  creditExpiryStatus?: "known" | "unavailable" | "stale";
  /** Earliest future actual expiry batch with remaining credit, when known. */
  nextKnownCreditExpiry?: {
    expiresAtMs: number;
    remaining: number | null;
    unit: string | null;
  } | null;
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
};
