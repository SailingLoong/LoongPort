import { invoke } from "@tauri-apps/api/core";
import type { AppId } from "./types";

export interface DiagnosticsExportResult {
  filePath: string;
  bytes: number;
}

export const diagnosticsApi = {
  /**
   * 导出诊断包（环境 + 脱敏日志 + 可选站点域名清单）。
   * `null` = 用户在保存对话框取消，不是错误。
   */
  exportDiagnostics: (includeSites: boolean) =>
    invoke<DiagnosticsExportResult | null>("export_diagnostics", {
      includeSites,
    }),
};

export interface ClipboardImage {
  width: number;
  height: number;
  rgbaBase64: string;
}

export interface FeedbackScreenshotPayload {
  name: string;
  /** data URL（`data:image/...;base64,`）或裸 base64，后端两者都认。 */
  base64: string;
}

export type FeedbackRelatedApp = AppId | "other";

export interface FeedbackFormCapabilities {
  extendedFields: boolean;
}

// type 别名（非 interface）：invoke 参数需要隐式索引签名。
export type FeedbackPrepareInput = {
  description: string;
  relatedApps: FeedbackRelatedApp[];
  /** User-supplied follow-up contact only; never part of the retained receipt. */
  contact?: string;
  includeDiagnostics: boolean;
  includeSites: boolean;
  screenshots: FeedbackScreenshotPayload[];
};

export type FeedbackSubmitInput = { previewId: string };

export interface FeedbackPreparedPreview {
  previewId: string;
  expiresInSeconds: number;
  files: {
    name: string;
    kind: "screenshot" | "diagnostics";
    bytes: number;
    sha256: string;
  }[];
  summary: FeedbackReportSummary;
  totalBytes: number;
  applicationConfigs: Record<string, unknown> | null;
}

/** Safe upload counters; description and screenshot contents are not retained here. */
export interface FeedbackReportSummary {
  descriptionIncluded: boolean;
  contactIncluded: boolean;
  screenshotCount: number;
  screenshotBytes: number;
  diagnosticsIncluded: boolean;
  diagnosticsBytes: number;
}

/** The native owner resolves presence; the renderer never receives a private path. */
export type FeedbackLocalCopy =
  | {
      status: "available" | "missing";
      fileName: "diagnostics.zip";
      bytes: number;
    }
  | { status: "notAttached" };

export type FeedbackReportIdentity =
  { kind: "report"; id: string } | { kind: "github_issue"; number: number };

export interface FeedbackReceipt {
  /** Native-owner comparison key only; display identity instead. */
  reportId: string;
  identity: FeedbackReportIdentity;
  uploadedAt: string;
  summary: FeedbackReportSummary;
  localCopy: FeedbackLocalCopy;
}

export type FeedbackSubmitResult =
  | {
      status: "accepted";
      receipt: FeedbackReceipt | null;
      localSave: "stored" | "failed" | "unavailable";
    }
  | { status: "unknown" };

export const feedbackApi = {
  /** Native owner confirms safe protocol support; unknown support is false. */
  getFormCapabilities: () =>
    invoke<FeedbackFormCapabilities>("feedback_get_form_capabilities"),
  /** 端点是否已随签名远端配置下发 —— 反馈入口按钮显隐的唯一判据。 */
  isEndpointConfigured: () =>
    invoke<boolean>("feedback_get_endpoint_configured"),
  /** 读剪贴板截图（RGBA 原始字节 + 尺寸；PNG 编码在前端 canvas 做）。 */
  readClipboardImage: () => invoke<ClipboardImage>("read_clipboard_image"),
  prepare: (input: FeedbackPrepareInput) =>
    invoke<FeedbackPreparedPreview>("prepare_feedback", input),
  cancelPreview: (previewId: string) =>
    invoke<void>("feedback_cancel_preview", { previewId }),
  /** Submit only bytes already frozen and reviewed through prepare_feedback. */
  submit: (input: FeedbackSubmitInput) =>
    invoke<FeedbackSubmitResult>("submit_feedback", input),
  getLastReport: () =>
    invoke<FeedbackReceipt | null>("feedback_get_last_report"),
  /** Export the retained diagnostics ZIP, not a newly collected report. False = cancelled. */
  exportLastDiagnostics: (expectedReportId: string) =>
    invoke<boolean>("feedback_export_last_diagnostics", { expectedReportId }),
  /** Deletes only the local record and ZIP. The server report is unaffected. */
  deleteLocalReport: (expectedReportId: string) =>
    invoke<void>("feedback_delete_local_report", { expectedReportId }),
};
