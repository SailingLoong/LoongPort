import { invoke } from "@tauri-apps/api/core";

// Public grant handles only. Tokens, UID, verifier and machine ID stay in Rust.
export interface QoderCnDeviceLoginRequest {
  providerId: string | null;
  credentialGeneration: string | null;
  rebind: boolean;
}
export type QoderCnDeviceLoginStartResult =
  | {
      status: "started";
      sessionId: string;
      providerId: string;
      credentialGeneration: string;
      authUrl: string;
      expiresAtMs: number;
      pollIntervalMs: number;
    }
  | { status: "failed"; code: string };
export type QoderCnDeviceLoginPollResult =
  | { status: "pending" }
  | { status: "completed"; providerId: string }
  | { status: "failed"; code: string };

export const qoderCnDeviceLoginApi = {
  start: (
    request: QoderCnDeviceLoginRequest,
  ): Promise<QoderCnDeviceLoginStartResult> =>
    invoke("qoder_cn_device_login_start", {
      request: {
        providerId: request.providerId,
        credentialGeneration: request.credentialGeneration,
        rebind: request.rebind,
      },
    }),
  poll: (sessionId: string): Promise<QoderCnDeviceLoginPollResult> =>
    invoke("qoder_cn_device_login_poll", { sessionId }),
  cancel: (sessionId: string): Promise<{ status: "cancelled" }> =>
    invoke("qoder_cn_device_login_cancel", { sessionId }),
};
