import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import type { useQoderAutomaticClaim } from "./useQoderAutomaticClaim";

export function QoderAutomaticClaim({
  state,
}: {
  state: ReturnType<typeof useQoderAutomaticClaim>;
}) {
  const { t } = useTranslation();
  return (
    <section className="space-y-3 border-t border-border-default px-6 py-4">
      <div className="flex items-center justify-between gap-3">
        <span className="text-sm font-medium">
          {t("qoderDaily.automatic.title", { defaultValue: "自动签到" })}
        </span>
        <Switch
          aria-label={t("qoderDaily.automatic.title", {
            defaultValue: "自动签到",
          })}
          checked={state.context?.enabled ?? false}
          disabled={!state.canChange}
          onCheckedChange={(checked) => void state.setEnabled(checked)}
        />
      </div>
      <p className="text-xs text-muted-foreground">
        {t("qoderDaily.automatic.window", {
          defaultValue:
            "开启后仅在 LoongPort 运行期间自动处理。以服务端实际活动窗口为准，UTC+8 每日 10:00 为活动边界；不补造活动，也不保证每次新增积分。",
        })}
      </p>
      <p className="text-xs text-muted-foreground">
        {t("qoderDaily.automatic.device", {
          defaultValue:
            "目前仅支持已验证官方组件的 Qoder Desktop 0.4.8（macOS ARM64）。此功能仅管理签到，不代表已接通原生推理消费。",
        })}
      </p>
      {!state.context?.enabled && (
        <label className="flex items-start gap-2 text-sm">
          <Checkbox
            aria-label={t("qoderDaily.automatic.consent", {
              defaultValue: "确认国际版 Qoder PAT 并持续授权自动签到",
            })}
            checked={state.declared}
            disabled={!state.canDeclare}
            onCheckedChange={(value) => state.declare(value === true)}
          />
          <span>
            {t("qoderDaily.automatic.consentDetail", {
              defaultValue:
                "我确认当前凭据是国际版 Qoder PAT，并授权 LoongPort 在运行期间定期查询并领取当前账号的每日活动积分，直到我关闭自动签到或凭据变化。此持续授权会保存，与上方仅本次手动领取的声明独立。",
            })}
          </span>
        </label>
      )}
      {state.context?.subject && (
        <p className="text-xs text-muted-foreground">{state.context.subject}</p>
      )}
      {state.context?.reason && (
        <p role="status" className="text-xs text-muted-foreground">
          {t("qoderDaily.automatic.unavailable", {
            defaultValue:
              "当前无法开启自动签到，请检查已保存凭据与官方设备组件。",
          })}
        </p>
      )}
      {state.unknown ? (
        <p role="alert" className="text-sm">
          {t("qoderDaily.automatic.unknown", {
            defaultValue:
              "自动签到设置结果尚未确认。请刷新状态确认，期间不会重复提交设置。",
          })}
        </p>
      ) : state.readError ? (
        <p role="alert" className="text-sm">
          {t("qoderDaily.automatic.readError", {
            defaultValue: "无法确认自动签到状态，请刷新后再操作。",
          })}
        </p>
      ) : state.busy ? (
        <p role="status" className="text-sm">
          {t("qoderDaily.automatic.saving", {
            defaultValue: "正在保存自动签到授权…",
          })}
        </p>
      ) : null}
      <Button
        type="button"
        size="sm"
        variant="outline"
        disabled={!state.canRefresh}
        onClick={state.refresh}
      >
        {t("qoderDaily.automatic.refresh", {
          defaultValue: "刷新自动签到状态",
        })}
      </Button>
    </section>
  );
}
