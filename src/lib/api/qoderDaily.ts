import { invoke } from "@tauri-apps/api/core";

export interface QoderClaimContext {
  providerId: string;
  credentialGeneration: string;
  deviceDependency: string | null;
}
export interface QoderAutomaticContext {
  providerId: string;
  credentialGeneration: string;
  revision: number;
  policyVersion: number;
  enabled: boolean;
  canEnable: boolean;
  reason: string | null;
  subject: string | null;
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
  automaticContext: (providerId: string): Promise<QoderAutomaticContext> =>
    invoke("qoder_daily_automatic_context", { providerId }),
  setAutomatic: (
    context: QoderAutomaticContext,
    enabled: boolean,
  ): Promise<QoderAutomaticContext> =>
    invoke("set_qoder_daily_automatic", {
      request: {
        providerId: context.providerId,
        credentialGeneration: context.credentialGeneration,
        expectedRevision: context.revision,
        policyVersion: context.policyVersion,
        enabled,
        source: enabled ? "internationalQoderPat" : null,
      },
    }),
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
