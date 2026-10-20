import { invoke } from "@tauri-apps/api/core";

export type MiniMaxCodeRegion = "cn" | "global";
export type MiniMaxCodeStatus =
  | "unavailable"
  | "claimable"
  | "alreadyClaimed"
  | "claimed"
  | "unknown"
  | "needsLogin";
export type MiniMaxCodeObservedStatus =
  "claimable" | "alreadyClaimed" | "unavailable";
export interface MiniMaxCodeQuotaWindow {
  remainingPercent: number | null;
  resetAtMs: number | null;
  unlimited: boolean;
}
export interface MiniMaxCodePlanQuota {
  fiveHour: MiniMaxCodeQuotaWindow;
  weekly: MiniMaxCodeQuotaWindow;
}
export interface MiniMaxCodeEntitlement {
  quotaState?: "available" | "not-subscribed" | "unavailable";
  quota?: MiniMaxCodePlanQuota | null;
  remainingCredits: string | null;
  hasTokenPlan: boolean | null;
  tokenPlanExpiresAt: number | null;
}
export interface MiniMaxCodeAccount {
  /** Informational snapshot from explicit refresh; absent/unknown is not zero. */
  entitlement?: MiniMaxCodeEntitlement | null;
  id: string;
  label: string;
  region: MiniMaxCodeRegion;
  buildEnv: "prod";
  realUserId: string;
  authGeneration: number;
  credentialVersion: number;
  authorized: boolean;
  status: MiniMaxCodeStatus;
  canClaim: boolean;
  canRefresh: boolean;
  canRecheck: boolean;
  canRetire: boolean;
  operation: {
    attemptId: string;
    /** Unix milliseconds, as are the other *At fields. */
    startedAt: number;
    day: string;
    status: "pending" | "unknown" | "claimed" | "alreadyClaimed";
    creditAmount: number | null;
  } | null;
  lastCheckedAt: number | null;
  lastObservedStatus: MiniMaxCodeObservedStatus | null;
  creditAmount: number | null;
}
export interface MiniMaxCodeLogin {
  flowId: string;
  verificationUri: string;
  userCode: string;
  expiresAt: number;
  intervalSeconds: number;
}
export interface MiniMaxCodeLoginResult {
  state: "waiting" | "saved";
  account: MiniMaxCodeAccount | null;
}
export type MiniMaxCodeFailure =
  | "storageUnavailable"
  | "busy"
  | "needsLogin"
  | "unsupportedContext"
  | "staleIdentity"
  | "invalidResponse"
  | "networkUnavailable"
  | "expiredAuthorization"
  | "authorizationCompleted"
  | "notClaimable";

/** Safe projections of the sole encrypted Code catalog; never API-key accounts. */
export const minimaxCodeApi = {
  list: () => invoke<MiniMaxCodeAccount[]>("list_minimax_code_accounts"),
  refresh: (id: string) =>
    invoke<MiniMaxCodeAccount>("refresh_minimax_code_account", { id }),
  claim: (id: string) =>
    invoke<MiniMaxCodeAccount>("claim_minimax_code_today", { id }),
  recheck: (id: string) =>
    invoke<MiniMaxCodeAccount>("recheck_minimax_code_claim", { id }),
  retire: (id: string) => invoke<void>("retire_minimax_code_account", { id }),
  beginLogin: (region: MiniMaxCodeRegion) =>
    invoke<MiniMaxCodeLogin>("begin_minimax_code_authorization", { region }),
  pollLogin: (flowId: string) =>
    invoke<MiniMaxCodeLoginResult>("poll_minimax_code_authorization", {
      flowId,
    }),
  cancelLogin: (flowId: string) =>
    invoke<void>("cancel_minimax_code_authorization", { flowId }),
};
