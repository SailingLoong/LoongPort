import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { AlertTriangle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { MODEL_MISMATCH } from "@/lib/api/events";
import {
  modelAlignmentApi,
  type ModelMismatch,
} from "@/lib/api/modelAlignment";
import { relayApi } from "@/lib/api/relay";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { extractErrorMessage } from "@/utils/errorUtils";

/**
 * 模型对齐告警横幅：客户端点名的模型 ≠ 档位已选模型（实际计费模型）时，
 * 常驻告知 + 一键出口。状态机在组件内，`App.tsx` 只挂一行（照
 * `AnnouncementDialog` 先例）；无活跃不符时零存在感。
 *
 * - 数据事实全部来自后端（活跃集 / 「改用」可用性），前端只展示与转发；
 * - 「改用」走标准切模型编排（`relay_switch_tier_model`：列表校验、失败回滚、
 *   codex 侧确认弹窗）——成功后该对不符自然消解；
 * - 「保持」= 本会话静默（分叉仍在、不再提示；重启后若仍在会再报）。
 */
export function ModelMismatchBanner() {
  const { t } = useTranslation();
  const [mismatches, setMismatches] = useState<ModelMismatch[]>([]);
  const [busyId, setBusyId] = useState<string | null>(null);

  const refresh = useCallback(() => {
    modelAlignmentApi
      .list()
      .then(setMismatches)
      .catch(() => undefined);
  }, []);

  // 新告警事件只做一件事：拉一次权威活跃集（事件可能在窗口隐藏期间发出）；
  // 挂载时也拉一次补齐窗口未开期间的告警。
  useTauriEvent<ModelMismatch>(MODEL_MISMATCH, refresh);
  useEffect(() => {
    refresh();
  }, [refresh]);

  if (mismatches.length === 0) {
    return null;
  }

  const keepTierModel = async (mismatch: ModelMismatch) => {
    try {
      await modelAlignmentApi.dismiss(mismatch);
    } finally {
      setMismatches((prev) => prev.filter((item) => item !== mismatch));
    }
  };

  const adoptRequestedModel = async (mismatch: ModelMismatch) => {
    const id = `${mismatch.appType}:${mismatch.providerId}:${mismatch.requestedModel}`;
    setBusyId(id);
    try {
      await relayApi.switchTierModel(
        mismatch.providerId,
        mismatch.appType,
        mismatch.requestedModel,
      );
      await modelAlignmentApi.dismiss(mismatch);
      setMismatches((prev) => prev.filter((item) => item !== mismatch));
    } catch (error) {
      toast.error(t("modelMismatch.switchFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="fixed bottom-4 left-1/2 z-40 flex w-[min(640px,calc(100vw-2rem))] -translate-x-1/2 flex-col gap-2">
      {mismatches.map((mismatch) => {
        const id = `${mismatch.appType}:${mismatch.providerId}:${mismatch.requestedModel}`;
        return (
          <div
            key={id}
            className="flex flex-col gap-2 rounded-lg border border-amber-500/60 bg-amber-50/95 px-4 py-3 shadow-lg backdrop-blur dark:border-amber-500/50 dark:bg-amber-950/85"
            role="alert"
          >
            <div className="flex items-start gap-2">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400" />
              <p className="text-sm leading-relaxed text-amber-900 dark:text-amber-100">
                {t("modelMismatch.body", {
                  app: mismatch.appType,
                  tier: mismatch.providerName,
                  requested: mismatch.requestedModel,
                  selected: mismatch.sentModel,
                })}
                {!mismatch.canSwitchToRequested && (
                  <span className="block text-amber-700 dark:text-amber-300">
                    {t("modelMismatch.notAvailable", {
                      requested: mismatch.requestedModel,
                    })}
                  </span>
                )}
              </p>
            </div>
            <div className="flex justify-end gap-2">
              <Button
                variant="ghost"
                onClick={() => void keepTierModel(mismatch)}
              >
                {t("modelMismatch.keep", { selected: mismatch.sentModel })}
              </Button>
              <Button
                variant="outline"
                disabled={!mismatch.canSwitchToRequested || busyId === id}
                onClick={() => void adoptRequestedModel(mismatch)}
              >
                {t("modelMismatch.adopt", {
                  requested: mismatch.requestedModel,
                })}
              </Button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
