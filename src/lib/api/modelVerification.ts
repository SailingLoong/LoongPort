import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

import {
  MODEL_VERIFICATION_CHANGED,
  MODEL_VERIFICATION_PROGRESS,
} from "./events";

export type VerificationVerdict =
  "trusted" | "suspicious" | "anomaly" | "inconclusive";

export type EvidenceLevel =
  "cryptographic" | "protocolBehavior" | "insufficient";

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
  "queued" | "running" | "completed" | "cancelled" | "failed";

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

/** 失败腿的原始请求/响应（诊断边车，只在证据 Failed 时有）。 */
export interface ProbeDiagnostic {
  /** 产出该事实的探针腿：core / identity / tool / structured / stream。 */
  probe: string;
  code: EvidenceCode;
  request: string;
  response: string;
}

export interface VerificationScopeSummary {
  providerId: string;
  appType: string;
  /** 后端决定是否在档位行展示；不确定结论为 null。 */
  badgeVerdict: VerificationVerdict | null;
  /** 后端按严重度、同级最新时间选出的弹窗默认报告。 */
  representativeReport: VerificationReport;
}

export type VerificationSource = "active";

export interface VerificationHistoryEntry {
  source: VerificationSource;
  report: VerificationReport;
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

/** 协议预筛结论（后端 `ModelFitness` 的镜像；判据唯一源是站点 ai-transit 快照）。 */
export type ModelFitness = "supported" | "unsupported_protocol" | "unknown";

/** 验证弹窗的一个模型选项。`unsupported` 项禁选并带原因标签。 */
export interface VerificationModelOption {
  name: string;
  fitness: ModelFitness;
}

export const modelVerificationApi = {
  listModels: (
    providerId: string,
    appType: string,
  ): Promise<VerificationModelOption[]> =>
    invoke("list_verification_models", { providerId, appType }),

  diagnostics: (
    providerId: string,
    appType: string,
    model: string,
  ): Promise<ProbeDiagnostic[]> =>
    invoke("get_model_verification_diagnostics", {
      providerId,
      appType,
      model,
    }),

  start: (target: VerificationTarget): Promise<StartRunResponse> =>
    invoke("start_model_verification", {
      providerId: target.providerId,
      appType: target.appType,
      model: target.model,
    }),

  cancel: (runId: string): Promise<void> =>
    invoke("cancel_model_verification", { runId }),

  listSummaries: (
    providerIds: string[],
    appType: string,
  ): Promise<VerificationScopeSummary[]> =>
    invoke("get_model_verification_summaries", { providerIds, appType }),

  listHistory: (
    providerId: string,
    appType: string,
  ): Promise<VerificationHistoryEntry[]> =>
    invoke("get_model_verification_history", { providerId, appType }),

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
};
