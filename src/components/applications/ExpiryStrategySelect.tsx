import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { autoModeApi } from "@/lib/api/autoMode";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

/** One existing global setting; never mutates application fallback or order. */
export function ExpiryStrategySelect({
  appId,
  disabled,
}: {
  appId: string;
  disabled: boolean;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const status = useQuery({
    queryKey: ["autoModeStatus", appId],
    queryFn: () => autoModeApi.getStatus(appId),
    retry: false,
  });
  const [saving, setSaving] = useState(false);
  const [saveFailed, setSaveFailed] = useState(false);
  const known = status.data && !status.error;
  const value = known
    ? status.data.strategy === "known_expiry_first"
      ? "known_expiry_first"
      : "cheapest"
    : "";
  async function save(next: string) {
    if (
      saving ||
      disabled ||
      !known ||
      (next !== "cheapest" && next !== "known_expiry_first")
    )
      return;
    setSaving(true);
    setSaveFailed(false);
    try {
      await autoModeApi.setStrategy(next);
    } catch {
      setSaveFailed(true);
    } finally {
      // Re-read even after a lost reply. Do not advertise an optimistic value.
      // All app snapshots contain this same global preference.
      await client.invalidateQueries({ queryKey: ["autoModeStatus"] });
      setSaving(false);
    }
  }
  return (
    <div className="space-y-1">
      <div
        className="inline-flex items-center gap-2 text-sm"
        title={t("applications.expiryStrategyHint")}
      >
        <span>{t("applications.expiryStrategyLabel")}</span>
        <Select
          value={value}
          onValueChange={(next) => {
            void save(next);
          }}
          disabled={disabled || saving || status.isFetching || !known}
        >
          <SelectTrigger
            className="h-8 w-auto min-w-36"
            aria-label={t("applications.expiryStrategyLabel")}
          >
            <SelectValue
              placeholder={t(
                status.error ? "common.unknown" : "common.loading",
              )}
            />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="cheapest">
              {t("applications.expiryStrategyOrder")}
            </SelectItem>
            <SelectItem value="known_expiry_first">
              {t("applications.expiryStrategyKnown")}
            </SelectItem>
          </SelectContent>
        </Select>
      </div>
      {status.error ? (
        <div role="alert" className="text-xs text-destructive">
          {t("applications.expiryStrategyReadFailed")}
          <Button
            variant="link"
            size="sm"
            disabled={status.isFetching}
            onClick={() => {
              void status.refetch();
            }}
          >
            {t("common.refresh")}
          </Button>
        </div>
      ) : saveFailed ? (
        <p role="alert" className="text-xs text-destructive">
          {t("applications.expiryStrategySaveFailed")}
        </p>
      ) : null}
    </div>
  );
}
