import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  feedbackSubmission,
  useFeedbackSubmission,
} from "./feedbackSubmission";

export function FeedbackSubmissionIndicator() {
  const { t } = useTranslation();
  const state = useFeedbackSubmission();
  if (state.phase === "preparing" || state.phase === "submitting")
    return (
      <span
        role="status"
        className="inline-flex items-center gap-1 text-muted-foreground"
      >
        <Loader2 className="h-3 w-3 animate-spin" />
        {t("settings.feedbackSubmitting")}
      </span>
    );
  return state.unread ? (
    <span
      className="h-1.5 w-1.5 rounded-full bg-primary"
      role="status"
      aria-label={t("settings.feedbackBackgroundUnread")}
    />
  ) : null;
}

export function FeedbackSubmissionDetails() {
  const { t } = useTranslation();
  const state = useFeedbackSubmission();
  if (state.phase === "idle") return null;
  const busy = state.phase === "preparing" || state.phase === "submitting";
  const result = state.result?.status === "accepted" ? state.result : null;
  return (
    <section
      className="space-y-2 rounded-lg border border-border/60 p-3 text-xs"
      aria-label={t("settings.feedbackBackgroundDetails")}
    >
      <p role="status">
        {t(
          busy
            ? "settings.feedbackSubmitting"
            : state.phase === "accepted"
              ? "settings.feedbackSuccess"
              : state.phase === "failed"
                ? "settings.feedbackPrepareFailed"
                : "settings.feedbackUploadUnknown",
        )}
      </p>
      {busy && <p>{t("settings.feedbackBackgroundContinue")}</p>}
      {state.phase === "failed" && (
        <Button
          size="sm"
          variant="outline"
          onClick={() => feedbackSubmission.retry()}
        >
          {t("settings.feedbackBackgroundRetry")}
        </Button>
      )}
      {result && result.localSave !== "stored" && (
        <p>
          {t(
            result.localSave === "failed"
              ? "settings.feedbackLocalSaveFailed"
              : "settings.feedbackReceiptUnavailable",
          )}
        </p>
      )}
      {result?.receipt && (
        <p className="break-all select-text">
          {result.receipt.identity.kind === "report"
            ? result.receipt.identity.id
            : t("settings.feedbackIssueNumber", {
                number: result.receipt.identity.number,
              })}
        </p>
      )}
      <details>
        <summary className="cursor-pointer">
          {t("settings.feedbackBackgroundDetails")}
        </summary>
        <p className="whitespace-pre-wrap break-words select-text">
          {state.input?.description}
        </p>
        {state.input?.contact && (
          <p className="break-words select-text">{state.input.contact}</p>
        )}
        <p>{state.input?.relatedApps.join(", ")}</p>
        <div className="flex flex-wrap gap-2">
          {state.input?.screenshots.map((shot, index) => (
            <img
              key={index}
              src={shot.base64}
              alt={shot.name}
              className="h-16 w-16 object-contain"
            />
          ))}
        </div>
        {state.prepared && (
          <>
            <ul>
              {state.prepared.files.map((file, index) => (
                <li key={index} className="break-all">
                  {file.name} · {file.bytes} B · SHA-256 {file.sha256}
                </li>
              ))}
            </ul>
            {state.prepared.applicationConfigs !== null && (
              <pre className="max-h-60 overflow-auto whitespace-pre-wrap break-all">
                {JSON.stringify(state.prepared.applicationConfigs, null, 2)}
              </pre>
            )}
          </>
        )}
      </details>
    </section>
  );
}
