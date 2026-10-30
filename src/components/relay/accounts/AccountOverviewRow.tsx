import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import type { AccountRowVM } from "./accountOverviewProjection";
/** Render only projected fields; no snapshot/platform branches or account commands. */
export function AccountOverviewRow({
  row,
  disabled,
  onOpenDetails,
}: {
  row: AccountRowVM;
  disabled?: boolean;
  onOpenDetails: (rowKey: string) => void;
}) {
  const { t } = useTranslation();
  const reasonId = useId();
  return (
    <article
      aria-label={row.name}
      className="grid min-w-0 gap-3 border-b border-border px-3 py-3 last:border-b-0 lg:grid-cols-[minmax(9rem,1fr)_minmax(14rem,2fr)_minmax(14rem,2fr)_auto] lg:items-center"
    >
      <div className="min-w-0">
        <p className="break-words text-sm font-semibold">{row.name}</p>
        {row.identity && (
          <p className="text-xs text-muted-foreground">{row.identity}</p>
        )}
      </div>
      <dl className="grid min-w-0 grid-cols-2 gap-x-3 gap-y-1 text-xs">
        {row.facts.map((fact) => (
          <div key={fact.label}>
            <dt className="text-muted-foreground">{fact.label}</dt>
            <dd className="break-words">{fact.value}</dd>
          </div>
        ))}
      </dl>
      <dl className="grid min-w-0 gap-1 text-xs">
        {[
          [
            t("accountRewardGlance.lastSuccess", { defaultValue: "最近成功" }),
            row.lastSuccess,
          ],
          [
            t("accountRewardGlance.nextExpiry", { defaultValue: "即将到期" }),
            row.nextExpiry,
          ],
        ].map(([label, value]) => (
          <div key={label}>
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="break-words">{value}</dd>
          </div>
        ))}
        {row.claimIssue && (
          <div>
            <dd className="break-words text-destructive">{row.claimIssue}</dd>
          </div>
        )}
      </dl>
      <div className="flex flex-wrap items-center gap-2 lg:max-w-48">
        <Button size="sm" disabled aria-describedby={reasonId}>
          {row.switchAction.label}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={disabled}
          aria-label={t("accountsOverview.viewAccount", {
            name: row.name,
            defaultValue: "查看 {{name}}",
          })}
          onClick={() => onOpenDetails(row.rowKey)}
        >
          {t("accountsOverview.details")}
        </Button>
        <p id={reasonId} className="w-full text-xs text-muted-foreground">
          {row.switchAction.reason}
        </p>
      </div>
    </article>
  );
}
