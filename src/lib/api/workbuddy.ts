import { invoke } from "@tauri-apps/api/core";
export type WorkBuddyClaimState =
  | "unconfirmed"
  | "available"
  | "claimed"
  | "alreadyClaimed"
  | "unavailable"
  | "needsVerification";
export interface WorkBuddyAccount {
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
  | "expiredAuthorization";
export interface WorkBuddyClaimResult {
  account: WorkBuddyAccount;
  error: WorkBuddyFailure | null;
}
export const workbuddyApi = {
  list: () => invoke<WorkBuddyAccount[]>("list_workbuddy_accounts"),
  refresh: (id: string) =>
    invoke<WorkBuddyAccount>("refresh_workbuddy_account", { id }),
  refreshAll: () =>
    invoke<WorkBuddyAccount[]>("refresh_all_workbuddy_accounts"),
  claim: (id: string) =>
    invoke<WorkBuddyAccount>("claim_workbuddy_today", { id }),
  claimAll: () => invoke<WorkBuddyClaimResult[]>("claim_all_workbuddy_today"),
  beginLogin: () => invoke<WorkBuddyLogin>("begin_workbuddy_authorization"),
  finishLogin: (flowId: string) =>
    invoke<{ state: "saved" | "waiting" }>("finish_workbuddy_authorization", {
      flowId,
    }),
};
