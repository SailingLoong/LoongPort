import { invoke } from "@tauri-apps/api/core";

export interface QoderClaimContext {
  providerId: string;
  credentialGeneration: string;
  deviceDependency: string | null;
}
export type QoderClaimResult =
  | { Unavailable: string }
  | {
      Observed: {
        action: "None" | "Post" | "Get";
        outcome: string;
        added_credits: number | null;
        operation: {
          attemptId: string;
          campaignId: string;
          campaignKey: string;
          startAt: number;
          endAt: number;
        };
      };
    };
export const qoderDailyApi = {
  context: (providerId: string): Promise<QoderClaimContext> =>
    invoke("qoder_daily_claim_context", { providerId }),
  claim: (context: QoderClaimContext): Promise<QoderClaimResult> =>
    invoke("claim_qoder_daily_credits", {
      providerId: context.providerId,
      declaration: {
        providerId: context.providerId,
        credentialGeneration: context.credentialGeneration,
        source: "internationalQoderPat",
      },
    }),
};
