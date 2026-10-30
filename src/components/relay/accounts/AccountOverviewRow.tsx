import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import type { AccountRowVM } from "./accountOverviewProjection";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useWorkBuddyNativeSwitch } from "./useWorkBuddyNativeSwitch";
/** Render only projected fields; no snapshot/platform branches or account commands. */
export function AccountOverviewRow({
  row,
  disabled,
  onOpenDetails,
}: {
  row: AccountRowVM;
  disabled?: boolean;
  onOpenDetails: (rowKey: string) => void;
}) {
  const { t } = useTranslation();
  const reasonId = useId();
  const nativeSwitch = useWorkBuddyNativeSwitch(row, disabled);
  return (
    <article
      aria-label={row.name}
      className="grid min-w-0 gap-3 border-b border-border px-3 py-3 last:border-b-0 lg:grid-cols-[minmax(9rem,1fr)_minmax(14rem,2fr)_minmax(14rem,2fr)_auto] lg:items-center"
    >
      <div className="min-w-0">
        <p className="break-words text-sm font-semibold">{row.name}</p>
        {row.identity && (
          <p className="text-xs text-muted-foreground">{row.identity}</p>
        )}
      </div>
      <dl className="grid min-w-0 grid-cols-2 gap-x-3 gap-y-1 text-xs">
        {row.facts.map((fact) => (
          <div key={fact.label}>
            <dt className="text-muted-foreground">{fact.label}</dt>
            <dd className="break-words">{fact.value}</dd>
          </div>
        ))}
      </dl>
      <dl className="grid min-w-0 gap-1 text-xs">
        {[
          [
            t("accountRewardGlance.lastSuccess", { defaultValue: "最近成功" }),
            row.lastSuccess,
          ],
          [
            t("accountRewardGlance.nextExpiry", { defaultValue: "即将到期" }),
            row.nextExpiry,
          ],
        ].map(([label, value]) => (
          <div key={label}>
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="break-words">{value}</dd>
          </div>
        ))}
        {row.claimIssue && (
          <div>
            <dd className="break-words text-destructive">{row.claimIssue}</dd>
          </div>
        )}
      </dl>
      <div className="flex flex-wrap items-center gap-2 lg:max-w-48">
        <Button
          size="sm"
          disabled={!nativeSwitch.canSwitch}
          aria-describedby={reasonId}
          onClick={nativeSwitch.open}
        >
          {row.switchAction.label}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={disabled}
          aria-label={t("accountsOverview.viewAccount", {
            name: row.name,
            defaultValue: "查看 {{name}}",
          })}
          onClick={() => onOpenDetails(row.rowKey)}
        >
          {t("accountsOverview.details")}
        </Button>
        <p id={reasonId} className="w-full text-xs text-muted-foreground">
          {!row.switchAction.enabled && row.switchAction.reason}
        </p>
        {nativeSwitch.report && (
          <p role="status">
            {t(
              `accountsOverview.nativeSwitch.${nativeSwitch.report.transaction}`,
              {
                defaultValue:
                  nativeSwitch.report.transaction === "applied"
                    ? "认证文件已切换。"
                    : "认证文件已是目标内容。",
              },
            )}{" "}
            {t(`accountsOverview.nativeSwitch.${nativeSwitch.report.restart}`, {
              defaultValue:
                nativeSwitch.report.restart === "notNeeded"
                  ? "客户端原本未运行，未启动。"
                  : nativeSwitch.report.restart === "requested"
                    ? "已请求启动；账号是否生效待验证。"
                    : nativeSwitch.report.restart === "failed"
                      ? "启动失败。"
                      : "启动结果未知。",
            })}
          </p>
        )}
        {nativeSwitch.error && (
          <p role="alert">
            {t(
              `accountsOverview.nativeSwitch.error.${nativeSwitch.error.stage}`,
              {
                reason: nativeSwitch.error.reason ?? "",
                defaultValue:
                  nativeSwitch.error.stage === "outcomeUnknown"
                    ? "切换结果未知，请先核对原操作，不要重复切换。"
                    : nativeSwitch.error.stage === "quitRefused"
                      ? "客户端拒绝退出，未切换认证文件。"
                      : nativeSwitch.error.stage === "quitTimeout"
                        ? "客户端退出超时，未切换认证文件。"
                        : nativeSwitch.error.stage === "writersRemaining"
                          ? "仍有客户端进程运行，未切换认证文件。"
                          : nativeSwitch.error.stage === "busy"
                            ? "另一个切换正在进行，未执行本次切换。"
                            : nativeSwitch.error.stage === "sourceChanged"
                              ? "客户端来源已变化，未切换认证文件。"
                              : nativeSwitch.error.stage === "transaction"
                                ? `认证文件事务报告异常（${nativeSwitch.error.reason ?? "unknown"}），再次操作前请检查恢复状态。`
                                : "切换未完成。",
              },
            )}
          </p>
        )}
        <ConfirmDialog
          isOpen={nativeSwitch.confirming}
          title={t("accountsOverview.nativeSwitch.title", {
            defaultValue: "确认切换账号",
          })}
          message={t("accountsOverview.nativeSwitch.confirm", {
            name: row.name,
            defaultValue:
              "将退出正在运行的 WorkBuddy，切换到 {{name}} 的认证文件，并尝试重开。账号是否生效需另行验证。",
          })}
          confirmText={t("accountsOverview.nativeSwitch.action", {
            defaultValue: "确认切换",
          })}
          cancelText={t("common.cancel", { defaultValue: "取消" })}
          variant="info"
          pending={nativeSwitch.busy}
          onConfirm={() => void nativeSwitch.confirm()}
          onCancel={nativeSwitch.cancel}
        />
      </div>
    </article>
  );
}
