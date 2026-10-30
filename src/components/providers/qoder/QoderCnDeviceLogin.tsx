import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useQoderCnDeviceLogin } from "./useQoderCnDeviceLogin";

/** A view owns only its ephemeral grant handle; the backend owns all credentials. */
export function QoderCnDeviceLogin({
  providerId,
  sourceGeneration,
  disabled = false,
}: {
  providerId?: string;
  sourceGeneration?: string;
  disabled?: boolean;
}) {
  const {
    active,
    open,
    rebind,
    context,
    phase,
    feedback,
    busy,
    canStart,
    permittedDeviceAction,
    start,
    openFor,
    close,
    cancel,
    changeOpen,
  } = useQoderCnDeviceLogin({ providerId, sourceGeneration, disabled });
  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <Button
        type="button"
        size="sm"
        variant="outline"
        disabled={!active || disabled}
        onClick={(event) => {
          event.stopPropagation();
          openFor(false);
        }}
      >
        {providerId ? "登录 / 重新授权" : "添加 Qoder CN 独立授权账号"}
      </Button>
      {providerId && (
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={!active || disabled}
          onClick={(event) => {
            event.stopPropagation();
            openFor(true);
          }}
        >
          更换绑定账号
        </Button>
      )}
      <DialogContent onClick={(event) => event.stopPropagation()}>
        <DialogHeader>
          <DialogTitle>
            {rebind ? "更换 Qoder CN 绑定账号" : "Qoder CN 独立授权"}
          </DialogTitle>
          <DialogDescription>
            这是本应用独立会话，不读取官方认证文件，不能保证服务器会话一定并存。
            免费福利请在账号的“CN
            活动状态”中单独查询或领取，不会随登录自动开启；活动已领取不代表今日签到成功。
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3 px-6 pb-6">
          {rebind && (
            <p>此操作将替换此账号的绑定，请在官方页面明确选择要绑定的账号。</p>
          )}
          {providerId && !context && !feedback && (
            <p role="status">正在读取身份状态…</p>
          )}
          {context && !permittedDeviceAction && (
            <p>当前状态不允许此授权操作，请关闭后重新查看。</p>
          )}
          {phase === "starting" && <p role="status">正在创建独立授权…</p>}
          {phase === "waiting" && <p role="status">正在等待浏览器授权…</p>}
          {feedback && (
            <p role={phase === "failed" ? "alert" : "status"}>{feedback}</p>
          )}
          {phase !== "completed" && (
            <p className="text-sm text-muted-foreground">
              关闭或取消只停止本地等待，不代表撤销服务器授权。
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            {phase === "completed" ? (
              <Button type="button" onClick={close}>
                完成，返回账号列表
              </Button>
            ) : (
              <Button
                type="button"
                disabled={!canStart}
                onClick={() => void start()}
              >
                {rebind ? "确认更换并在浏览器中授权" : "在浏览器中授权"}
              </Button>
            )}
            {busy && (
              <Button type="button" variant="outline" onClick={cancel}>
                取消本次授权
              </Button>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
