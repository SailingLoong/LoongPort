import { invoke } from "@tauri-apps/api/core";
import type { AppId } from "./types";

// ===== 连通性检查类型 =====
// 注意：本检查只探测 base_url 是否可达，不发真实大模型请求，也不触碰故障转移熔断器。

export type HealthStatus = "operational" | "degraded" | "failed";

export interface StreamCheckConfig {
  /** 单次探测超时（秒） */
  timeoutSecs: number;
  /** 超时类失败的最大重试次数 */
  maxRetries: number;
  /** 降级阈值（毫秒）：可达但 TTFB 超过该值判定为"较慢" */
  degradedThresholdMs: number;
}

export interface StreamCheckResult {
  status: HealthStatus;
  success: boolean;
  message: string;
  responseTimeMs?: number;
  httpStatus?: number;
  testedAt: number;
  retryCount: number;
  /**
   * 这个档位**真正能调什么** —— 后端在可达性之后额外问一次 `/v1/models`（零成本，
   * 不计费）得到的结论。空串 = 没探（非托管档位）或探不出来。
   *
   * 为什么需要它：可达性只答「端口通不通」，而档位真正的失效方式往往在那之后 ——
   * 密钥失效（401）、分组没挂任何模型、或只挂了生图模型却被当对话档位用
   * （实测这三种在可达性探测里全显示「正常」）。
   *
   * 字段名沿用后端 `stream_check_logs` 表的既有列（原本恒为空串），所以历史日志
   * 与批量检查不必改结构就能带上这条信息。
   */
  modelUsed?: string;
}

// ===== 连通性检查 API =====

/**
 * 连通性检查（单个供应商）
 */
export async function streamCheckProvider(
  appType: AppId,
  providerId: string,
): Promise<StreamCheckResult> {
  return invoke("stream_check_provider", { appType, providerId });
}

/**
 * 批量连通性检查
 */
export async function streamCheckAllProviders(
  appType: AppId,
  proxyTargetsOnly: boolean = false,
): Promise<Array<[string, StreamCheckResult]>> {
  return invoke("stream_check_all_providers", { appType, proxyTargetsOnly });
}

/**
 * 获取连通性检查配置
 */
export async function getStreamCheckConfig(): Promise<StreamCheckConfig> {
  return invoke("get_stream_check_config");
}

/**
 * 保存连通性检查配置
 */
export async function saveStreamCheckConfig(
  config: StreamCheckConfig,
): Promise<void> {
  return invoke("save_stream_check_config", { config });
}
