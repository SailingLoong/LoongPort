import { useCallback, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ClipboardPaste, ImagePlus, Loader2, X } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/textarea";
import { feedbackApi } from "@/lib/api";
import { extractErrorMessage } from "@/utils/errorUtils";

/** 与后端闸同值（commands/feedback.rs）：前端预检，后端终检。 */
const MAX_SCREENSHOTS = 6;
const MAX_SCREENSHOT_BYTES = 5 * 1024 * 1024;
const MAX_DESCRIPTION_CHARS = 8_000;

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
  const [screenshots, setScreenshots] = useState<ScreenshotItem[]>([]);
  const [includeDiagnostics, setIncludeDiagnostics] = useState(true);
  const [includeSites, setIncludeSites] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [pasting, setPasting] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const reset = useCallback(() => {
    setDescription("");
    setScreenshots([]);
    setIncludeDiagnostics(true);
    setIncludeSites(false);
  }, []);

  const addScreenshot = useCallback(
    (name: string, dataUrl: string) => {
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
    [t],
  );

  const handlePasteFromClipboard = async () => {
    setPasting(true);
    try {
      const image = await feedbackApi.readClipboardImage();
      const dataUrl = await rgbaToPngDataUrl(
        image.width,
        image.height,
        image.rgbaBase64,
      );
      clipboardCounter += 1;
      addScreenshot(`clipboard-${clipboardCounter}.png`, dataUrl);
    } catch (error) {
      toast.error(extractErrorMessage(error), { closeButton: true });
    } finally {
      setPasting(false);
    }
  };

  const handlePickFiles = async (files: FileList | null) => {
    if (!files) return;
    for (const file of Array.from(files)) {
      try {
        const dataUrl = await readFileAsDataUrl(file);
        addScreenshot(file.name, dataUrl);
      } catch (error) {
        toast.error(extractErrorMessage(error), { closeButton: true });
      }
    }
  };

  const handleSubmit = async () => {
    if (description.trim().length === 0) {
      toast.error(t("settings.feedbackDescriptionRequired"));
      return;
    }
    if (description.length > MAX_DESCRIPTION_CHARS) {
      toast.error(t("settings.feedbackDescriptionTooLong"));
      return;
    }
    setSubmitting(true);
    try {
      await feedbackApi.submit({
        description,
        includeDiagnostics,
        includeSites: includeDiagnostics && includeSites,
        screenshots: screenshots.map((shot) => ({
          name: shot.name,
          base64: shot.dataUrl,
        })),
      });
      toast.success(t("settings.feedbackSuccess"), { closeButton: true });
      reset();
      onClose();
    } catch (error) {
      toast.error(extractErrorMessage(error), { closeButton: true });
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && !submitting) onClose();
      }}
    >
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{t("settings.feedbackTitle")}</DialogTitle>
          <DialogDescription>
            {t("settings.feedbackPrivacyNote")}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <Textarea
              value={description}
              onChange={(event) => setDescription(event.target.value)}
              placeholder={t("settings.feedbackDescriptionPlaceholder")}
              rows={5}
              className="resize-none"
              aria-label={t("settings.feedbackDescriptionLabel")}
            />
          </div>

          <div className="flex flex-col gap-2">
            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7 gap-1.5 text-xs"
                onClick={handlePasteFromClipboard}
                disabled={
                  pasting || submitting || screenshots.length >= MAX_SCREENSHOTS
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
                disabled={submitting || screenshots.length >= MAX_SCREENSHOTS}
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
                      onClick={() =>
                        setScreenshots((current) =>
                          current.filter((item) => item.id !== shot.id),
                        )
                      }
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
                checked={includeDiagnostics}
                onCheckedChange={(checked) =>
                  setIncludeDiagnostics(checked === true)
                }
                className="mt-0.5"
              />
              <span className="text-sm leading-none">
                {t("settings.feedbackIncludeDiagnostics")}
              </span>
            </label>
            {includeDiagnostics && (
              <label className="ml-6 flex cursor-pointer items-start gap-2">
                <Checkbox
                  checked={includeSites}
                  onCheckedChange={(checked) =>
                    setIncludeSites(checked === true)
                  }
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
        </div>

        <DialogFooter className="gap-2">
          <Button variant="ghost" onClick={onClose} disabled={submitting}>
            {t("common.cancel")}
          </Button>
          <Button onClick={handleSubmit} disabled={submitting}>
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
