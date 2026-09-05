/**
 * 验真判定徽章的两种呈现，验真 UI 的收口处之一：
 *
 * - `TierVerdictChip`：中转站档位行上的文字 chip，结论读自
 *   `TierVerificationProvider` 拉取的 summaries；
 * - `BoardVerdictDot`：省心看板卡片上的状态圆点，结论读自看板 DTO 的
 *   `verificationVerdict` 字段（数据源是后端看板命令，不走 Provider）。
 *
 * 模块下线（`MODEL_VERIFICATION_ENABLED = false`）或没有可展示结论时
 * 一律不渲染。
 */
import { useTranslation } from "react-i18next";
import { CheckCircle2 } from "lucide-react";

import type { VerificationVerdict } from "@/lib/api/modelVerification";
import { cn } from "@/lib/utils";

import { MODEL_VERIFICATION_ENABLED } from "./availability";
import { useTierVerification } from "./TierVerificationProvider";

export function TierVerdictChip({ providerId }: { providerId: string }) {
  const { verdictFor } = useTierVerification();
  const { t } = useTranslation();
  const verdict = verdictFor(providerId);

  // inconclusive（证据不足）不是可展示的结论 —— 宁可空白也不误导。
  if (!MODEL_VERIFICATION_ENABLED || !verdict || verdict === "inconclusive") {
    return null;
  }

  if (verdict === "trusted") {
    return (
      <span
        className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium text-emerald-600 ring-1 ring-inset ring-emerald-500/30 dark:text-emerald-400"
        title={t("loongport.modelVerification.tierVerdict.trustedHint")}
      >
        <CheckCircle2 className="h-2.5 w-2.5" />
        {t("loongport.modelVerification.tierVerdict.trusted")}
      </span>
    );
  }

  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 ring-inset",
        verdict === "anomaly"
          ? "text-red-600 ring-red-500/30 dark:text-red-400"
          : "text-amber-600 ring-amber-500/30 dark:text-amber-400",
      )}
      title={t(`loongport.modelVerification.tierVerdict.${verdict}Hint`)}
    >
      {t(`loongport.modelVerification.tierVerdict.${verdict}`)}
    </span>
  );
}

export function BoardVerdictDot({
  verdict,
}: {
  verdict: VerificationVerdict | string | null | undefined;
}) {
  const { t } = useTranslation();

  if (
    !MODEL_VERIFICATION_ENABLED ||
    (verdict !== "anomaly" && verdict !== "suspicious")
  ) {
    return null;
  }

  const label = t(`loongport.modelVerification.tierVerdict.${verdict}`, {
    defaultValue: verdict === "anomaly" ? "检测到异常" : "需要复核",
  });

  return (
    <span
      title={label}
      className={`h-2 w-2 shrink-0 rounded-full ${
        verdict === "anomaly" ? "bg-red-600" : "bg-amber-500"
      }`}
    >
      <span className="sr-only">{label}</span>
    </span>
  );
}
