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
   * 字段名沿用后端 `stream_check_logs` 表的既有列（原本恒为空串）。新结果是带 `kind`
   * 的 JSON，历史行里的纯文本仍可作为 legacy 值回退显示，因此不需要改数据库结构。
   */
  modelUsed?: string;
}

export type ModelProbeVerdict =
  | { kind: "keyExpired"; status: number }
  | { kind: "forbidden"; status: number }
  | { kind: "noModels" }
  | { kind: "imageOnly"; models: string[] }
  | { kind: "models"; total: number; head: string[] };

/** Parse the structured verdict stored in the legacy `modelUsed` field. */
export function parseModelProbeVerdict(
  value: string | undefined,
): ModelProbeVerdict | null {
  if (!value) return null;

  try {
    const parsed: unknown = JSON.parse(value);
    if (!parsed || typeof parsed !== "object" || !("kind" in parsed)) {
      return null;
    }

    const candidate = parsed as Record<string, unknown>;
    switch (candidate.kind) {
      case "keyExpired":
      case "forbidden":
        return typeof candidate.status === "number"
          ? { kind: candidate.kind, status: candidate.status }
          : null;
      case "noModels":
        return { kind: "noModels" };
      case "imageOnly":
      case "models":
        if (
          !Array.isArray(candidate.models) &&
          !Array.isArray(candidate.head)
        ) {
          return null;
        }
        if (candidate.kind === "imageOnly") {
          return Array.isArray(candidate.models) &&
            candidate.models.every((model) => typeof model === "string")
            ? { kind: "imageOnly", models: candidate.models }
            : null;
        }
        return typeof candidate.total === "number" &&
          Array.isArray(candidate.head) &&
          candidate.head.every((model) => typeof model === "string")
          ? { kind: "models", total: candidate.total, head: candidate.head }
          : null;
      default:
        return null;
    }
  } catch {
    return null;
  }
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
