import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ApplicationConfiguration } from "@/lib/api/applicationOverview";
import type {
  ApplicationRoutingChange,
  ApplicationRoutingTier,
} from "@/lib/api/applicationRouting";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

/** Existing application bindings only. Selection is local until the owner reviews it. */
export function ApplicationInlineSelection({
  configurations,
  tiers,
  currentProviderId,
  currentModel,
  disabled,
  onReview,
}: {
  configurations: ApplicationConfiguration[];
  tiers: ApplicationRoutingTier[];
  currentProviderId?: string;
  currentModel?: string | null;
  disabled: boolean;
  onReview: (
    selection: NonNullable<ApplicationRoutingChange["selection"]>,
  ) => void;
}) {
  const { t } = useTranslation();
  const [intent, setIntent] = useState<{
    providerId: string;
    model?: string;
  } | null>(null);
  const providerId = intent?.providerId ?? currentProviderId ?? "";
  const available = configurations.filter((item) => item.canSelect);
  const selected = available.find((item) => item.providerId === providerId);
  const tier = tiers.find((item) => item.providerId === providerId);
  const models = tier?.models.length
    ? tier.models
    : [tier?.effectiveModel].filter((model): model is string => Boolean(model));
  const model = intent ? (intent.model ?? "") : (currentModel ?? "");
  const staleModel = Boolean(model && !models.includes(model));
  const changed =
    providerId !== currentProviderId ||
    (intent !== null && model !== (currentModel ?? ""));
  const canReview = !disabled && Boolean(selected) && !staleModel && changed;
  return (
    <div className="flex flex-wrap items-end gap-3">
      <div className="grid min-w-0 gap-1 text-sm">
        <span>{t("applications.source")}</span>
        <Select
          value={selected ? providerId : ""}
          disabled={disabled}
          onValueChange={(value) => setIntent({ providerId: value })}
        >
          <SelectTrigger aria-label={t("applications.source")}>
            <SelectValue placeholder={t("applications.empty")} />
          </SelectTrigger>
          <SelectContent>
            {available.map((item) => (
              <SelectItem key={item.providerId} value={item.providerId}>
                {[item.name, item.serviceName, item.accountLabel]
                  .filter(Boolean)
                  .join(" · ")}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="grid min-w-0 gap-1 text-sm">
        <span>{t("applications.modelFilter")}</span>
        <Select
          value={model ? `model:${model}` : "default"}
          disabled={disabled || !selected || !models.length}
          onValueChange={(value) =>
            setIntent({
              providerId,
              model: value === "default" ? undefined : value.slice(6),
            })
          }
        >
          <SelectTrigger
            aria-label={`${t("applications.currentConfiguration")} · ${t("applications.modelFilter")}`}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="default">{t("applications.default")}</SelectItem>
            {staleModel && (
              <SelectItem value={`model:${model}`} disabled>
                {model}
              </SelectItem>
            )}
            {models.map((value) => (
              <SelectItem key={value} value={`model:${value}`}>
                {value}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <Button
        variant="outline"
        disabled={!canReview}
        onClick={() => {
          if (canReview) onReview({ providerId, ...(model ? { model } : {}) });
        }}
      >
        {t("applications.reviewChanges")}
      </Button>
    </div>
  );
}
