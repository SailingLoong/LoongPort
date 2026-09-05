import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleAlert, Copy, Check } from "lucide-react";

import type {
  ProbeDiagnostic,
  VerificationReport,
} from "@/lib/api/modelVerification";
import { modelVerificationApi } from "@/lib/api/modelVerification";

export interface VerificationEvidenceListProps {
  report: VerificationReport;
}

/**
 * 验证依据列表：未通过行带小叹号，点开在列表下方展开该腿留存的原始
 * 请求/响应（诊断边车）——用户知情与 debug 共用这一份。
 */
export function VerificationEvidenceList({
  report,
}: VerificationEvidenceListProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState<string | null>(null);
  const [items, setItems] = useState<ProbeDiagnostic[] | null>(null);
  const [copied, setCopied] = useState(false);

  const toggle = useCallback(
    async (code: string) => {
      if (expanded === code) {
        setExpanded(null);
        return;
      }
      setExpanded(code);
      setItems(null);
      try {
        const loaded = await modelVerificationApi.diagnostics(
          report.target.providerId,
          report.target.appType,
          report.target.model,
        );
        setItems(loaded.filter((item) => item.code === code));
      } catch {
        // 读不到就显示「无留存」，不给用户报错。
        setItems([]);
      }
    },
    [expanded, report.target],
  );

  const copyAll = async () => {
    if (!items?.length) return;
    const text = items
      .map(
        (item) =>
          `# ${item.probe} / ${item.code}\n## request\n${item.request}\n## response\n${item.response}`,
      )
      .join("\n\n");
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <section aria-label={t("loongport.modelVerification.evidence.title")}>
      <p className="text-sm text-muted-foreground">
        {t(
          `loongport.modelVerification.evidence.level.${report.evidenceLevel}`,
        )}
      </p>
      <ul className="mt-3 space-y-2">
        {report.facts.map((fact) => {
          const label = t(
            `loongport.modelVerification.evidence.fact.${fact.code}`,
          );
          return (
            <li
              key={`${fact.code}:${fact.outcome}`}
              className="flex items-center justify-between gap-4 text-sm"
            >
              <span className="flex items-center gap-1.5">
                {label}
                {fact.outcome === "failed" && (
                  <button
                    type="button"
                    className="text-muted-foreground hover:text-foreground"
                    title={t("loongport.modelVerification.evidence.diagnose")}
                    aria-label={`${label}: ${t("loongport.modelVerification.evidence.diagnose")}`}
                    aria-expanded={expanded === fact.code}
                    onClick={() => void toggle(fact.code)}
                  >
                    <CircleAlert className="h-3.5 w-3.5" />
                  </button>
                )}
              </span>
              <span className="text-muted-foreground">
                {t(
                  `loongport.modelVerification.evidence.outcome.${fact.outcome}`,
                )}
              </span>
            </li>
          );
        })}
      </ul>
      {expanded && (
        <div className="mt-3 space-y-2 rounded-md border border-border p-3">
          <div className="flex items-center justify-between">
            <p className="text-xs font-medium">
              {t("loongport.modelVerification.diagnostic.title")}
            </p>
            {items?.length ? (
              <button
                type="button"
                className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
                onClick={() => void copyAll()}
              >
                {copied ? (
                  <Check className="h-3 w-3" />
                ) : (
                  <Copy className="h-3 w-3" />
                )}
                {copied
                  ? t("loongport.modelVerification.diagnostic.copied")
                  : t("loongport.modelVerification.diagnostic.copy")}
              </button>
            ) : null}
          </div>
          {items === null && (
            <p className="text-xs text-muted-foreground">
              {t("loongport.modelVerification.diagnostic.loading")}
            </p>
          )}
          {items?.length === 0 && (
            <p className="text-xs text-muted-foreground">
              {t("loongport.modelVerification.diagnostic.empty")}
            </p>
          )}
          {items?.map((item, index) => (
            <div key={index} className="space-y-1">
              <p className="text-xs text-muted-foreground">
                {t(
                  `loongport.modelVerification.diagnostic.probe.${item.probe}`,
                  {
                    defaultValue: item.probe,
                  },
                )}
              </p>
              <DiagBlock
                label={t("loongport.modelVerification.diagnostic.request")}
              >
                {item.request}
              </DiagBlock>
              <DiagBlock
                label={t("loongport.modelVerification.diagnostic.response")}
              >
                {item.response}
              </DiagBlock>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function DiagBlock({ label, children }: { label: string; children: string }) {
  return (
    <div>
      <p className="text-xs text-muted-foreground">{label}</p>
      <pre className="mt-0.5 max-h-40 overflow-auto rounded-md bg-muted p-2 text-xs leading-relaxed whitespace-pre-wrap break-all">
        {children}
      </pre>
    </div>
  );
}
