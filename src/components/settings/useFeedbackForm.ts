import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { APP_IDS } from "@/config/appConfig";
import { feedbackApi } from "@/lib/api";
import { usageApi } from "@/lib/api/usage";
import type { AppId } from "@/lib/api/types";
import type {
  FeedbackReceipt,
  FeedbackPreparedPreview,
  FeedbackRelatedApp,
} from "@/lib/api/diagnostics";
import { extractErrorMessage } from "@/utils/errorUtils";

/** 与后端闸同值（commands/feedback.rs）：前端预检，后端终检。 */
export const MAX_SCREENSHOTS = 6;
const MAX_SCREENSHOT_BYTES = 5 * 1024 * 1024;
const MAX_DESCRIPTION_CHARS = 8_000;
export const MAX_CONTACT_CHARS = 200;
export const MAX_CONTACT_RAW_UTF16 = 1024;

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

/** Single owner for the feedback draft, frozen preview and native operation lifecycle. */
export function useFeedbackForm(open: boolean, onClose: () => void) {
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
  const [capabilityStatus, setCapabilityStatus] = useState<
    "loading" | "ready" | "failed"
  >("loading");
  const capabilitySessionRef = useRef<number | null>(null);
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

  const loadCapabilities = useCallback(async () => {
    const session = sessionRef.current;
    if (!openRef.current || capabilitySessionRef.current === session) return;
    capabilitySessionRef.current = session;
    setCapabilityStatus("loading");
    const isCurrent = () =>
      mountedRef.current && openRef.current && session === sessionRef.current;
    try {
      const capabilities = await feedbackApi.getFormCapabilities();
      if (isCurrent()) {
        setExtendedFields(capabilities.extendedFields === true);
        setCapabilityStatus("ready");
      }
    } catch {
      if (isCurrent()) {
        setExtendedFields(false);
        setCapabilityStatus("failed");
      }
    } finally {
      if (capabilitySessionRef.current === session)
        capabilitySessionRef.current = null;
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
      void loadCapabilities();
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
  }, [open, loadReceipt, loadCapabilities, invalidatePreview]);

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

  const changeDescription = (value: string) => {
    invalidatePreview();
    setDescription(value);
  };
  const changeContact = (value: string) => {
    invalidatePreview();
    setContact(value);
  };
  const changeRelatedApp = (app: FeedbackRelatedApp, checked: boolean) => {
    invalidatePreview();
    setRelatedApps((current) =>
      checked
        ? (current ?? []).includes(app)
          ? current
          : [...(current ?? []), app]
        : (current ?? []).filter((id) => id !== app),
    );
  };
  const clearOptionalFields = () => {
    invalidatePreview();
    setRelatedApps([]);
    setContact("");
    setNotice(null);
  };
  const removeScreenshot = (id: string) => {
    invalidatePreview();
    setScreenshots((current) => current.filter((item) => item.id !== id));
  };
  const changeDiagnostics = (checked: boolean) => {
    invalidatePreview();
    setIncludeDiagnostics(checked);
  };
  const changeSites = (checked: boolean) => {
    invalidatePreview();
    setIncludeSites(checked);
  };
  const changeOpen = (next: boolean) => {
    if (!next && !busyRef.current) closeDialog();
  };
  const hasOptionalDraft = selectedApps.length > 0 || contact.length > 0;
  const showOptionalFields = extendedFields || hasOptionalDraft;
  const submitDisabled =
    preparing ||
    submitting ||
    receiptAction !== null ||
    attachmentAction !== null ||
    description.trim().length === 0;
  // The first action freezes the preview only. A second explicit action uploads that token.
  const handlePrimaryAction = () => {
    if (!openRef.current) return;
    // Follow the stage the user actually saw, never upgrade a stale Submit handler.
    if (!preview) {
      if (previewRef.current) return;
      return handlePrepare();
    }
    // An older confirmation must never consume a newer draft's token.
    if (preview.previewId !== previewRef.current?.previewId) return;
    return handleSubmit();
  };
  return {
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
  };
}
