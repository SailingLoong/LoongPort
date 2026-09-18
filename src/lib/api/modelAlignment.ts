import { invoke } from "@tauri-apps/api/core";

/**
 * 模型对齐告警的前端契约（后端 `proxy::model_alignment` 是字段与语义的唯一源）。
 *
 * 「模型不符」= 客户端点名的模型 ≠ 实际计费模型（代理已按档位对齐转发）。
 * 前端只展示事实与转发用户动作，不自行推导。
 */
export interface ModelMismatch {
  appType: string;
  providerId: string;
  providerName: string;
  requestedModel: string;
  sentModel: string;
  /** 请求模型是否在该档位可用模型列表内（「改用」按钮可用性，后端判定）。 */
  canSwitchToRequested: boolean;
}

export const modelAlignmentApi = {
  list: (): Promise<ModelMismatch[]> => invoke("get_active_model_mismatches"),

  /** 用户选择「保持档位模型」：该对不符本会话静默。 */
  dismiss: (mismatch: ModelMismatch): Promise<void> =>
    invoke("dismiss_model_mismatch", {
      appType: mismatch.appType,
      providerId: mismatch.providerId,
      requestedModel: mismatch.requestedModel,
      sentModel: mismatch.sentModel,
    }),
};
