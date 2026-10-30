import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useWorkBuddyNativeRecoveryViewModel } from "./useWorkBuddyNativeRecoveryViewModel";

export function WorkBuddyNativeRecovery() {
  const { t } = useTranslation();
  const vm = useWorkBuddyNativeRecoveryViewModel();
  return (
    <section
      aria-label={t("workbuddy.nativeRecovery.title", {
        defaultValue: "认证文件恢复",
      })}
      className="space-y-2 rounded-xl border border-border bg-card p-3 text-xs"
    >
      <h3 className="text-sm font-medium">
        {t("workbuddy.nativeRecovery.title", { defaultValue: "认证文件恢复" })}
      </h3>
      <p>
        {t("workbuddy.nativeRecovery.description", {
          defaultValue:
            "先退出 WorkBuddy 和相关进程。恢复按当前保险库的原日志执行，不会自动退出或启动客户端。",
        })}
      </p>
      <Button
        size="sm"
        variant="outline"
        disabled={vm.busy}
        onClick={() => void vm.check()}
      >
        {t("workbuddy.nativeRecovery.check", { defaultValue: "检查恢复状态" })}
      </Button>{" "}
      <Button
        size="sm"
        variant="outline"
        disabled={!vm.canRecover}
        onClick={vm.open}
      >
        {t("workbuddy.nativeRecovery.open", { defaultValue: "恢复认证文件" })}
      </Button>
      {vm.busy && (
        <p>
          {t("workbuddy.nativeRecovery.busy", {
            defaultValue: "正在处理；离开页面不会取消已经提交的恢复操作。",
          })}
        </p>
      )}
      {vm.inspection && (
        <p role="status">
          {vm.afterReceipt
            ? t(`workbuddy.nativeRecovery.after.${vm.inspection.phase}`, {
                defaultValue:
                  vm.inspection.phase === "clear"
                    ? "恢复后读取的当前日志快照：无待恢复项。"
                    : vm.inspection.phase === "pending"
                      ? "恢复后读取的当前日志快照：仍待恢复。"
                      : "恢复后读取的当前日志快照：不存在。",
              })
            : t(`workbuddy.nativeRecovery.phase.${vm.inspection.phase}`, {
                defaultValue:
                  vm.inspection.phase === "pending"
                    ? "日志当前待恢复。"
                    : vm.inspection.phase === "clear"
                      ? "日志当前无待恢复项。"
                      : "当前没有日志。",
              })}
        </p>
      )}
      {vm.receipt && (
        <p role="status">
          {vm.receipt === "restored"
            ? t("workbuddy.nativeRecovery.restored", {
                defaultValue: "最近一次收到的恢复回执：原文件已恢复。",
              })
            : t("workbuddy.nativeRecovery.alreadyApplied", {
                defaultValue:
                  "最近一次收到的恢复回执：已提交的目标文件仍匹配。",
              })}
        </p>
      )}
      {vm.error && (
        <p role="alert">
          {t(`workbuddy.nativeRecovery.error.${vm.error}`, {
            defaultValue:
              vm.error === "unsupported"
                ? "当前平台不支持此操作。"
                : vm.error === "checkFailed"
                  ? "检查失败，请重新检查。"
                  : vm.error === "afterCheckFailed"
                    ? "已收到恢复回执，但后续日志检查失败。"
                    : "恢复结果未确认；操作可能仍在执行，不能自动重试。",
          })}
        </p>
      )}
      {vm.unresolvedRecovery && vm.error !== "recoveryUnknown" && (
        <p>
          {t("workbuddy.nativeRecovery.unresolved", {
            defaultValue:
              "此前恢复结果仍未确认；当前快照及后续回执不能证明此前请求的结果。",
          })}
        </p>
      )}
      <p>
        {t("workbuddy.nativeRecovery.unknown", {
          defaultValue:
            "日志快照不证明此前未知请求的结果，也不证明客户端账号已生效。",
        })}
      </p>
      <ConfirmDialog
        isOpen={vm.confirming}
        title={t("workbuddy.nativeRecovery.confirmTitle", {
          defaultValue: "确认恢复认证文件",
        })}
        message={t("workbuddy.nativeRecovery.confirmDescription", {
          defaultValue:
            "请先退出 WorkBuddy 和相关进程。将依据当前保险库中的原日志恢复认证文件；不会自动启动客户端，也不会重放切换。",
        })}
        confirmText={t("workbuddy.nativeRecovery.confirm", {
          defaultValue: "确认恢复",
        })}
        cancelText={t("common.cancel", { defaultValue: "取消" })}
        variant="info"
        pending={vm.busy}
        onConfirm={() => void vm.recover()}
        onCancel={vm.cancel}
      />
    </section>
  );
}
