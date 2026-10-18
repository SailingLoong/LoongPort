import { invoke } from "@tauri-apps/api/core";
import type { CodexReasoningMode } from "@/types";
import type { AppId } from "./types";
import {
  isProviderEditRequest,
  matchesProviderEditResult,
  type ProviderEditRequest,
  type ProviderEditResult,
  type ProviderEditPreview,
} from "./providers";

export interface ApplicationModelSettingsRequest extends ProviderEditRequest {
  scope: "providerModelSettings";
}
export interface ApplicationModelSettingsResult extends Omit<
  ProviderEditResult,
  "request"
> {
  request: ApplicationModelSettingsRequest;
}
export interface ApplicationModelSettingsPreview extends Omit<
  ProviderEditPreview,
  "request"
> {
  request: ApplicationModelSettingsRequest;
  conflicts?: string[];
}
export function isApplicationModelSettingsRequest(
  value: unknown,
): value is ApplicationModelSettingsRequest {
  return (
    isProviderEditRequest(value) &&
    (value as ApplicationModelSettingsRequest).scope === "providerModelSettings"
  );
}
export function matchesApplicationModelSettingsResult(
  value: unknown,
  app: AppId,
  request: ApplicationModelSettingsRequest,
): value is ApplicationModelSettingsResult {
  return (
    matchesProviderEditResult(value, app, request) &&
    isApplicationModelSettingsRequest(value.request)
  );
}
export function completedApplicationModelSettingsEdit(
  value: unknown,
  app: AppId,
  request: ApplicationModelSettingsRequest,
): value is ApplicationModelSettingsResult {
  return (
    matchesApplicationModelSettingsResult(value, app, request) &&
    value.status === "completed"
  );
}

/** Native resolver facts only. Never infer these from names or level lists. */
export type ReasoningMode = CodexReasoningMode;
export interface ReasoningCapabilities {
  levels: string[];
  defaultLevel: string | null;
  mode: ReasoningMode;
  source: "appDeclaration" | "endpointMetadata" | "exactProfile" | "unknown";
  conflicts: string[];
  verified: boolean;
  transport: {
    kind: "unknown" | "explicitTemplate" | "exactEndpoint";
    supportsThinking: boolean | null;
    supportsEffort: boolean | null;
    thinkingParam: string | null;
    effortParam: string | null;
    effortValueMode: string | null;
  };
  controls: { on: boolean; off: boolean; effort: boolean };
}
export type ReasoningIntent =
  | { kind: "serviceDefault" }
  | { kind: "off" }
  | { kind: "on" }
  | { kind: "effort"; level: string };
export type FieldChange<T> =
  { action: "set"; value: T } | { action: "restore" };
export interface ReasoningDeclaration {
  reasoningMode: ReasoningMode;
  reasoningLevels: string[];
  defaultReasoningLevel?: string;
}
export interface ApplicationModelSettingsPatch {
  confirmSourceChange?: boolean;
  /** Existing full-row CAS revision shown with the source-change effects. */
  sourceChangeRevision?: string;
  model?: FieldChange<string>;
  reasoning?: FieldChange<ReasoningIntent>;
  declaration?: { model: string; change: FieldChange<ReasoningDeclaration> };
  transport?: FieldChange<{ templateId: string }>;
}
export interface ApplicationModelOption {
  model: string;
  capabilities: ReasoningCapabilities;
  declaration?: ReasoningDeclaration;
  declarationOverride?: boolean;
  canRestoreDeclaration?: boolean;
  restoreUnavailableReason?: string;
}
export interface ApplicationModelSettings {
  app: AppId;
  providerId: string;
  revision: string;
  models: ApplicationModelOption[];
  selectedModel: string | null;
  reasoningIntent: ReasoningIntent;
  overrides: { model: boolean; reasoning: boolean; transport?: boolean };
  templateOptions?: Array<{ id: string; label: string }>;
  selectedTemplateId?: string | null;
  canRestoreTransport?: boolean;
  canRestoreModel: boolean;
  canRestoreReasoning: boolean;
  restoreUnavailableReason?: string;
  sourceChanged: boolean;
  sourceChangeEffects?: { declarationModels: string[]; transport: boolean };
  editable: boolean;
  conflicts: string[];
  restartRequired: boolean;
  /** Native ownership decision: manual config/catalog requires the original editor. */
  manualConfig: boolean;
  originalSave?: ApplicationModelSettingsResult | null;
}

/** Narrow adapter to the existing provider owner, not a second settings store. */
export const applicationModelSettingsApi = {
  get: (app: AppId, providerId: string) =>
    invoke<ApplicationModelSettings>("get_application_model_settings", {
      app,
      providerId,
    }),
  preview: (
    app: AppId,
    providerId: string,
    patch: ApplicationModelSettingsPatch,
    requestId: string,
  ) =>
    invoke<ApplicationModelSettingsPreview>(
      "preview_application_model_settings",
      {
        app,
        providerId,
        patch,
        requestId,
      },
    ),
  confirm: (
    app: AppId,
    providerId: string,
    patch: ApplicationModelSettingsPatch,
    request: ApplicationModelSettingsRequest,
  ) =>
    invoke<ApplicationModelSettingsResult>(
      "confirm_application_model_settings",
      {
        app,
        providerId,
        patch,
        request,
      },
    ),
  query: (app: AppId, request: ApplicationModelSettingsRequest) =>
    invoke<ApplicationModelSettingsResult>("query_provider_edit", {
      app,
      request,
    }),
};
