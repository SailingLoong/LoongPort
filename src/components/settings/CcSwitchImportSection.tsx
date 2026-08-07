import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Import, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useQueryClient } from "@tanstack/react-query";
import { useCcSwitchImport } from "@/hooks/useCcSwitchImport";
import { CcSwitchImportDialog } from "@/components/settings/CcSwitchImportDialog";

/**
 * 设置页 advanced → data 区里的「从 cc-switch 导入」小节。
 *
 * 检测到 `~/.cc-switch/cc-switch.db` 才给按钮（`preview.sourceExists`），否则灰显提示 ——
 * 有源可导才值得占用户一眼。
 */
export function CcSwitchImportSection() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { preview, loadPreview } = useCcSwitchImport();
  const [dialogOpen, setDialogOpen] = useState(false);

  useEffect(() => {
    void loadPreview();
  }, [loadPreview]);

  const handleImported = useCallback(() => {
    // 导入后 provider 列表要刷新（新搬进来的 + 回填的托管档位）。
    void queryClient.invalidateQueries({ queryKey: ["providers"] });
  }, [queryClient]);

  const sourceExists = preview?.sourceExists === true;

  return (
    <section className="space-y-4">
      <div className="flex items-center justify-between gap-4">
        <div className="space-y-1">
          <p className="text-sm font-medium">
            {t("settings.ccSwitchImport.sectionTitle", {
              defaultValue: "从 cc-switch 导入",
            })}
          </p>
          <p className="text-xs text-muted-foreground">
            {t("settings.ccSwitchImport.sectionHint", {
              defaultValue:
                "把 cc-switch 的 provider / MCP / skills / prompt 一次性复制过来，不动 cc-switch。",
            })}
          </p>
        </div>

        {preview === null ? (
          <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
        ) : sourceExists ? (
          <Button type="button" onClick={() => setDialogOpen(true)}>
            <Import className="mr-2 h-4 w-4" />
            {t("settings.ccSwitchImport.button", {
              defaultValue: "从 cc-switch 导入",
            })}
          </Button>
        ) : (
          <p className="text-xs text-muted-foreground">
            {t("settings.ccSwitchImport.noSource", {
              defaultValue:
                "未检测到 cc-switch 数据（~/.cc-switch/cc-switch.db）",
            })}
          </p>
        )}
      </div>

      <CcSwitchImportDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        onImported={handleImported}
      />
    </section>
  );
}
