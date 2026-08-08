import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import {
  Loader2,
  Import,
  CheckCircle2,
  XCircle,
  AlertTriangle,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useCcSwitchImport } from "@/hooks/useCcSwitchImport";
import type { SkippedProvider } from "@/lib/api/ccSwitchImport";

interface CcSwitchImportDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 导入成功后要做的刷新（invalidate providers 查询等）。 */
  onImported?: () => void | Promise<void>;
}

/**
 * 从 cc-switch 一键导入的确认对话框。
 *
 * 三个入口共用：设置页按钮、LoongPort 图标旁按钮、首次启动弹窗 —— 它们只控制 `open`，
 * 预览/导入/报告的流程全在这个组件里。
 *
 * ## 只读源库
 *
 * 文案明确「这会把 cc-switch 的配置**复制**过来，不动 cc-switch 那边」—— 导入绝不改源库
 * （后端 `SQLITE_OPEN_READ_ONLY` + 集成测试钉着）。
 */
export function CcSwitchImportDialog({
  open,
  onOpenChange,
  onImported,
}: CcSwitchImportDialogProps) {
  const { t } = useTranslation();
  const {
    preview,
    status,
    errorMessage,
    report,
    isImporting,
    loadPreview,
    runImport,
    reset,
  } = useCcSwitchImport();

  // 每次打开都重拉预览（源库可能在两次打开之间变了）。
  useEffect(() => {
    if (open) {
      reset();
      void loadPreview();
    }
  }, [open, reset, loadPreview]);

  const handleConfirm = async () => {
    const r = await runImport();
    if (r) {
      await onImported?.();
    }
  };

  const handleClose = () => {
    onOpenChange(false);
  };

  const showReport = Boolean(report);
  const loading = status === "loading" || status === "importing";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* 不用 DialogHeader / DialogFooter 分区壳 —— 与 StatsNoticeDialog 同一条理由：
          那两个壳自带 border-b / border-t + bg-muted/20，是给表单类弹窗设计的分区。
          用在这（一段要读的说明 + 预览/报告）上会把内容切成三段，像系统报错框
          而不是一个邀请。直接在 DialogContent 里排版，仍用官方组件。 */}
      <DialogContent className="sm:max-w-lg gap-0 p-6">
        <DialogTitle className="flex items-center gap-2 text-base font-semibold">
          <Import className="h-5 w-5 text-blue-500" />
          {t("settings.ccSwitchImport.title", {
            defaultValue: "从 cc-switch 导入",
          })}
        </DialogTitle>
        <DialogDescription className="mt-1.5 text-sm leading-relaxed">
          {t("settings.ccSwitchImport.description", {
            defaultValue:
              "这会把 cc-switch 的配置复制到 LoongPort，不会动 cc-switch 那边。",
          })}
        </DialogDescription>

        <div className="mt-4">
          {loading && (
            <div className="flex items-center justify-center gap-2 py-8 text-muted-foreground">
              <Loader2 className="h-5 w-5 animate-spin" />
              {t("settings.ccSwitchImport.loading", {
                defaultValue: "正在读取 cc-switch 数据…",
              })}
            </div>
          )}

          {!loading && errorMessage && !report && (
            <div className="flex items-start gap-2 rounded-lg border border-red-500/30 bg-red-500/5 p-4 text-sm text-red-600">
              <XCircle className="mt-0.5 h-4 w-4 shrink-0" />
              <span>{errorMessage}</span>
            </div>
          )}

          {!loading && !errorMessage && !report && preview && (
            <PreviewBody preview={preview} />
          )}

          {!loading && report && <ReportBody report={report} />}
        </div>

        <div className="mt-5 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end sm:items-center">
          {showReport ? (
            <Button type="button" onClick={handleClose}>
              {t("common.close", { defaultValue: "关闭" })}
            </Button>
          ) : (
            <>
              <Button
                type="button"
                variant="ghost"
                onClick={handleClose}
                disabled={isImporting}
              >
                {t("common.cancel", { defaultValue: "取消" })}
              </Button>
              <Button
                type="button"
                disabled={
                  isImporting || status === "loading" || !preview?.sourceExists
                }
                onClick={handleConfirm}
              >
                {isImporting ? (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                ) : (
                  <Import className="mr-2 h-4 w-4" />
                )}
                {isImporting
                  ? t("settings.ccSwitchImport.importing", {
                      defaultValue: "导入中…",
                    })
                  : t("settings.ccSwitchImport.confirm", {
                      defaultValue: "一键导入",
                    })}
              </Button>
            </>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function PreviewBody({
  preview,
}: {
  preview: NonNullable<ReturnType<typeof useCcSwitchImport>["preview"]>;
}) {
  const { t } = useTranslation();

  if (!preview.sourceExists) {
    return (
      <p className="text-sm text-muted-foreground">
        {t("settings.ccSwitchImport.noSource", {
          defaultValue:
            "未检测到 cc-switch 数据（~/.cc-switch/cc-switch.db）。",
        })}
      </p>
    );
  }

  const { providers } = preview;
  return (
    <div className="space-y-4 text-sm">
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Stat
          label={t("settings.ccSwitchImport.statProviders", {
            defaultValue: "Providers",
          })}
          value={providers.willImport}
        />
        <Stat
          label={t("settings.ccSwitchImport.statMcp", {
            defaultValue: "MCP",
          })}
          value={preview.mcpServers}
        />
        <Stat
          label={t("settings.ccSwitchImport.statPrompts", {
            defaultValue: "Prompts",
          })}
          value={preview.prompts}
        />
        <Stat
          label={t("settings.ccSwitchImport.statSkills", {
            defaultValue: "Skills",
          })}
          value={preview.skills}
        />
      </div>

      {providers.mergedToRelay.length > 0 && (
        <NotImportedList
          title={t("settings.ccSwitchImport.mergedToRelayTitle", {
            defaultValue:
              "以下 {{count}} 条的站点已由中转站组维护，统一在中转站区管理，不重复导入：",
            count: providers.mergedToRelay.length,
          })}
          items={providers.mergedToRelay}
        />
      )}

      {providers.skipped.length > 0 && (
        <NotImportedList
          title={t("settings.ccSwitchImport.skippedTitle", {
            defaultValue: "以下 N 条已由 LoongPort 接管，不会重复导入：",
            count: providers.skipped.length,
          })}
          items={providers.skipped}
        />
      )}

      {preview.notes.map((note, i) => (
        <p key={i} className="text-xs text-muted-foreground">
          {note}
        </p>
      ))}
    </div>
  );
}

function ReportBody({
  report,
}: {
  report: NonNullable<ReturnType<typeof useCcSwitchImport>["report"]>;
}) {
  const { t } = useTranslation();
  return (
    <div className="space-y-4 text-sm">
      <div className="flex items-start gap-2 rounded-lg border border-emerald-500/30 bg-emerald-500/5 p-4">
        <CheckCircle2 className="mt-0.5 h-5 w-5 shrink-0 text-emerald-600" />
        <div className="space-y-1">
          <p className="font-medium text-emerald-700">
            {t("settings.ccSwitchImport.importSuccess", {
              defaultValue: "导入完成",
            })}
          </p>
          <p className="text-muted-foreground">
            {t("settings.ccSwitchImport.reportSummary", {
              defaultValue:
                "导入 {{providers}} 个 provider、{{mcp}} 个 MCP、{{prompts}} 个 prompt、{{skills}} 个 skill。",
              providers: report.providersImported,
              mcp: report.mcpImported,
              prompts: report.promptsImported,
              skills: report.skillsImported,
            })}
          </p>
          {report.backupId && (
            <p className="text-xs text-muted-foreground">
              {t("settings.ccSwitchImport.backupId", {
                defaultValue: "导入前已自动备份：{{backupId}}",
                backupId: report.backupId,
              })}
            </p>
          )}
        </div>
      </div>

      {report.relaysMerged.length > 0 && (
        <NotImportedList
          title={t("settings.ccSwitchImport.mergedToRelayAfterTitle", {
            defaultValue:
              "这 {{count}} 条的站点已由中转站组维护，请在中转站区查看：",
            count: report.relaysMerged.length,
          })}
          items={report.relaysMerged}
        />
      )}

      {report.providersSkipped.length > 0 && (
        <NotImportedList
          title={t("settings.ccSwitchImport.skippedAfterTitle", {
            defaultValue:
              "这 {{count}} 条已由 LoongPort 接管，请在中转站区查看：",
            count: report.providersSkipped.length,
          })}
          items={report.providersSkipped}
        />
      )}

      {report.warnings.map((w, i) => (
        <p key={i} className="flex items-start gap-1.5 text-xs text-amber-600">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          {w}
        </p>
      ))}
    </div>
  );
}

/** 一组「不导入」的 provider 及其原因标题（同指纹跳过 / 同站点归入中转站组）。 */
function NotImportedList({
  title,
  items,
}: {
  title: string;
  items: SkippedProvider[];
}) {
  return (
    <div className="space-y-2 rounded-lg border border-amber-500/30 bg-amber-500/5 p-3">
      <p className="flex items-center gap-1.5 font-medium text-amber-700">
        <AlertTriangle className="h-4 w-4" />
        {title}
      </p>
      <ul className="list-disc pl-5 text-muted-foreground">
        {items.map((s) => (
          <li key={`${s.appType}-${s.name}`}>
            {s.name}
            <span className="text-xs">（{s.appType}）</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-lg border border-border bg-muted/40 p-3 text-center">
      <div className="text-xl font-semibold">{value}</div>
      <div className="text-xs text-muted-foreground">{label}</div>
    </div>
  );
}
