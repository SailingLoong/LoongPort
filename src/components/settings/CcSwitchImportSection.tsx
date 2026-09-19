import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Download, Import, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { useQueryClient } from "@tanstack/react-query";
import { useCcSwitchImport } from "@/hooks/useCcSwitchImport";
import { CcSwitchImportDialog } from "@/components/settings/CcSwitchImportDialog";

/**
 * 设置页高级区里的「从 cc-switch 导入」折叠项（自带 AccordionItem）。
 *
 * 这是一次性迁移动作，只对装过 cc-switch 的用户有意义 —— 所以**整个折叠项**只在
 * 检测到 `~/.cc-switch/cc-switch.db`（`preview.sourceExists`）且版本导得动时才渲染；
 * 没源库的用户不再看到一个永远用不了的空抽屉。源库在但 cc-switch 比当前应用新
 * （版本导不动）同样整项隐藏 —— 留一个点了必报「版本过新」的按钮，不如不给。
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

  // 预览已返回但没源库（或版本导不动）→ 整项不渲染；预览在途也不占位。
  const importable =
    preview?.sourceExists === true && preview.canImport !== false;
  if (!importable) {
    return null;
  }

  return (
    <AccordionItem
      value="ccSwitchImport"
      className="rounded-xl glass-card overflow-hidden"
    >
      <AccordionTrigger className="px-6 py-4 hover:no-underline hover:bg-muted/50 data-[state=open]:bg-muted/50">
        <div className="flex items-center gap-3">
          <Download className="h-5 w-5 text-blue-500" />
          <div className="text-left">
            <h3 className="text-base font-semibold">
              {t("settings.ccSwitchImport.title", {
                defaultValue: "从 cc-switch 导入",
              })}
            </h3>
            <p className="text-sm text-muted-foreground font-normal">
              {t("settings.ccSwitchImport.description", {
                defaultValue:
                  "把 cc-switch 的配置一次性复制过来，不动 cc-switch 那边",
              })}
            </p>
          </div>
        </div>
      </AccordionTrigger>
      <AccordionContent className="px-6 pb-6 pt-4 border-t border-border/50">
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
            ) : (
              <Button type="button" onClick={() => setDialogOpen(true)}>
                <Import className="mr-2 h-4 w-4" />
                {t("settings.ccSwitchImport.button", {
                  defaultValue: "从 cc-switch 导入",
                })}
              </Button>
            )}
          </div>

          <CcSwitchImportDialog
            open={dialogOpen}
            onOpenChange={setDialogOpen}
            onImported={handleImported}
          />
        </section>
      </AccordionContent>
    </AccordionItem>
  );
}
