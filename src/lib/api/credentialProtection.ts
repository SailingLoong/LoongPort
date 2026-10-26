import { invoke } from "@tauri-apps/api/core";

// Backend-owned storage protection. Manual recovery is a state, not a mode.
export type CredentialProtectionMode = "encrypted" | "plaintext";
export interface CredentialProtectionStatus {
  mode: CredentialProtectionMode | "unconfigured";
  status: "needs_configuration" | "available" | "key_required" | "blocked";
  shouldPrompt: boolean;
  canConfigure: boolean;
  canUnlock: boolean;
  canSwitchToPlaintext: boolean;
  canCreateSeparateConfiguration: boolean;
  reviewToken: string | null;
  restartRequired: boolean;
  errorCode: string | null;
}
export interface ConfigureCredentialProtectionRequest {
  mode: CredentialProtectionMode;
  keyText?: string;
  preserveExisting: boolean;
  plaintextAcknowledged: boolean;
  reviewToken: string;
}
export const credentialProtectionKey = ["credentialProtection"] as const;

export const credentialProtectionApi = {
  status: () =>
    invoke<CredentialProtectionStatus>("get_credential_protection_status"),
  configure: (request: ConfigureCredentialProtectionRequest) =>
    invoke<CredentialProtectionStatus>("configure_credential_protection", {
      request,
    }),
  unlock: (keyText: string, reviewToken: string) =>
    invoke<CredentialProtectionStatus>("unlock_credential_runtime", {
      keyText,
      reviewToken,
    }),
};
