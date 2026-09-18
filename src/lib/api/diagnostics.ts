import { invoke } from "@tauri-apps/api/core";

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

// type 别名（非 interface）：invoke 参数需要隐式索引签名。
export type FeedbackSubmitInput = {
  description: string;
  includeDiagnostics: boolean;
  includeSites: boolean;
  screenshots: FeedbackScreenshotPayload[];
};

export const feedbackApi = {
  /** 端点是否已随签名远端配置下发 —— 反馈入口按钮显隐的唯一判据。 */
  isEndpointConfigured: () =>
    invoke<boolean>("feedback_get_endpoint_configured"),
  /** 读剪贴板截图（RGBA 原始字节 + 尺寸；PNG 编码在前端 canvas 做）。 */
  readClipboardImage: () => invoke<ClipboardImage>("read_clipboard_image"),
  submit: (input: FeedbackSubmitInput) =>
    invoke<{ success: boolean }>("submit_feedback", input),
};
