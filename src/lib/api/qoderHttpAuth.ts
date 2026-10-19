import { invoke } from "@tauri-apps/api/core";

export type QoderHttpIdentityAction = "establish" | "recover" | "rebind";
export type QoderHttpIdentityStatus =
  | "unavailable"
  | "unestablished"
  | "ready"
  | "refreshNeeded"
  | "reestablishRequired"
  | "pending"
  | "unknown";

// Only the original source revision and safe presentation state cross IPC.
export interface QoderHttpIdentityContext {
  providerId: string;
  credentialGeneration: string;
  status: QoderHttpIdentityStatus;
  expiresAtMs: number | null;
  allowedActions: QoderHttpIdentityAction[];
}

export interface QoderHttpIdentityRequest {
  declaration: {
    providerId: string;
    credentialGeneration: string;
    source: "internationalQoderPat";
  };
  action: QoderHttpIdentityAction;
}

export type QoderHttpIdentityFailureCode =
  | "unavailable"
  | "source_changed"
  | "owner_changed"
  | "invalid_source"
  | "state_changed"
  | "identity_rebind_required"
  | "outcome_unknown";

export type QoderHttpIdentityResult =
  | { status: "completed" | "changed"; context: QoderHttpIdentityContext }
  | { status: "cancelled" }
  | { status: "failed"; code: QoderHttpIdentityFailureCode };

export const qoderHttpAuthApi = {
  context: (providerId: string): Promise<QoderHttpIdentityContext> =>
    invoke("qoder_http_identity_context", { providerId }),
  action: (
    context: QoderHttpIdentityContext,
    action: QoderHttpIdentityAction,
  ): Promise<QoderHttpIdentityResult> => {
    const request: QoderHttpIdentityRequest = {
      declaration: {
        providerId: context.providerId,
        credentialGeneration: context.credentialGeneration,
        source: "internationalQoderPat",
      },
      action,
    };
    return invoke("qoder_http_identity_action", { request });
  },
};
