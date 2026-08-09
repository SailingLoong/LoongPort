import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

import {
  MODEL_VERIFICATION_ANOMALY, MODEL_VERIFICATION_CHANGED,
  MODEL_VERIFICATION_PROGRESS,
} from "./events";

export type VerificationVerdict =
  | "trusted"
  | "suspicious"
  | "anomaly"
  | "inconclusive";

export type EvidenceLevel =
  | "cryptographic"
  | "protocolBehavior"
  | "insufficient";

export type EvidenceCode =
  | "basicEnvelope"
  | "modelMatch"
  | "streamLifecycle"
  | "usageConsistency"
  | "toolCallShape"
  | "structuredOutput"
  | "thinkingSignature"
  | "signatureContinuation"
  | "foreignProtocol"
  | "foreignSelfIdentification";

export type EvidenceOutcome = "passed" | "failed" | "skipped";

export type RunFailureKind =
  | "authentication"
  | "rateLimited"
  | "insufficientBalance"
  | "network"
  | "upstream"
  | "timeout"
  | "modelUnavailable"
  | "cancelled"
  | "invalidResponse";

export type RunState =
  | "queued"
  | "running"
  | "completed"
  | "cancelled"
  | "failed";

export interface VerificationTarget {
  providerId: string;
  appType: string;
  model: string;
}

export interface VerificationScope {
  providerId: string;
  appType: string;
}

export interface EvidenceFact {
  code: EvidenceCode;
  outcome: EvidenceOutcome;
}

export interface VerificationReport {
  target: VerificationTarget;
  verdict: VerificationVerdict;
  evidenceLevel: EvidenceLevel;
  facts: EvidenceFact[];
  rulesVersion: number;
  checkedAt: number;
}

export interface StartRunResponse {
  runId: string;
  state: RunState;
}

export interface VerificationProgressEvent extends VerificationTarget {
  runId: string;
  state: RunState;
  completedChecks: number;
  totalChecks: number;
  failure: RunFailureKind | null;
}

export type RuntimeAppType = "codex" | "claude";
export type RuntimeAppStatus = "active" | "waiting" | "error";
export type RuntimeAppReason =
  | "currentProviderUnsupported"
  | "clientUnavailable"
  | "noCurrentProvider"
  | "takeoverFailed"
  | "recoveryFailed";
export interface RuntimeAppState {
  appType: RuntimeAppType;
  status: RuntimeAppStatus;
  reason: RuntimeAppReason | null;
}
export interface RuntimeVerificationSnapshot {
  setting: { runtimeAutoEnabled: boolean; updatedAt: number };
  apps: RuntimeAppState[];
}
export type AnomalyFingerprint = EvidenceCode;
export interface ModelVerificationAnomalyEvent extends VerificationTarget {
  fingerprint: AnomalyFingerprint;
}

export const modelVerificationApi = {
  listModels: (providerId: string, appType: string): Promise<string[]> =>
    invoke("list_verification_models", { providerId, appType }),

  start: (target: VerificationTarget): Promise<StartRunResponse> =>
    invoke("start_model_verification", {
      providerId: target.providerId,
      appType: target.appType,
      model: target.model,
    }),

  cancel: (runId: string): Promise<void> =>
    invoke("cancel_model_verification", { runId }),

  listResults: (providerIds: string[]): Promise<VerificationReport[]> =>
    invoke("get_model_verification_results", { providerIds }),

  onProgress: (
    handler: (event: VerificationProgressEvent) => void,
  ): Promise<UnlistenFn> =>
    listen<VerificationProgressEvent>(MODEL_VERIFICATION_PROGRESS, (event) =>
      handler(event.payload),
    ),

  onChanged: (
    handler: (scope: VerificationScope) => void,
  ): Promise<UnlistenFn> =>
    listen<VerificationScope>(MODEL_VERIFICATION_CHANGED, (event) =>
      handler(event.payload),
    ),

  getRuntimeSetting: (): Promise<RuntimeVerificationSnapshot> =>
    invoke("get_runtime_verification_setting"),

  setRuntimeEnabled: (enabled: boolean): Promise<RuntimeVerificationSnapshot> =>
    invoke("set_runtime_verification_enabled", { enabled }),

  onAnomaly: (
    handler: (event: ModelVerificationAnomalyEvent) => void,
  ): Promise<UnlistenFn> =>
    listen<ModelVerificationAnomalyEvent>(MODEL_VERIFICATION_ANOMALY, (event) =>
      handler(event.payload),
    ),
};
