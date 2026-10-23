import { invoke } from "@tauri-apps/api/core";

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
      automaticClaimEnabled: false;
      blockedReason: "daily_contract_unverified";
      runtimeIdentity: "not_requested";
    })
  | { status: "failed"; code: QoderCnCampaignFailure };

export const qoderCnCampaignsApi = {
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
