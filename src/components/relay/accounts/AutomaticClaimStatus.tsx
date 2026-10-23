import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { AutomaticClaimSnapshot } from "@/lib/api/automaticClaim";

/** Shared presentation of backend scheduler facts, separate from account receipts. */
export function AutomaticClaimStatus({
  snapshot,
  failed,
  reload,
}: {
  snapshot: AutomaticClaimSnapshot | null;
  failed: boolean;
  reload: () => void | Promise<void>;
}) {
  const { t, i18n } = useTranslation();
  const date = (value: number) =>
    new Date(value).toLocaleString(i18n?.language);
  return (
    <section
      aria-label={t("automaticStatus.title")}
      className="space-y-1 rounded-md border border-border p-3 text-xs text-muted-foreground"
    >
      <p className="font-medium">{t("automaticStatus.title")}</p>
      <p role="status" className="flex items-center gap-2">
        {snapshot?.phase === "checking" && (
          <Loader2 aria-hidden="true" className="h-3.5 w-3.5 animate-spin" />
        )}
        {t(
          failed
            ? "automaticStatus.unavailable"
            : snapshot
              ? `automaticStatus.${snapshot.phase}`
              : "automaticStatus.loading",
        )}
      </p>
      {failed && (
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={() => void reload()}
        >
          {t("automaticStatus.reload")}
        </Button>
      )}
      {snapshot?.lastCheckedAt != null && (
        <p>
          {t("automaticStatus.lastCheckedAt", {
            time: date(snapshot.lastCheckedAt),
          })}
        </p>
      )}
      {snapshot?.result && (
        <p>{t(`automaticStatus.result.${snapshot.result}`)}</p>
      )}
      {snapshot?.reason && (
        <p>{t(`automaticStatus.reason.${snapshot.reason}`)}</p>
      )}
      {snapshot?.nextCheckAt != null && (
        <p>
          {t("automaticStatus.nextCheckAt", {
            time: date(snapshot.nextCheckAt),
          })}
        </p>
      )}
      <p>{t("automaticStatus.receiptNotice")}</p>
    </section>
  );
}
