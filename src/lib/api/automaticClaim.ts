import { invoke } from "@tauri-apps/api/core";

export type AutomaticClaimProvider = "workbuddy" | "minimaxCode";
/** Platform scheduler observation, not an account policy or a claim receipt. */
export interface AutomaticClaimSnapshot {
  provider: AutomaticClaimProvider;
  phase: "waiting" | "checking";
  lastCheckedAt: number | null;
  nextCheckAt: number | null;
  result: "completed" | "failed" | "skipped" | null;
  reason:
    "locked" | "disabled" | "pending" | "notEligible" | "checkFailed" | null;
}
export const automaticClaimApi = {
  status: (provider: AutomaticClaimProvider) =>
    invoke<AutomaticClaimSnapshot>("get_automatic_claim_status", { provider }),
};
