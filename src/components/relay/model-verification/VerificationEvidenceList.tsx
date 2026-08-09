import { useTranslation } from "react-i18next";

import type { VerificationReport } from "@/lib/api/modelVerification";

export interface VerificationEvidenceListProps {
  report: VerificationReport;
}

export function VerificationEvidenceList({
  report,
}: VerificationEvidenceListProps) {
  const { t } = useTranslation();

  return (
    <section aria-label={t("loongport.modelVerification.evidence.title")}>
      <p className="text-sm text-muted-foreground">
        {t(
          `loongport.modelVerification.evidence.level.${report.evidenceLevel}`,
        )}
      </p>
      <ul className="mt-3 space-y-2">
        {report.facts.map((fact) => (
          <li
            key={`${fact.code}:${fact.outcome}`}
            className="flex items-center justify-between gap-4 text-sm"
          >
            <span>
              {t(`loongport.modelVerification.evidence.fact.${fact.code}`)}
            </span>
            <span className="text-muted-foreground">
              {t(
                `loongport.modelVerification.evidence.outcome.${fact.outcome}`,
              )}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}
