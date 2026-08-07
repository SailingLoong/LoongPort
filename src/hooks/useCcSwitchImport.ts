import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ccSwitchImportApi } from "@/lib/api";
import type {
  CcSwitchImportPreview,
  CcSwitchImportReport,
} from "@/lib/api/ccSwitchImport";

export type CcSwitchImportStatus =
  | "idle"
  | "loading"
  | "importing"
  | "success"
  | "error";

export interface UseCcSwitchImportResult {
  /** 最近一次拉到的预览（可能为 null = 还没拉 / 源库不存在）。 */
  preview: CcSwitchImportPreview | null;
  status: CcSwitchImportStatus;
  errorMessage: string | null;
  /** 最近一次导入的报告（null = 还没导过 / 失败）。 */
  report: CcSwitchImportReport | null;
  isImporting: boolean;
  /** 拉预览（设置页/首启弹窗挂载时调一次）。 */
  loadPreview: () => Promise<void>;
  /** 执行导入。返回报告；失败返回 null（错误已 toast）。 */
  runImport: () => Promise<CcSwitchImportReport | null>;
  /** 清掉 status/error/report，回到待机。 */
  reset: () => void;
}

export function useCcSwitchImport(): UseCcSwitchImportResult {
  const { t } = useTranslation();

  const [preview, setPreview] = useState<CcSwitchImportPreview | null>(null);
  const [status, setStatus] = useState<CcSwitchImportStatus>("idle");
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [report, setReport] = useState<CcSwitchImportReport | null>(null);
  const [isImporting, setIsImporting] = useState(false);

  const loadPreview = useCallback(async () => {
    setStatus("loading");
    setErrorMessage(null);
    try {
      const p = await ccSwitchImportApi.getPreview();
      setPreview(p);
      setStatus("idle");
    } catch (error) {
      console.error("[useCcSwitchImport] preview failed", error);
      setStatus("error");
      setErrorMessage(
        error instanceof Error ? error.message : String(error ?? ""),
      );
    }
  }, []);

  const runImport = useCallback(async () => {
    if (isImporting) return null;
    setIsImporting(true);
    setStatus("importing");
    setErrorMessage(null);
    try {
      const r = await ccSwitchImportApi.import();
      setReport(r);
      setStatus("success");
      if (r.warnings && r.warnings.length > 0) {
        // 非致命问题（回填失败/后置同步失败），导入仍成功但用户该知道。
        console.warn("[useCcSwitchImport] import warnings", r.warnings);
      }
      return r;
    } catch (error) {
      console.error("[useCcSwitchImport] import failed", error);
      setStatus("error");
      const message =
        error instanceof Error ? error.message : String(error ?? "");
      setErrorMessage(message);
      toast.error(
        t("settings.ccSwitchImport.importFailed", {
          defaultValue: "从 cc-switch 导入失败: {{message}}",
          message,
        }),
      );
      return null;
    } finally {
      setIsImporting(false);
    }
  }, [isImporting, t]);

  const reset = useCallback(() => {
    setStatus("idle");
    setErrorMessage(null);
    setReport(null);
  }, []);

  return {
    preview,
    status,
    errorMessage,
    report,
    isImporting,
    loadPreview,
    runImport,
    reset,
  };
}
