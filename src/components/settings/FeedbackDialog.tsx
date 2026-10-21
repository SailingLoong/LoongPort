import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ClipboardPaste, Copy, ImagePlus, Loader2, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { getAppDisplayName } from "@/config/appConfig";
import { copyText } from "@/lib/clipboard";
import type { FeedbackReportIdentity } from "@/lib/api/diagnostics";
import {
  useFeedbackForm,
  MAX_SCREENSHOTS,
  MAX_CONTACT_CHARS,
  MAX_CONTACT_RAW_UTF16,
} from "./useFeedbackForm";

function copyableIdentity(identity: FeedbackReportIdentity): string {
  return identity.kind === "github_issue" ? `#${identity.number}` : identity.id;
}

function ReportIdentityCopyButton({
  identity,
}: {
  identity: FeedbackReportIdentity;
}) {
  const { t } = useTranslation();
  const [copying, setCopying] = useState(false);
  const [status, setStatus] = useState<"copied" | "failed" | null>(null);
  const inFlightRef = useRef(false);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const handleCopy = async () => {
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    setCopying(true);
    setStatus(null);
    try {
      await copyText(copyableIdentity(identity));
      if (mountedRef.current) setStatus("copied");
    } catch {
      if (mountedRef.current) setStatus("failed");
    } finally {
      inFlightRef.current = false;
      if (mountedRef.current) setCopying(false);
    }
  };

  return (
    <div className="space-y-1">
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={handleCopy}
        disabled={copying}
      >
        {copying ? (
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
        ) : (
          <Copy className="h-3.5 w-3.5" />
        )}
        {t("settings.feedbackCopyIdentity")}
      </Button>
      {status && (
        <p
          role={status === "failed" ? "alert" : "status"}
          className="text-xs text-muted-foreground"
        >
          {t(
            status === "failed"
              ? "settings.feedbackIdentityCopyFailed"
              : "settings.feedbackIdentityCopied",
          )}
        </p>
      )}
    </div>
  );
}

/**
 * 问题反馈弹窗：描述 + 截图（剪贴板 / 文件）+ 可选诊断信息，一键回传。
 * 上传前把「会发什么、发去哪」摆在正文里 —— 这就是知情界面本身。
 */
export interface FeedbackDialogProps {
  open: boolean;
  onClose: () => void;
}

export function FeedbackDialog({ open, onClose }: FeedbackDialogProps) {
  const { t } = useTranslation();
  const {
    description,
    selectedApps,
    contact,
    appOrder,
    extendedFields,
    capabilityStatus,
    loadCapabilities,
    screenshots,
    includeDiagnostics,
    includeSites,
    submitting,
    preparing,
    preview,
    previewRegionRef,
    contactTooLarge,
    contactCharacters,
    attachmentAction,
    pasting,
    fileInputRef,
    receipt,
    receiptAction,
    notice,
    acceptedUnsaved,
    closeDialog,
    handlePasteFromClipboard,
    handlePickFiles,
    handleReceiptAction,
    changeDescription,
    changeContact,
    changeRelatedApp,
    clearOptionalFields,
    removeScreenshot,
    changeDiagnostics,
    changeSites,
    changeOpen,
    showOptionalFields,
    hasOptionalDraft,
    submitDisabled,
    handlePrimaryAction,
  } = useFeedbackForm(open, onClose);
  const identityText = (identity: FeedbackReportIdentity) =>
    identity.kind === "github_issue"
      ? t("settings.feedbackIssueNumber", { number: identity.number })
      : identity.id;

  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{t("settings.feedbackTitle")}</DialogTitle>
          <DialogDescription>
            {t("settings.feedbackPrivacyNote")}
          </DialogDescription>
        </DialogHeader>

        <div className="flex min-h-0 flex-col gap-3 overflow-y-auto px-6 py-4">
          <div className="flex flex-col gap-1.5">
            <Textarea
              disabled={submitting}
              value={description}
              onChange={(event) => changeDescription(event.target.value)}
              placeholder={t("settings.feedbackDescriptionPlaceholder")}
              rows={5}
              className="resize-none"
              aria-label={t("settings.feedbackDescriptionLabel")}
            />
          </div>

          {showOptionalFields && (
            <div className="space-y-3 rounded-lg border border-border/60 bg-background/50 px-4 py-3">
              <fieldset className="space-y-2">
                <legend className="text-sm font-medium">
                  {t("settings.feedbackRelatedApps")}
                </legend>
                <div className="grid grid-cols-2 gap-2">
                  {[...appOrder, "other" as const].map((app) => {
                    const label =
                      app === "other"
                        ? t("settings.feedbackOtherApp")
                        : getAppDisplayName(app, t);
                    return (
                      <label
                        key={app}
                        className="flex items-center gap-2 text-sm"
                      >
                        <Checkbox
                          aria-label={label}
                          checked={selectedApps.includes(app)}
                          disabled={submitting || receiptAction !== null}
                          onCheckedChange={(checked) =>
                            changeRelatedApp(app, checked === true)
                          }
                        />
                        {label}
                      </label>
                    );
                  })}
                </div>
              </fieldset>
              <div className="space-y-1.5">
                <label
                  htmlFor="feedback-contact"
                  className="text-sm font-medium"
                >
                  {t("settings.feedbackContactLabel")}
                </label>
                <Input
                  id="feedback-contact"
                  value={contact}
                  onChange={(event) => changeContact(event.target.value)}
                  maxLength={MAX_CONTACT_RAW_UTF16}
                  disabled={submitting || receiptAction !== null}
                  aria-describedby="feedback-contact-hint feedback-contact-length"
                  aria-invalid={
                    contactTooLarge ||
                    /\p{Cc}/u.test(contact) ||
                    (contactCharacters ?? 0) > MAX_CONTACT_CHARS
                  }
                />
                <p
                  id="feedback-contact-hint"
                  className="text-xs text-muted-foreground"
                >
                  {t("settings.feedbackContactHint")}
                </p>
                <p
                  id="feedback-contact-length"
                  className="text-xs text-muted-foreground"
                >
                  {contactTooLarge
                    ? t("settings.feedbackContactInvalid")
                    : t("settings.feedbackContactLength", {
                        count: contactCharacters,
                        max: MAX_CONTACT_CHARS,
                      })}
                </p>
              </div>
              {!extendedFields && hasOptionalDraft && (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={submitting || receiptAction !== null}
                  onClick={clearOptionalFields}
                >
                  {t("settings.feedbackClearOptionalFields")}
                </Button>
              )}
            </div>
          )}
          {!extendedFields && (
            <div className="space-y-2">
              <p className="text-xs text-muted-foreground" role="status">
                {t(
                  capabilityStatus === "loading"
                    ? "settings.feedbackCapabilitiesLoading"
                    : capabilityStatus === "failed"
                      ? "settings.feedbackCapabilitiesFailed"
                      : "settings.feedbackExtendedFieldsUnsupported",
                )}
              </p>
              {capabilityStatus === "failed" && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => void loadCapabilities()}
                >
                  {t("settings.feedbackCapabilitiesRetry")}
                </Button>
              )}
            </div>
          )}

          <p className="text-xs text-muted-foreground">
            {t("settings.feedbackUserContentNotice")}
          </p>
          <div className="flex flex-col gap-2">
            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7 gap-1.5 text-xs"
                onClick={handlePasteFromClipboard}
                disabled={
                  attachmentAction !== null ||
                  submitting ||
                  receiptAction !== null ||
                  screenshots.length >= MAX_SCREENSHOTS
                }
              >
                {pasting ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <ClipboardPaste className="h-3.5 w-3.5" />
                )}
                {t("settings.feedbackPasteScreenshot")}
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7 gap-1.5 text-xs"
                onClick={() => fileInputRef.current?.click()}
                disabled={
                  attachmentAction !== null ||
                  submitting ||
                  receiptAction !== null ||
                  screenshots.length >= MAX_SCREENSHOTS
                }
              >
                <ImagePlus className="h-3.5 w-3.5" />
                {t("settings.feedbackAddScreenshotFile")}
              </Button>
              <span className="ml-auto text-xs text-muted-foreground">
                {t("settings.feedbackScreenshotCount", {
                  count: screenshots.length,
                  max: MAX_SCREENSHOTS,
                })}
              </span>
            </div>
            <input
              ref={fileInputRef}
              type="file"
              disabled={
                attachmentAction !== null ||
                submitting ||
                receiptAction !== null
              }
              accept="image/png,image/jpeg,image/webp,image/gif,image/bmp"
              multiple
              className="hidden"
              onChange={(event) => {
                void handlePickFiles(event.target.files);
                event.target.value = "";
              }}
            />
            {screenshots.length > 0 && (
              <div className="flex flex-wrap gap-2">
                {screenshots.map((shot) => (
                  <div
                    key={shot.id}
                    className="group relative h-16 w-16 overflow-hidden rounded-md border border-border/60 bg-background/50"
                  >
                    <img
                      src={shot.dataUrl}
                      alt={shot.name}
                      className="h-full w-full object-cover"
                    />
                    <button
                      type="button"
                      className="absolute right-0.5 top-0.5 rounded bg-background/80 p-0.5 opacity-0 transition-opacity group-hover:opacity-100 focus:opacity-100"
                      onClick={() => removeScreenshot(shot.id)}
                      aria-label={t("settings.feedbackRemoveScreenshot")}
                      disabled={submitting}
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>

          <div className="flex flex-col gap-2 rounded-lg border border-border/60 bg-background/50 px-4 py-3">
            <label className="flex cursor-pointer items-start gap-2">
              <Checkbox
                disabled={submitting}
                checked={includeDiagnostics}
                onCheckedChange={(checked) =>
                  changeDiagnostics(checked === true)
                }
                className="mt-0.5"
              />
              <span className="text-sm leading-none">
                {t("settings.feedbackIncludeDiagnostics")}
                <span className="mt-1 block text-xs leading-normal text-muted-foreground">
                  {t("settings.feedbackDiagnosticsDetails")}
                </span>
              </span>
            </label>
            {includeDiagnostics && (
              <label className="ml-6 flex cursor-pointer items-start gap-2">
                <Checkbox
                  disabled={submitting}
                  checked={includeSites}
                  onCheckedChange={(checked) => changeSites(checked === true)}
                  className="mt-0.5"
                />
                <span className="min-w-0">
                  <span className="block text-sm leading-none">
                    {t("settings.feedbackIncludeSites")}
                  </span>
                  <span className="mt-1 block text-xs text-muted-foreground">
                    {t("settings.exportDiagnosticsIncludeSitesHint")}
                  </span>
                </span>
              </label>
            )}
          </div>
          {preview && (
            <section
              ref={previewRegionRef}
              tabIndex={-1}
              aria-label={t("settings.feedbackPreviewTitle")}
              className="space-y-2 rounded-lg border border-border/60 bg-background/50 px-4 py-3"
            >
              <h3 className="text-sm font-medium">
                {t("settings.feedbackPreviewTitle")}
              </h3>
              <p className="text-xs text-muted-foreground">
                {t("settings.feedbackPreviewHint")}
              </p>
              {preview.summary.contactIncluded && (
                <p className="text-xs text-muted-foreground">
                  {t("settings.feedbackPreviewContactNotice")}
                </p>
              )}
              {preview.files.length === 0 ? (
                <p className="text-xs">
                  {t("settings.feedbackPreviewNoFiles")}
                </p>
              ) : (
                <ul className="space-y-2 text-xs">
                  {preview.files.map((file, index) => (
                    <li key={`${file.name}-${index}`} className="space-y-1">
                      <p className="break-all font-medium">{file.name}</p>
                      <p>
                        {t("settings.feedbackPreviewFileBytes", {
                          bytes: file.bytes,
                        })}
                      </p>
                      <p className="break-all select-text">
                        <span>SHA-256: </span>
                        <span>{file.sha256}</span>
                      </p>
                    </li>
                  ))}
                </ul>
              )}
              <p className="text-xs">
                {t("settings.feedbackPreviewTotalBytes", {
                  bytes: preview.totalBytes,
                })}
              </p>
              <p className="text-xs text-muted-foreground">
                {t("settings.feedbackPreviewExpires", {
                  seconds: preview.expiresInSeconds,
                })}
              </p>
              {preview.applicationConfigs !== null && (
                <div className="space-y-1">
                  <h4 className="text-xs font-medium">
                    {t("settings.feedbackPreviewApplicationConfigs")}
                  </h4>
                  <pre className="max-h-60 overflow-auto whitespace-pre-wrap break-all rounded bg-background/50 p-2 text-xs select-text">
                    {JSON.stringify(preview.applicationConfigs, null, 2)}
                  </pre>
                </div>
              )}
            </section>
          )}
          {notice && (
            <p
              role="alert"
              className="rounded-lg border border-border/60 px-4 py-3 text-sm"
            >
              {t(notice)}
            </p>
          )}

          {acceptedUnsaved && (
            <section
              aria-label={t("settings.feedbackUnsavedUpload")}
              className="flex flex-col gap-2 rounded-lg border border-border/60 bg-background/50 px-4 py-3"
            >
              <h3 className="text-sm font-medium">
                {t("settings.feedbackUnsavedUpload")}
              </h3>
              <p role="alert" className="text-sm">
                {t(acceptedUnsaved.notice)}
              </p>
              <dl className="space-y-1 text-xs">
                <div>
                  <dt className="text-muted-foreground">
                    {t(
                      acceptedUnsaved.receipt.identity.kind === "github_issue"
                        ? "settings.feedbackIssueLabel"
                        : "settings.feedbackReportId",
                    )}
                  </dt>
                  <dd className="break-all select-text">
                    {identityText(acceptedUnsaved.receipt.identity)}
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">
                    {t("settings.feedbackUploadedAt")}
                  </dt>
                  <dd>
                    <time dateTime={acceptedUnsaved.receipt.uploadedAt}>
                      {acceptedUnsaved.receipt.uploadedAt}
                    </time>
                  </dd>
                </div>
              </dl>
              <ReportIdentityCopyButton
                key={copyableIdentity(acceptedUnsaved.receipt.identity)}
                identity={acceptedUnsaved.receipt.identity}
              />
            </section>
          )}

          {receipt && (
            <section
              aria-label={t("settings.feedbackLastReport")}
              className="flex flex-col gap-2 rounded-lg border border-border/60 bg-background/50 px-4 py-3"
            >
              <h3 className="text-sm font-medium">
                {t("settings.feedbackLastReport")}
              </h3>
              <dl className="space-y-1 text-xs">
                <div>
                  <dt className="text-muted-foreground">
                    {t(
                      receipt.identity.kind === "github_issue"
                        ? "settings.feedbackIssueLabel"
                        : "settings.feedbackReportId",
                    )}
                  </dt>
                  <dd className="break-all select-text">
                    {identityText(receipt.identity)}
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">
                    {t("settings.feedbackUploadedAt")}
                  </dt>
                  <dd>
                    <time dateTime={receipt.uploadedAt}>
                      {receipt.uploadedAt}
                    </time>
                  </dd>
                </div>
                {receipt.localCopy.status !== "notAttached" && (
                  <div>
                    <dt className="text-muted-foreground">
                      {t("settings.feedbackLocalPackage")}
                    </dt>
                    <dd>{receipt.localCopy.fileName}</dd>
                  </div>
                )}
              </dl>
              <ReportIdentityCopyButton
                key={copyableIdentity(receipt.identity)}
                identity={receipt.identity}
              />
              {receipt.localCopy.status === "missing" && (
                <p className="text-xs text-muted-foreground">
                  {t("settings.feedbackLocalMissing")}
                </p>
              )}
              {receipt.localCopy.status === "notAttached" && (
                <p className="text-xs text-muted-foreground">
                  {t("settings.feedbackDiagnosticsNotAttached")}
                </p>
              )}
              <Collapsible>
                <CollapsibleTrigger asChild>
                  <Button type="button" size="sm" variant="outline">
                    {t("settings.feedbackViewSummary")}
                  </Button>
                </CollapsibleTrigger>
                <CollapsibleContent className="mt-2 space-y-1 text-xs text-muted-foreground">
                  <p>
                    {t(
                      receipt.summary.descriptionIncluded
                        ? "settings.feedbackSummaryDescription"
                        : "settings.feedbackSummaryNoDescription",
                    )}
                  </p>
                  <p>
                    {t("settings.feedbackSummaryScreenshots", {
                      count: receipt.summary.screenshotCount,
                      bytes: receipt.summary.screenshotBytes,
                    })}
                  </p>
                  <p>
                    {t(
                      receipt.summary.diagnosticsIncluded
                        ? "settings.feedbackSummaryDiagnostics"
                        : "settings.feedbackDiagnosticsNotAttached",
                      { bytes: receipt.summary.diagnosticsBytes },
                    )}
                  </p>
                  <p>
                    {t(
                      receipt.summary.contactIncluded
                        ? "settings.feedbackSummaryContact"
                        : "settings.feedbackSummaryNoContact",
                    )}
                  </p>
                  <p>{t("settings.feedbackSummaryScope")}</p>
                </CollapsibleContent>
              </Collapsible>
              <p className="text-xs text-muted-foreground">
                {t("settings.feedbackDeleteLocalOnlyHint")}
              </p>
              <div className="flex flex-wrap gap-2">
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => void handleReceiptAction("export")}
                  disabled={
                    submitting ||
                    preparing ||
                    receiptAction !== null ||
                    receipt.localCopy.status !== "available"
                  }
                >
                  {receiptAction === "export" && (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  )}
                  {t("settings.feedbackExportLocalCopy")}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => void handleReceiptAction("delete")}
                  disabled={submitting || preparing || receiptAction !== null}
                >
                  {receiptAction === "delete" && (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  )}
                  {t("settings.feedbackDeleteLocalCopy")}
                </Button>
              </div>
            </section>
          )}
        </div>

        <DialogFooter className="gap-2">
          <Button
            variant="ghost"
            onClick={closeDialog}
            disabled={submitting || receiptAction !== null}
          >
            {t("common.cancel")}
          </Button>
          <Button onClick={handlePrimaryAction} disabled={submitDisabled}>
            {(preparing || submitting) && (
              <Loader2 className="h-4 w-4 animate-spin" />
            )}
            {t(
              submitting
                ? "settings.feedbackSubmitting"
                : preparing
                  ? "settings.feedbackPreparing"
                  : preview
                    ? "settings.feedbackConfirmSubmit"
                    : "settings.feedbackSubmit",
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
