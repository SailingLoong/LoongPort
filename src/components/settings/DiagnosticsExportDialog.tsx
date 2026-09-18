import { useState } from "react";
import { useTranslation } from "react-i18next";
import { FileArchive, Loader2 } from "lucide-react";
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
import { diagnosticsApi } from "@/lib/api";
import { extractErrorMessage } from "@/utils/errorUtils";

/**
 * 「导出诊断包」确认弹窗：唯一的选项是是否附带站点域名清单（默认不含 ——
 * 域名是用户的使用细节，知情后才带）。导出动作本身只写本地文件，
 * 发给谁（群里/私聊）完全由用户自己决定。
 */
export interface DiagnosticsExportDialogProps {
  open: boolean;
  onCancel: () => void;
}

export function DiagnosticsExportDialog({
  open,
  onCancel,
}: DiagnosticsExportDialogProps) {
  const { t } = useTranslation();
  const [includeSites, setIncludeSites] = useState(false);
  const [exporting, setExporting] = useState(false);

  const handleExport = async () => {
    setExporting(true);
    try {
      // null = 用户在保存对话框取消，安静收场。
      const result = await diagnosticsApi.exportDiagnostics(includeSites);
      if (result) {
        toast.success(
          t("settings.exportDiagnosticsSuccess", { path: result.filePath }),
          { closeButton: true },
        );
        onCancel();
      }
    } catch (error) {
      toast.error(extractErrorMessage(error), { closeButton: true });
    } finally {
      setExporting(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && !exporting) onCancel();
      }}
    >
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{t("settings.exportDiagnosticsTitle")}</DialogTitle>
          <DialogDescription>
            {t("settings.exportDiagnosticsDescription")}
          </DialogDescription>
        </DialogHeader>
        <label className="flex cursor-pointer items-start gap-2 rounded-lg border border-border/60 bg-background/50 px-4 py-3">
          <Checkbox
            checked={includeSites}
            onCheckedChange={(checked) => setIncludeSites(checked === true)}
            className="mt-0.5"
          />
          <span className="min-w-0">
            <span className="block text-sm leading-none">
              {t("settings.exportDiagnosticsIncludeSites")}
            </span>
            <span className="mt-1 block text-xs text-muted-foreground">
              {t("settings.exportDiagnosticsIncludeSitesHint")}
            </span>
          </span>
        </label>
        <DialogFooter className="gap-2">
          <Button variant="ghost" onClick={onCancel} disabled={exporting}>
            {t("common.cancel")}
          </Button>
          <Button onClick={handleExport} disabled={exporting}>
            {exporting ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <FileArchive className="h-4 w-4" />
            )}
            {t("settings.exportDiagnosticsConfirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
