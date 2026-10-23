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
  source?:
    "internationalQoderPat" | "chinaQoderPat" | "chinaQoderDevice" | null;
  status: QoderHttpIdentityStatus;
  expiresAtMs: number | null;
  allowedActions: QoderHttpIdentityAction[];
  allowedDeviceActions?: Array<"login" | "rebind">;
}

export interface QoderHttpIdentityRequest {
  declaration: {
    providerId: string;
    credentialGeneration: string;
    source: "internationalQoderPat" | "chinaQoderPat";
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
    if (
      context.source !== "internationalQoderPat" &&
      context.source !== "chinaQoderPat"
    ) {
      return Promise.reject(new Error("qoder.http_identity.invalid_source"));
    }
    const request: QoderHttpIdentityRequest = {
      declaration: {
        providerId: context.providerId,
        credentialGeneration: context.credentialGeneration,
        source: context.source,
      },
      action,
    };
    return invoke("qoder_http_identity_action", { request });
  },
};
