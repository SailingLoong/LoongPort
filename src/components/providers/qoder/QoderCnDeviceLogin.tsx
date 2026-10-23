import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { usePreservedViewActive } from "@/components/ui/PreservedView";
import { settingsApi } from "@/lib/api/settings";
import {
  qoderHttpAuthApi,
  type QoderHttpIdentityContext,
} from "@/lib/api/qoderHttpAuth";
import { qoderCnDeviceLoginApi } from "@/lib/api/qoderCnDeviceLogin";

type Phase =
  "idle" | "starting" | "waiting" | "completed" | "failed" | "cancelled";
interface Attempt {
  abandoned: boolean;
  cancelRequested: boolean;
  sessionId?: string;
  pollTimer?: ReturnType<typeof setTimeout>;
  expiryTimer?: ReturnType<typeof setTimeout>;
}
const uncertainMessage =
  "授权结果未确认，请重新查看账号状态后主动重试。不会自动重发授权请求。";
function failureMessage(code: string) {
  switch (code) {
    case "expired":
    case "session_expired":
      return "本次授权已过期，请主动重新发起。";
    case "identity_rebind_required":
      return "账号身份不一致，未更换原账号。若要更换，请明确选择更换绑定账号。";
    case "source_changed":
    case "owner_changed":
      return "账号凭据已变化，请关闭窗口后重新查看。";
    case "unavailable":
      return "当前独立授权不可用，请检查账号与凭据保管状态。";
    default:
      return uncertainMessage;
  }
}
function isOfficialGrantUrl(value: string) {
  try {
    const url = new URL(value);
    return (
      url.origin === "https://qoder.cn" &&
      url.pathname === "/device/selectAccounts" &&
      !url.username &&
      !url.password &&
      !url.hash
    );
  } catch {
    return false;
  }
}

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
  const active = usePreservedViewActive();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [rebind, setRebind] = useState(false);
  const [context, setContext] = useState<QoderHttpIdentityContext | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [feedback, setFeedback] = useState<string | null>(null);
  const current = useRef<Attempt | null>(null);
  const available = useRef(false);
  const contextEpoch = useRef(0);

  const abandon = useCallback((attempt: Attempt | null) => {
    if (!attempt) return;
    attempt.abandoned = true;
    clearTimeout(attempt.pollTimer);
    clearTimeout(attempt.expiryTimer);
    if (current.current === attempt) current.current = null;
    if (attempt.sessionId && !attempt.cancelRequested) {
      attempt.cancelRequested = true;
      // Local cancellation is best effort, not a claim of server revocation.
      void qoderCnDeviceLoginApi.cancel(attempt.sessionId).catch(() => {});
    }
  }, []);

  useLayoutEffect(() => {
    available.current = active && !disabled;
    setOpen(false);
    setContext(null);
    setPhase("idle");
    setFeedback(null);
    return () => {
      available.current = false;
      ++contextEpoch.current;
      abandon(current.current);
    };
  }, [active, disabled, providerId, sourceGeneration, abandon]);

  useLayoutEffect(() => {
    const epoch = ++contextEpoch.current;
    setContext(null);
    if (open && available.current && providerId) {
      void queryClient
        .fetchQuery({
          queryKey: ["qoderHttpIdentity", providerId, sourceGeneration],
          queryFn: () => qoderHttpAuthApi.context(providerId),
          staleTime: 0,
          retry: false,
        })
        .then((value) => {
          if (contextEpoch.current !== epoch || !available.current) return;
          if (value.providerId === providerId) setContext(value);
          else {
            setPhase("failed");
            setFeedback(uncertainMessage);
          }
        })
        .catch(() => {
          if (contextEpoch.current === epoch && available.current) {
            setPhase("failed");
            setFeedback(uncertainMessage);
          }
        });
    }
    return () => {
      ++contextEpoch.current;
    };
  }, [open, rebind, providerId, sourceGeneration, queryClient]);

  const busy = phase === "starting" || phase === "waiting";
  // No status-to-permission inference here: the backend supplies permitted actions.
  const canStart =
    active &&
    !disabled &&
    !busy &&
    phase !== "completed" &&
    (!providerId ||
      (context?.providerId === providerId &&
        context.source === "chinaQoderDevice" &&
        Boolean(context.credentialGeneration) &&
        context.allowedDeviceActions?.includes(rebind ? "rebind" : "login")));
  const isCurrent = (attempt: Attempt) =>
    available.current && !attempt.abandoned && current.current === attempt;

  async function start() {
    if (!canStart || current.current) return;
    const attempt: Attempt = { abandoned: false, cancelRequested: false };
    current.current = attempt;
    setPhase("starting");
    setFeedback(null);
    const fail = (message: string) => {
      if (!isCurrent(attempt)) return;
      abandon(attempt);
      setPhase("failed");
      setFeedback(message);
    };
    try {
      const result = await qoderCnDeviceLoginApi
        .start({
          providerId: providerId ?? null,
          credentialGeneration: providerId
            ? context!.credentialGeneration
            : null,
          rebind,
        })
        .finally(() => {
          // Start may already have saved a recoverable Provider, even on failure.
          // Refresh data after a late response too, without reviving its old view.
          void queryClient.invalidateQueries({
            queryKey: ["providers", "codex"],
          });
        });
      if (result.status === "failed") {
        fail(failureMessage(result.code));
        return;
      }
      attempt.sessionId = result.sessionId;
      if (!isCurrent(attempt)) {
        abandon(attempt);
        return;
      }
      if (
        !result.sessionId ||
        !result.providerId ||
        !result.credentialGeneration ||
        (providerId &&
          (result.providerId !== providerId ||
            result.credentialGeneration !== context!.credentialGeneration)) ||
        !isOfficialGrantUrl(result.authUrl) ||
        !Number.isFinite(result.expiresAtMs) ||
        !Number.isFinite(result.pollIntervalMs) ||
        result.pollIntervalMs <= 0 ||
        result.pollIntervalMs > 60_000
      ) {
        fail(uncertainMessage);
        return;
      }
      const remaining = result.expiresAtMs - Date.now();
      if (remaining <= 0) {
        fail(failureMessage("expired"));
        return;
      }
      // Independent deadline also expires a hung browser opener or in-flight poll.
      attempt.expiryTimer = setTimeout(
        () => fail(failureMessage("expired")),
        Math.min(remaining, 2_147_483_647),
      );
      await settingsApi.openExternal(result.authUrl);
      if (!isCurrent(attempt)) return;
      setPhase("waiting");
      const poll = async () => {
        if (!isCurrent(attempt)) return;
        if (Date.now() >= result.expiresAtMs) {
          fail(failureMessage("expired"));
          return;
        }
        try {
          const status = await qoderCnDeviceLoginApi.poll(result.sessionId);
          if (!isCurrent(attempt)) return;
          if (Date.now() >= result.expiresAtMs) {
            fail(failureMessage("expired"));
            return;
          }
          if (status.status === "pending") {
            attempt.pollTimer = setTimeout(
              () => void poll(),
              result.pollIntervalMs,
            );
          } else if (status.status === "failed") {
            fail(failureMessage(status.code));
          } else if (status.providerId !== result.providerId) {
            fail(uncertainMessage);
          } else {
            clearTimeout(attempt.expiryTimer);
            current.current = null;
            setPhase("completed");
            setFeedback(
              "授权已保存，不代表真实推理已验证。请通过真实请求验证权益。",
            );
            void queryClient.invalidateQueries({
              queryKey: ["providers", "codex"],
            });
            void queryClient.invalidateQueries({
              queryKey: ["qoderHttpIdentity", status.providerId],
            });
          }
        } catch {
          fail(uncertainMessage);
        }
      };
      attempt.pollTimer = setTimeout(() => void poll(), result.pollIntervalMs);
    } catch {
      fail(uncertainMessage);
    }
  }
  function openFor(changeAccount: boolean) {
    setRebind(changeAccount);
    setPhase("idle");
    setFeedback(null);
    setOpen(true);
  }
  function close() {
    ++contextEpoch.current;
    abandon(current.current);
    setOpen(false);
    setPhase("idle");
    setFeedback(null);
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(value) => {
        if (!value) close();
      }}
    >
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
          {context &&
            !context.allowedDeviceActions?.includes(
              rebind ? "rebind" : "login",
            ) && <p>当前状态不允许此授权操作，请关闭后重新查看。</p>}
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
              <Button
                type="button"
                variant="outline"
                onClick={() => {
                  abandon(current.current);
                  setPhase("cancelled");
                  setFeedback("已停止本地授权等待，不代表撤销服务器授权。");
                }}
              >
                取消本次授权
              </Button>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
