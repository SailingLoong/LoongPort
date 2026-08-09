import { useState, useCallback } from "react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import {
  parseModelProbeVerdict,
  streamCheckProvider,
  type StreamCheckResult,
} from "@/lib/api/connectivity-check";
import type { AppId } from "@/lib/api";

/**
 * 供应商连通性检查。
 *
 * 只探测 base_url 是否可达（任何 HTTP 响应都算可达），不发真实大模型请求。
 * 刻意 **不** 重置故障转移熔断器——可达 ≠ 配置正确，一个端口通但鉴权废的供应商
 * 不应被误判为"健康"而切回线上。熔断器只由真实转发流量驱动（见 proxy/forwarder.rs）。
 */
export function useStreamCheck(appId: AppId) {
  const { t } = useTranslation();
  const [checkingIds, setCheckingIds] = useState<Set<string>>(new Set());

  const formatModelProbe = useCallback(
    (modelUsed: string | undefined) => {
      if (!modelUsed) return undefined;

      const verdict = parseModelProbeVerdict(modelUsed);
      if (!verdict) return modelUsed;

      switch (verdict.kind) {
        case "keyExpired":
          return t("streamCheck.modelProbe.keyExpired", {
            status: verdict.status,
          });
        case "forbidden":
          return t("streamCheck.modelProbe.forbidden", {
            status: verdict.status,
          });
        case "noModels":
          return t("streamCheck.modelProbe.noModels");
        case "imageOnly":
          return t("streamCheck.modelProbe.imageOnly", {
            models: verdict.models.join(" / "),
          });
        case "models":
          return t("streamCheck.modelProbe.models", {
            total: verdict.total,
            models: verdict.head.join(" / "),
          });
      }
    },
    [t],
  );

  const checkProvider = useCallback(
    async (
      providerId: string,
      providerName: string,
    ): Promise<StreamCheckResult | null> => {
      setCheckingIds((prev) => new Set(prev).add(providerId));

      try {
        const result = await streamCheckProvider(appId, providerId);

        if (result.status === "operational") {
          toast.success(
            t("streamCheck.reachable", {
              providerName: providerName,
              responseTimeMs: result.responseTimeMs,
              defaultValue: `${providerName} 连通正常 (${result.responseTimeMs}ms)`,
            }),
            {
              closeButton: true,
              // 「真正能调什么」—— 后端零成本探出来的（见 `modelUsed` 的文档）。
              // **可达 ≠ 可用**：密钥失效、分组没挂模型、只挂生图模型这三种，
              // 可达性探测都报「正常」，而这一行会说清楚。
              description: formatModelProbe(result.modelUsed),
            },
          );
        } else if (result.status === "degraded") {
          toast.warning(
            t("streamCheck.reachableSlow", {
              providerName: providerName,
              responseTimeMs: result.responseTimeMs,
              defaultValue: `${providerName} 连通但较慢 (${result.responseTimeMs}ms)`,
            }),
            // ⚠️ 这一支**同样要显示** `modelUsed`（review 抓出）——「慢」与「坏」是两件
            // 独立的事：一个响应慢的档位同样可能密钥已失效、或只挂了生图模型。
            // 漏在这里等于让这个功能在最需要它的那一类档位上静默失效
            // （结论算出来了、写进日志了，就是不给用户看）。
            { description: formatModelProbe(result.modelUsed) },
          );
        } else {
          // 仅当无法建立连接（DNS / 连接被拒 / TLS / 超时）才会到这里
          toast.error(
            t("streamCheck.unreachable", {
              providerName: providerName,
              message: result.message,
              defaultValue: `${providerName} 无法连通: ${result.message}`,
            }),
            {
              description: t("streamCheck.unreachableHint", {
                defaultValue:
                  "无法建立连接（DNS / 连接 / TLS / 超时）。请检查 base_url 与网络。",
              }),
              duration: 8000,
              closeButton: true,
            },
          );
        }

        return result;
      } catch (e) {
        toast.error(
          t("streamCheck.error", {
            providerName: providerName,
            error: String(e),
            defaultValue: `${providerName} 检查出错: ${String(e)}`,
          }),
        );
        return null;
      } finally {
        setCheckingIds((prev) => {
          const next = new Set(prev);
          next.delete(providerId);
          return next;
        });
      }
    },
    [appId, formatModelProbe, t],
  );

  const isChecking = useCallback(
    (providerId: string) => checkingIds.has(providerId),
    [checkingIds],
  );

  return { checkProvider, isChecking };
}
