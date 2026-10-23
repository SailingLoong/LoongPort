import { invoke } from "@tauri-apps/api/core";
import type { QoderAutomaticContext } from "./qoderDaily";

export interface QoderCnCampaignDeclaration {
  providerId: string;
  credentialGeneration: string;
  source: "chinaQoderPat" | "chinaQoderDevice";
}
export type QoderCnCampaignFailure =
  | "invalid_source"
  | "source_changed"
  | "owner_changed"
  | "owner_unavailable"
  | "identity_required"
  | "identity_pending"
  | "identity_unknown"
  | "identity_expired"
  | "identity_mismatch"
  | "authentication_required"
  | "verification_required"
  | "rate_limited"
  | "redirect_rejected"
  | "http_error"
  | "service_rejected"
  | "invalid_response"
  | "response_too_large"
  | "timeout"
  | "network_error";
export type QoderCnCampaignReadResult =
  | (QoderCnCampaignDeclaration & {
      status: "observed";
      campaigns: Array<{
        campaignId: string;
        campaignKey: string | null;
        claimStatus: "claimable" | "claimed" | "unknown";
        /** Generic activity interval, not a daily claim round. Unix milliseconds. */
        startAt: number | null;
        endAt: number | null;
        benefitAmount: number | null;
      }>;
      runtimeIdentity: "not_requested";
    })
  | { status: "failed"; code: QoderCnCampaignFailure };

export type QoderCnBenefitRunResult =
  | {
      status: "completed";
      providerId: string;
      credentialGeneration: string;
      activities: Array<{
        campaignId: string;
        outcome:
          "claimedDeltaUnknown" | "observedClaimed" | "unknown" | "skipped";
        reason: string | null;
        /** Reported amount, never an increment attributed to this request. */
        benefitAmount: number | null;
      }>;
    }
  | { status: "failed"; code: QoderCnCampaignFailure };

export const qoderCnCampaignsApi = {
  automaticContext: (providerId: string): Promise<QoderAutomaticContext> =>
    invoke("qoder_daily_automatic_context", { providerId }),
  setAutomatic: (
    context: QoderAutomaticContext,
    enabled: boolean,
    source: QoderCnCampaignDeclaration["source"],
  ): Promise<QoderAutomaticContext> =>
    invoke("set_qoder_daily_automatic", {
      request: {
        providerId: context.providerId,
        credentialGeneration: context.credentialGeneration,
        expectedRevision: context.revision,
        policyVersion: context.policyVersion,
        enabled,
        source: enabled ? source : null,
      },
    }),
  claim: (
    declaration: QoderCnCampaignDeclaration,
  ): Promise<QoderCnBenefitRunResult> =>
    invoke("claim_qoder_cn_benefits", {
      declaration: {
        providerId: declaration.providerId,
        credentialGeneration: declaration.credentialGeneration,
        source: declaration.source,
      },
    }),
  read: (
    declaration: QoderCnCampaignDeclaration,
  ): Promise<QoderCnCampaignReadResult> =>
    invoke("read_qoder_cn_campaigns", {
      declaration: {
        providerId: declaration.providerId,
        credentialGeneration: declaration.credentialGeneration,
        source: declaration.source,
      },
    }),
};
