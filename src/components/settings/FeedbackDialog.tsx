import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ClipboardPaste, Copy, ImagePlus, Loader2, X } from "lucide-react";
import { toast } from "sonner";

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
import { APP_IDS, getAppDisplayName } from "@/config/appConfig";
import { feedbackApi } from "@/lib/api";
import { usageApi } from "@/lib/api/usage";
import type { AppId } from "@/lib/api/types";
import { copyText } from "@/lib/clipboard";
import type {
  FeedbackReceipt,
  FeedbackPreparedPreview,
  FeedbackRelatedApp,
  FeedbackReportIdentity,
} from "@/lib/api/diagnostics";
import { extractErrorMessage } from "@/utils/errorUtils";

/** 与后端闸同值（commands/feedback.rs）：前端预检，后端终检。 */
const MAX_SCREENSHOTS = 6;
const MAX_SCREENSHOT_BYTES = 5 * 1024 * 1024;
const MAX_DESCRIPTION_CHARS = 8_000;
const MAX_CONTACT_CHARS = 200;
const MAX_CONTACT_RAW_UTF16 = 1024;

interface ScreenshotItem {
  id: string;
  name: string;
  dataUrl: string;
}

let clipboardCounter = 0;

/** RGBA 原始字节 → PNG data URL（web 标准 canvas 路径，不为此引图像库）。 */
async function rgbaToPngDataUrl(
  width: number,
  height: number,
  rgbaBase64: string,
): Promise<string> {
  const binary = atob(rgbaBase64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("canvas unavailable");
  context.putImageData(
    new ImageData(new Uint8ClampedArray(bytes.buffer), width, height),
    0,
    0,
  );
  return new Promise<string>((resolve, reject) => {
    canvas.toBlob((blob) => {
      if (!blob) {
        reject(new Error("canvas toBlob failed"));
        return;
      }
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result as string);
      reader.onerror = () => reject(reader.error ?? new Error("read failed"));
      reader.readAsDataURL(blob);
    }, "image/png");
  });
}

function dataUrlBytes(dataUrl: string): number {
  const base64 = dataUrl.slice(dataUrl.indexOf(",") + 1);
  return Math.floor((base64.length * 3) / 4);
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsDataURL(file);
  });
}

// Match the native/server Unicode White_Space contract; ECMAScript trim also removes BOM.
function trimContact(contact: string): string {
  return contact.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
}

function cancelPreparedPreview(previewId: string) {
  // Local owner cancellation is ID-matched and idempotent. A failed cleanup
  // never restores a token to the renderer; the native TTL remains authoritative.
  void feedbackApi.cancelPreview(previewId).catch(() => {});
}

function feedbackFailureNotice(error: unknown, fallback: string): string {
  switch (error) {
    case "feedback_contact_unsupported":
      return "settings.feedbackContactUnsupported";
    case "feedback_extended_fields_unsupported":
      return "settings.feedbackExtendedFieldsUnsupported";
    case "feedback_preview_changed":
      return "settings.feedbackPreviewChanged";
    case "feedback_preview_unavailable":
      return "settings.feedbackPreviewUnavailable";
    default:
      return fallback;
  }
}

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
  const [description, setDescription] = useState("");
  // null = a new untouched form; [] = an explicit empty selection.
  const [relatedApps, setRelatedApps] = useState<FeedbackRelatedApp[] | null>(
    null,
  );
  const selectedApps = relatedApps ?? [];
  const [recommendedApp, setRecommendedApp] = useState<AppId | null>(null);
  const [contact, setContact] = useState("");
  const [appOrder, setAppOrder] = useState<AppId[]>(APP_IDS);
  const [extendedFields, setExtendedFields] = useState(false);
  const [screenshots, setScreenshots] = useState<ScreenshotItem[]>([]);
  const [includeDiagnostics, setIncludeDiagnostics] = useState(true);
  const [includeSites, setIncludeSites] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [preview, setPreview] = useState<FeedbackPreparedPreview | null>(null);
  const previewRef = useRef<FeedbackPreparedPreview | null>(null);
  const previewRegionRef = useRef<HTMLElement>(null);
  const draftVersionRef = useRef(0);
  const preparingRef = useRef(false);
  const contactTooLarge = contact.length > MAX_CONTACT_RAW_UTF16;
  const contactCharacters = contactTooLarge
    ? null
    : Array.from(trimContact(contact)).length;
  const [attachmentAction, setAttachmentAction] = useState<
    "paste" | "files" | null
  >(null);
  const attachmentSessionRef = useRef<number | null>(null);
  const pasting = attachmentAction === "paste";
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [receipt, setReceipt] = useState<FeedbackReceipt | null>(null);
  const [receiptAction, setReceiptAction] = useState<
    "export" | "delete" | null
  >(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [acceptedUnsaved, setAcceptedUnsaved] = useState<{
    receipt: FeedbackReceipt;
    notice: string;
  } | null>(null);
  const sessionRef = useRef(0);
  const receiptReadRef = useRef(0);
  const busyRef = useRef(false);
  const openRef = useRef(open);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const invalidatePreview = useCallback(() => {
    draftVersionRef.current += 1;
    const previous = previewRef.current;
    previewRef.current = null;
    if (mountedRef.current) setPreview(null);
    if (previous) cancelPreparedPreview(previous.previewId);
  }, []);

  const closeDialog = () => {
    invalidatePreview();
    onClose();
  };

  const loadReceipt = useCallback(async () => {
    const session = sessionRef.current;
    const read = ++receiptReadRef.current;
    try {
      const result = await feedbackApi.getLastReport();
      if (
        mountedRef.current &&
        session === sessionRef.current &&
        read === receiptReadRef.current
      ) {
        setReceipt(result);
      }
    } catch {
      if (
        mountedRef.current &&
        session === sessionRef.current &&
        read === receiptReadRef.current
      ) {
        setNotice("settings.feedbackReceiptLoadFailed");
      }
    }
  }, []);

  useEffect(() => {
    openRef.current = open;
    sessionRef.current += 1;
    attachmentSessionRef.current = null;
    setAttachmentAction(null);
    if (open) {
      const session = sessionRef.current;
      const isCurrent = () =>
        mountedRef.current && openRef.current && session === sessionRef.current;
      setExtendedFields(false);
      setAppOrder(APP_IDS);
      setRecommendedApp(null);
      void feedbackApi.getFormCapabilities().then(
        (capabilities) => {
          if (isCurrent())
            setExtendedFields(capabilities.extendedFields === true);
        },
        () => {
          /* Unknown support stays disabled; basic feedback remains available. */
        },
      );
      void usageApi.getUsageSummaryByApp().then(
        (usage) => {
          if (!isCurrent()) return;
          // Existing local request records are an ordering hint, not launch counts.
          // The dashboard folds Desktop into Claude, so neither gets an invented
          // per-app frequency from that combined bucket. No counts are uploaded.
          const counts = new Map(
            usage
              .filter(
                ({ appType, summary }) =>
                  appType !== "claude" &&
                  appType !== "claude-desktop" &&
                  Number.isSafeInteger(summary.totalRequests) &&
                  summary.totalRequests >= 0,
              )
              .map(({ appType, summary }) => [appType, summary.totalRequests]),
          );
          const ordered = [...APP_IDS].sort(
            (a, b) => (counts.get(b) ?? 0) - (counts.get(a) ?? 0),
          );
          setAppOrder(ordered);
          setRecommendedApp(
            (counts.get(ordered[0]) ?? 0) > 0 ? ordered[0] : null,
          );
        },
        () => {
          /* Missing frequency keeps the registry's stable neutral order. */
        },
      );
      setNotice(null);
      setAcceptedUnsaved(null);
      void loadReceipt();
    }
    return () => {
      sessionRef.current += 1;
      invalidatePreview();
    };
  }, [open, loadReceipt, invalidatePreview]);

  useEffect(() => {
    if (open && extendedFields && recommendedApp && relatedApps === null)
      setRelatedApps([recommendedApp]);
  }, [open, extendedFields, recommendedApp, relatedApps]);

  useEffect(() => {
    if (preview) previewRegionRef.current?.focus();
  }, [preview]);

  const isCurrentSession = (session: number) =>
    mountedRef.current && openRef.current && session === sessionRef.current;

  const finishOperation = (session: number) => {
    busyRef.current = false;
    if (!mountedRef.current) return;
    setSubmitting(false);
    setReceiptAction(null);
    // A hidden session may have finished a native mutation. Re-read its owner;
    // never apply that session's draft reset or returned projection to a new one.
    if (openRef.current && session !== sessionRef.current) void loadReceipt();
  };

  const reset = useCallback(() => {
    invalidatePreview();
    setDescription("");
    setRelatedApps(null);
    setContact("");
    setScreenshots([]);
    setIncludeDiagnostics(true);
    setIncludeSites(false);
  }, [invalidatePreview]);

  const addScreenshot = useCallback(
    (name: string, dataUrl: string) => {
      invalidatePreview();
      setScreenshots((current) => {
        if (current.length >= MAX_SCREENSHOTS) {
          toast.error(
            t("settings.feedbackScreenshotTooMany", { count: MAX_SCREENSHOTS }),
          );
          return current;
        }
        if (dataUrlBytes(dataUrl) > MAX_SCREENSHOT_BYTES) {
          toast.error(t("settings.feedbackScreenshotTooLarge"));
          return current;
        }
        return [...current, { id: crypto.randomUUID(), name, dataUrl }];
      });
    },
    [t, invalidatePreview],
  );

  const finishAttachmentRead = (session: number) => {
    if (mountedRef.current && attachmentSessionRef.current === session) {
      attachmentSessionRef.current = null;
      setAttachmentAction(null);
    }
  };

  const identityText = (identity: FeedbackReportIdentity) =>
    identity.kind === "github_issue"
      ? t("settings.feedbackIssueNumber", { number: identity.number })
      : identity.id;

  const handlePasteFromClipboard = async () => {
    if (busyRef.current || attachmentSessionRef.current !== null) return;
    invalidatePreview();
    const session = sessionRef.current;
    attachmentSessionRef.current = session;
    setAttachmentAction("paste");
    try {
      const image = await feedbackApi.readClipboardImage();
      if (!isCurrentSession(session)) return;
      const dataUrl = await rgbaToPngDataUrl(
        image.width,
        image.height,
        image.rgbaBase64,
      );
      if (!isCurrentSession(session)) return;
      clipboardCounter += 1;
      addScreenshot(`clipboard-${clipboardCounter}.png`, dataUrl);
    } catch (error) {
      if (isCurrentSession(session))
        toast.error(extractErrorMessage(error), { closeButton: true });
    } finally {
      finishAttachmentRead(session);
    }
  };

  const handlePickFiles = async (files: FileList | null) => {
    if (!files || busyRef.current || attachmentSessionRef.current !== null)
      return;
    invalidatePreview();
    const session = sessionRef.current;
    attachmentSessionRef.current = session;
    setAttachmentAction("files");
    try {
      for (const file of Array.from(files)) {
        if (!isCurrentSession(session)) return;
        try {
          const dataUrl = await readFileAsDataUrl(file);
          if (!isCurrentSession(session)) return;
          addScreenshot(file.name, dataUrl);
        } catch (error) {
          if (isCurrentSession(session))
            toast.error(extractErrorMessage(error), { closeButton: true });
        }
      }
    } finally {
      finishAttachmentRead(session);
    }
  };

  const handlePrepare = async () => {
    if (
      busyRef.current ||
      preparingRef.current ||
      attachmentSessionRef.current !== null
    )
      return;
    invalidatePreview();
    if (description.trim().length === 0) {
      toast.error(t("settings.feedbackDescriptionRequired"));
      return;
    }
    if (description.length > MAX_DESCRIPTION_CHARS) {
      toast.error(t("settings.feedbackDescriptionTooLong"));
      return;
    }
    // Reject controls in the original input; trimming must not hide them.
    if (contact.length > MAX_CONTACT_RAW_UTF16 || /\p{Cc}/u.test(contact)) {
      setNotice("settings.feedbackContactInvalid");
      return;
    }
    const trimmedContact = trimContact(contact);
    if (Array.from(trimmedContact).length > MAX_CONTACT_CHARS) {
      setNotice("settings.feedbackContactTooLong");
      return;
    }
    if (!extendedFields && (selectedApps.length > 0 || trimmedContact)) {
      setNotice("settings.feedbackExtendedFieldsUnsupported");
      return;
    }
    // Preparing confirms even an empty selection; late frequency cannot add consent.
    setRelatedApps(selectedApps);
    preparingRef.current = true;
    setPreparing(true);
    setNotice(null);
    const session = sessionRef.current;
    const version = draftVersionRef.current;
    try {
      const result = await feedbackApi.prepare({
        description,
        relatedApps: [...APP_IDS, "other" as const].filter((app) =>
          selectedApps.includes(app),
        ),
        ...(trimmedContact ? { contact: trimmedContact } : {}),
        includeDiagnostics,
        includeSites: includeDiagnostics && includeSites,
        screenshots: screenshots.map((shot) => ({
          name: shot.name,
          base64: shot.dataUrl,
        })),
      });
      if (!isCurrentSession(session) || version !== draftVersionRef.current) {
        cancelPreparedPreview(result.previewId);
        return;
      }
      previewRef.current = result;
      setPreview(result);
    } catch (error) {
      if (isCurrentSession(session) && version === draftVersionRef.current)
        setNotice(
          feedbackFailureNotice(error, "settings.feedbackPrepareFailed"),
        );
    } finally {
      preparingRef.current = false;
      if (mountedRef.current) setPreparing(false);
    }
  };

  const handleSubmit = async () => {
    const activePreview = previewRef.current;
    if (
      busyRef.current ||
      preparingRef.current ||
      attachmentSessionRef.current !== null ||
      !activePreview
    )
      return;
    // One attempt consumes the reviewed token, including uncertain upload results.
    previewRef.current = null;
    setPreview(null);
    busyRef.current = true;
    const session = sessionRef.current;
    receiptReadRef.current += 1;
    setSubmitting(true);
    setNotice(null);
    setAcceptedUnsaved(null);
    try {
      const result = await feedbackApi.submit({
        previewId: activePreview.previewId,
      });
      if (!isCurrentSession(session)) return;
      if (result.status !== "accepted") {
        setNotice("settings.feedbackUploadUnknown");
        return;
      }
      // An accepted upload must not be presented as a failed upload merely
      // because its local receipt could not be persisted (or an old server has no ID).
      reset();
      if (result.localSave === "stored" && result.receipt) {
        setReceipt(result.receipt);
        toast.success(t("settings.feedbackSuccess"), { closeButton: true });
      } else {
        const localNotice =
          result.localSave === "failed"
            ? "settings.feedbackLocalSaveFailed"
            : "settings.feedbackReceiptUnavailable";
        if (result.receipt) {
          // This accepted receipt was not persisted. Keep it separate so its
          // identity remains copyable without granting local package actions.
          setAcceptedUnsaved({ receipt: result.receipt, notice: localNotice });
        } else {
          setNotice(localNotice);
        }
      }
    } catch (error) {
      if (isCurrentSession(session))
        setNotice(
          feedbackFailureNotice(error, "settings.feedbackUploadFailed"),
        );
    } finally {
      finishOperation(session);
    }
  };

  const handleReceiptAction = async (action: "export" | "delete") => {
    if (busyRef.current || !receipt) return;
    busyRef.current = true;
    const session = sessionRef.current;
    const expectedReportId = receipt.reportId;
    receiptReadRef.current += 1;
    setReceiptAction(action);
    setNotice(null);
    try {
      if (action === "delete") {
        await feedbackApi.deleteLocalReport(expectedReportId);
        if (isCurrentSession(session)) {
          setReceipt(null);
          toast.success(t("settings.feedbackLocalDeleted"));
        }
      } else {
        const exported =
          await feedbackApi.exportLastDiagnostics(expectedReportId);
        if (exported && isCurrentSession(session))
          toast.success(t("settings.feedbackLocalExported"));
      }
    } catch {
      if (isCurrentSession(session)) {
        setNotice(
          action === "delete"
            ? "settings.feedbackDeleteLocalFailed"
            : "settings.feedbackExportLocalFailed",
        );
      }
    } finally {
      finishOperation(session);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && !busyRef.current) closeDialog();
      }}
    >
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
              onChange={(event) => {
                invalidatePreview();
                setDescription(event.target.value);
              }}
              placeholder={t("settings.feedbackDescriptionPlaceholder")}
              rows={5}
              className="resize-none"
              aria-label={t("settings.feedbackDescriptionLabel")}
            />
          </div>

          <div className="space-y-3 rounded-lg border border-border/60 bg-background/50 px-4 py-3">
            <fieldset className="space-y-2">
              <legend className="text-sm font-medium">
                {t("settings.feedbackRelatedApps")}
              </legend>
              <p className="text-xs text-muted-foreground">
                {t("settings.feedbackAppOrderHint")}
              </p>
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
                        disabled={
                          !extendedFields ||
                          submitting ||
                          receiptAction !== null
                        }
                        onCheckedChange={(checked) => {
                          invalidatePreview();
                          setRelatedApps((current) =>
                            checked === true
                              ? (current ?? []).includes(app)
                                ? current
                                : [...(current ?? []), app]
                              : (current ?? []).filter((id) => id !== app),
                          );
                        }}
                      />
                      {label}
                    </label>
                  );
                })}
              </div>
            </fieldset>
            <div className="space-y-1.5">
              <label htmlFor="feedback-contact" className="text-sm font-medium">
                {t("settings.feedbackContactLabel")}
              </label>
              <Input
                id="feedback-contact"
                value={contact}
                onChange={(event) => {
                  invalidatePreview();
                  setContact(event.target.value);
                }}
                maxLength={MAX_CONTACT_RAW_UTF16}
                disabled={
                  !extendedFields || submitting || receiptAction !== null
                }
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
            {!extendedFields && (
              <p className="text-xs text-muted-foreground">
                {t("settings.feedbackExtendedFieldsUnsupported")}
              </p>
            )}
            {!extendedFields &&
              (selectedApps.length > 0 || contact.length > 0) && (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={submitting || receiptAction !== null}
                  onClick={() => {
                    setRelatedApps([]);
                    setContact("");
                    setNotice(null);
                  }}
                >
                  {t("settings.feedbackClearOptionalFields")}
                </Button>
              )}
          </div>

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
                      onClick={() => {
                        invalidatePreview();
                        setScreenshots((current) =>
                          current.filter((item) => item.id !== shot.id),
                        );
                      }}
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
                onCheckedChange={(checked) => {
                  invalidatePreview();
                  setIncludeDiagnostics(checked === true);
                }}
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
                  onCheckedChange={(checked) => {
                    invalidatePreview();
                    setIncludeSites(checked === true);
                  }}
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
          <Button
            variant="outline"
            onClick={handlePrepare}
            disabled={
              preparing ||
              submitting ||
              receiptAction !== null ||
              attachmentAction !== null
            }
          >
            {preparing && <Loader2 className="h-4 w-4 animate-spin" />}
            {t(
              preparing
                ? "settings.feedbackPreparing"
                : "settings.feedbackPrepare",
            )}
          </Button>
          <Button
            onClick={handleSubmit}
            disabled={
              !preview ||
              preparing ||
              submitting ||
              receiptAction !== null ||
              attachmentAction !== null
            }
          >
            {submitting && <Loader2 className="h-4 w-4 animate-spin" />}
            {submitting
              ? t("settings.feedbackSubmitting")
              : t("settings.feedbackSubmit")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
