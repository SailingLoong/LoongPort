import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Download } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { settingsApi, ccSwitchImportApi } from "@/lib/api";
import type { CcSwitchImportPreview } from "@/lib/api/ccSwitchImport";
import type { Settings } from "@/types";
import { CcSwitchImportDialog } from "@/components/settings/CcSwitchImportDialog";

/**
 * 「从 cc-switch 导入」的总入口：**图标旁的小按钮 + 首次启动弹窗**。
 *
 * - **图标旁按钮**：LoongPort 品牌名旁边的小下载图标，检测到 `~/.cc-switch/cc-switch.db`
 *   才显示（`preview.sourceExists`），点了打开导入确认框。
 * - **首启弹窗**：第一次打开时（`ccSwitchImportPrompted` 还没置过）如果检测到 cc-switch
 *   数据，自动弹出「是否一键导入」的确认框；确认或关闭都记下「问过了」，
 *   下次启动不再打扰 —— 与 `StatsNoticeDialog` 的 `statsNoticeConfirmed` 同一个惯例。
 */
export function CcSwitchImportEntry() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [preview, setPreview] = useState<CcSwitchImportPreview | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [isFirstLaunch, setIsFirstLaunch] = useState(false);

  useEffect(() => {
    let cancelled = false;
    // 两个事实都要：有没有 cc-switch 数据（决定弹不弹/显不显按钮）、问过没。
    Promise.all([settingsApi.get(), ccSwitchImportApi.getPreview()])
      .then(([s, p]) => {
        if (cancelled) return;
        setSettings(s);
        setPreview(p);
        if (s.ccSwitchImportPrompted === undefined && p.sourceExists) {
          setIsFirstLaunch(true);
          setDialogOpen(true);
        }
      })
      // 任何一个读失败就不弹：宁可这次不提示，也不能为这个功能在启动时弹一个报错。
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const markPrompted = useCallback(async () => {
    if (!settings || settings.ccSwitchImportPrompted) return;
    const { webdavSync: _webdavSync, ...rest } = settings;
    try {
      await settingsApi.save({ ...rest, ccSwitchImportPrompted: true });
    } catch {
      // 存不进去下次再弹，无害。
    }
  }, [settings]);

  const handleOpenChange = useCallback(
    (next: boolean) => {
      setDialogOpen(next);
      // 首启那一轮：对话框关掉（无论导没导）都记下「问过了」。
      if (!next && isFirstLaunch) {
        setIsFirstLaunch(false);
        void markPrompted();
      }
    },
    [isFirstLaunch, markPrompted],
  );

  const handleImported = useCallback(() => {
    // 导入后 provider 列表要刷新（新搬进来的 + 回填的托管档位）。
    void queryClient.invalidateQueries({ queryKey: ["providers"] });
  }, [queryClient]);

  const sourceExists = preview?.sourceExists === true;

  return (
    <>
      {preview === null ? (
        // 预览还没回来：占个等宽位，避免头部跳动。
        <div className="w-8" />
      ) : sourceExists ? (
        <Button
          variant="ghost"
          size="icon"
          onClick={() => setDialogOpen(true)}
          title={t("settings.ccSwitchImport.button", {
            defaultValue: "从 cc-switch 导入",
          })}
          className="hover:bg-black/5 dark:hover:bg-white/5"
        >
          <Download className="w-4 h-4" />
        </Button>
      ) : null}

      <CcSwitchImportDialog
        open={dialogOpen}
        onOpenChange={handleOpenChange}
        onImported={handleImported}
      />
    </>
  );
}
