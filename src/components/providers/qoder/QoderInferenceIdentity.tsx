import { useTranslation } from "react-i18next";
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
import { useQoderInferenceIdentity } from "./useQoderInferenceIdentity";

export function QoderInferenceIdentity({
  providerId,
  sourceGeneration,
  disabled = false,
}: {
  providerId: string;
  // Original non-secret Provider revision, used only to invalidate this view.
  sourceGeneration?: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const {
    open,
    changeOpen,
    declared,
    setDeclared,
    context,
    visibleContext,
    actions,
    canAct,
    busy,
    feedback,
    perform,
    statusMessages,
    actionLabels,
  } = useQoderInferenceIdentity({ providerId, sourceGeneration, disabled });
  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogTrigger asChild>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={disabled}
          onClick={(event) => event.stopPropagation()}
        >
          {t("qoderHttpIdentity.title", { defaultValue: "Qoder 推理身份" })}
        </Button>
      </DialogTrigger>
      <DialogContent onClick={(event) => event.stopPropagation()}>
        <DialogHeader>
          <DialogTitle>
            {t("qoderHttpIdentity.title", { defaultValue: "Qoder 推理身份" })}
          </DialogTitle>
          <DialogDescription>
            {context?.source === "chinaQoderDevice"
              ? "这是本应用独立授权身份。授权已保存不代表真实推理已验证；登录或更换账号请使用独立授权入口。"
              : t("qoderHttpIdentity.description", {
                  defaultValue:
                    "使用此供应商已保存的对应区域 Qoder PAT 主动建立推理身份。打开窗口不会交换凭据；旧 SDK Token 不会自动转换为 PAT。",
                })}
          </DialogDescription>
        </DialogHeader>
        {visibleContext && (
          <p role="status">{statusMessages[visibleContext.status]}</p>
        )}
        {!visibleContext && !feedback && (
          <p role="status">
            {t("qoderHttpIdentity.loading", {
              defaultValue: "正在读取身份状态…",
            })}
          </p>
        )}
        {actions.length > 0 && (
          <label className="flex items-start gap-2">
            <Checkbox
              checked={declared}
              onCheckedChange={(value) => setDeclared(value === true)}
              disabled={disabled || busy}
              aria-label={t("qoderHttpIdentity.source", {
                defaultValue: "确认当前凭据来源为本账号区域 Qoder PAT",
              })}
            />
            <span>
              {visibleContext?.source === "chinaQoderPat"
                ? "我确认当前保存的是本人独立授权用于 LoongPort 的 Qoder CN PAT，不是从官方客户端复制的登录态或刷新令牌。本次声明不会被保存。"
                : t("qoderHttpIdentity.sourceDetail", {
                    defaultValue:
                      "我明确确认此供应商当前保存的是下方所示区域的 Qoder PAT。本次声明不会被保存。",
                  })}
            </span>
          </label>
        )}
        {context?.source && (
          <p className="text-sm">
            {context.source === "chinaQoderDevice"
              ? "凭据区域：Qoder CN（本应用独立授权）"
              : context.source === "chinaQoderPat"
                ? "凭据区域：Qoder CN（独立 PAT 模式）"
                : "凭据区域：Qoder 国际版"}
          </p>
        )}
        {feedback && <p role={feedback.role}>{feedback.message}</p>}
        {busy && (
          <p role="status">
            {t("qoderHttpIdentity.busy", {
              defaultValue: "正在处理。关闭此窗口不会撤销已提交的操作。",
            })}
          </p>
        )}
        <div className="flex flex-wrap gap-2">
          {actions.map((action) => (
            <Button
              key={action}
              type="button"
              variant={action === "rebind" ? "outline" : "default"}
              disabled={!canAct}
              onClick={() => void perform(action)}
            >
              {actionLabels[action]}
            </Button>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
