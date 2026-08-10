import { useTranslation } from "react-i18next";

import type {
  VerificationHistoryEntry,
  VerificationVerdict,
} from "@/lib/api/modelVerification";
import { cn } from "@/lib/utils";

import { VerificationEvidenceList } from "./VerificationEvidenceList";

const verdictClass: Record<VerificationVerdict, string> = {
  trusted: "text-emerald-600 dark:text-emerald-400",
  suspicious: "text-amber-600 dark:text-amber-400",
  anomaly: "text-destructive",
  inconclusive: "text-muted-foreground",
};

export function VerificationHistoryList({
  entries,
}: {
  entries: VerificationHistoryEntry[];
}) {
  const { t } = useTranslation();

  return (
    <div className="divide-y divide-border-default">
      {entries.map((entry, index) => {
        const checkedAt = new Date(entry.report.checkedAt * 1000);
        return (
          <article
            className="space-y-2 py-3 first:pt-0 last:pb-0"
            key={`${entry.source}-${entry.report.target.model}-${entry.report.checkedAt}-${index}`}
          >
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0 space-y-0.5">
                <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
                  <span className="font-medium">
                    {t(
                      `loongport.modelVerification.history.source.${entry.source}`,
                    )}
                  </span>
                  <span className="break-all text-muted-foreground">
                    {entry.report.target.model}
                  </span>
                </p>
                <time
                  className="block text-xs text-muted-foreground"
                  dateTime={checkedAt.toISOString()}
                >
                  {checkedAt.toLocaleString()}
                </time>
              </div>
              <span
                className={cn(
                  "shrink-0 text-xs font-medium",
                  verdictClass[entry.report.verdict],
                )}
              >
                {t(
                  `loongport.modelVerification.verdict.${entry.report.verdict}`,
                )}
              </span>
            </div>
            <VerificationEvidenceList report={entry.report} />
          </article>
        );
      })}
    </div>
  );
}
