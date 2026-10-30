import { useTranslation } from "react-i18next";
import { QoderAutomaticClaim } from "./QoderAutomaticClaim";
import { useQoderAutomaticClaim } from "./useQoderAutomaticClaim";
import { useQoderDailyClaim } from "./useQoderDailyClaim";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";

export function QoderDailyClaim({
  providerId,
  sourceGeneration,
  disabled = false,
}: {
  providerId: string;
  sourceGeneration?: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const {
    open,
    setOpen,
    declared,
    setDeclared,
    result,
    error,
    busy,
    unavailable,
    ready,
    claim,
    message,
  } = useQoderDailyClaim(providerId, disabled, sourceGeneration);
  const automatic = useQoderAutomaticClaim(
    providerId,
    open,
    disabled,
    sourceGeneration,
  );
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={disabled || busy}
          onClick={(event) => event.stopPropagation()}
        >
          {t("qoderDaily.title", { defaultValue: "Qoder 每日积分" })}
        </Button>
      </DialogTrigger>
      <DialogContent
        className="overflow-y-auto"
        onClick={(event) => event.stopPropagation()}
      >
        <DialogHeader>
          <DialogTitle>
            {t("qoderDaily.title", { defaultValue: "Qoder 每日积分" })}
          </DialogTitle>
          <DialogDescription>
            {t("qoderDaily.description", {
              defaultValue:
                "仅适用于国际版。主动查询当前服务端活动，最多领取一个明确的 100 Credits 活动。不会在聊天或用量刷新时自动领取。",
            })}
          </DialogDescription>
        </DialogHeader>
        {unavailable && (
          <p role="status">
            {unavailable === "OfficialDeviceUnsupported"
              ? t("qoderDaily.deviceUnsupported", {
                  defaultValue:
                    "目前仅支持已安装的 Qoder Desktop 0.4.8（macOS ARM64）。此平台或版本尚未验证，不会运行设备组件或发送认证请求。",
                })
              : unavailable === "OfficialDeviceUnavailable"
                ? t("qoderDaily.deviceMissing", {
                    defaultValue:
                      "未发现国际版 Qoder 桌面设备组件，当前无法领取。不会发送认证请求。",
                  })
                : t("qoderDaily.deviceUnverified", {
                    defaultValue:
                      "设备组件的官方来源尚未验证，当前无法领取。不会运行该组件或发送认证请求。",
                  })}
          </p>
        )}
        <label className="flex items-start gap-2">
          <Checkbox
            checked={declared}
            onCheckedChange={(value) => setDeclared(value === true)}
            disabled={busy || disabled}
            aria-label={t("qoderDaily.source", {
              defaultValue: "确认当前凭据来源为国际版 Qoder PAT",
            })}
          />
          <span>
            {t("qoderDaily.sourceDetail", {
              defaultValue:
                "我明确确认此供应商当前保存的是国际版 Qoder PAT。普通推理 Token 不能用于此操作。本次声明不会被保存。",
            })}
          </span>
        </label>
        {error && (
          <p role="alert">
            {t("qoderDaily.error", {
              defaultValue:
                "操作未完成。请重新打开以查询状态；未确认的领取只会对账，不会自动重发。",
            })}
          </p>
        )}
        {message && <p role="status">{message}</p>}
        {result && "Observed" in result && (
          <p className="text-xs text-muted-foreground">
            {result.Observed.operation.campaignId} ·{" "}
            {result.Observed.operation.startAt}–
            {result.Observed.operation.endAt}
          </p>
        )}
        <Button type="button" disabled={!ready} onClick={() => void claim()}>
          {busy
            ? t("qoderDaily.pending", { defaultValue: "正在处理…" })
            : t("qoderDaily.claim", { defaultValue: "查询并领取" })}
        </Button>
        <QoderAutomaticClaim state={automatic} />
      </DialogContent>
    </Dialog>
  );
}
