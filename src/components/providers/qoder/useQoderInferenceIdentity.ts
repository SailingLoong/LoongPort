import { useQoderCommandAuthority } from "./useQoderCommandAuthority";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import {
  qoderHttpAuthApi,
  type QoderHttpIdentityAction,
  type QoderHttpIdentityContext,
  type QoderHttpIdentityFailureCode,
  type QoderHttpIdentityStatus,
} from "@/lib/api/qoderHttpAuth";

export function useQoderInferenceIdentity({
  providerId,
  sourceGeneration,
  disabled = false,
}: {
  providerId: string;
  sourceGeneration?: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const authority = useQoderCommandAuthority();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [declared, setDeclared] = useState(false);
  const [context, setContext] = useState<QoderHttpIdentityContext | null>(null);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{
    role: "status" | "alert";
    message: string;
  } | null>(null);
  const inFlight = useRef(false);
  const mounted = useRef(false);
  const epoch = useRef(0);

  const uncertainMessage = t("qoderHttpIdentity.uncertain", {
    defaultValue: "操作结果未确认，请重新打开查看状态。",
  });
  const statusMessages: Record<QoderHttpIdentityStatus, string> = {
    unavailable: t("qoderHttpIdentity.unavailable", {
      defaultValue:
        "当前 Qoder 推理身份不可用，请检查原配置与凭据保管状态后重试。",
    }),
    unestablished: t("qoderHttpIdentity.unestablished", {
      defaultValue: "尚未建立推理身份。",
    }),
    ready: t("qoderHttpIdentity.ready", {
      defaultValue: "已建立推理身份，可供后续请求复用。",
    }),
    refreshNeeded: t("qoderHttpIdentity.refreshNeeded", {
      defaultValue: "等待下一次请求自动刷新。",
    }),
    reestablishRequired: t("qoderHttpIdentity.reestablishRequired", {
      defaultValue: "请重新建立推理身份。",
    }),
    pending: t("qoderHttpIdentity.pending", {
      defaultValue:
        "已有身份操作正在处理或尚未完成。主动恢复需要一次系统确认，且不会重发旧刷新请求。",
    }),
    unknown: t("qoderHttpIdentity.unknown", {
      defaultValue:
        "上次操作结果尚未确认。主动恢复需要一次系统确认，且不会重发旧刷新请求。",
    }),
  };
  const actionLabels: Record<QoderHttpIdentityAction, string> = {
    establish: t("qoderHttpIdentity.establish", {
      defaultValue: "声明对应区域 PAT 并建立",
    }),
    recover: t("qoderHttpIdentity.recover", {
      defaultValue: "恢复推理身份",
    }),
    rebind: t("qoderHttpIdentity.rebind", {
      defaultValue: "重新绑定账号",
    }),
  };
  const failureMessages: Record<QoderHttpIdentityFailureCode, string> = {
    unavailable: statusMessages.unavailable,
    source_changed: t("qoderHttpIdentity.sourceChanged", {
      defaultValue: "供应商凭据已变化，请重新打开并确认当前来源。",
    }),
    owner_changed: t("qoderHttpIdentity.ownerChanged", {
      defaultValue: "凭据保管状态已变化，请重新打开查看状态。",
    }),
    invalid_source: t("qoderHttpIdentity.invalidSource", {
      defaultValue: "当前来源声明无法验证，请检查已保存的对应区域 Qoder PAT。",
    }),
    state_changed: t("qoderHttpIdentity.stateChanged", {
      defaultValue: "身份状态已变化，请重新打开查看状态。",
    }),
    identity_rebind_required: t("qoderHttpIdentity.rebindRequired", {
      defaultValue:
        "账号身份不一致，未自动接受新账号。请重新打开查看状态；更换账号需明确选择重新绑定。",
    }),
    outcome_unknown: uncertainMessage,
  };

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useLayoutEffect(() => {
    // Invalidate before paint when the same Provider receives a new source.
    const current = ++epoch.current;
    setDeclared(false);
    setContext(null);
    setFeedback(null);
    if (open && !disabled) {
      // Context may initialize a legacy source revision, but never exchanges.
      qoderHttpAuthApi
        .context(providerId)
        .then((value) => {
          if (epoch.current !== current) return;
          if (value.providerId === providerId) setContext(value);
          else setFeedback({ role: "alert", message: uncertainMessage });
        })
        .catch(() => {
          if (epoch.current === current)
            setFeedback({ role: "alert", message: uncertainMessage });
        });
    }
    return () => {
      ++epoch.current;
    };
  }, [open, providerId, sourceGeneration, disabled, uncertainMessage]);

  const visibleContext = context?.providerId === providerId ? context : null;
  const actions =
    visibleContext?.status !== "unavailable"
      ? (visibleContext?.allowedActions ?? [])
      : [];
  const canAct =
    !disabled &&
    !busy &&
    declared &&
    Boolean(visibleContext?.credentialGeneration) &&
    (visibleContext?.source === "internationalQoderPat" ||
      visibleContext?.source === "chinaQoderPat");

  async function perform(action: QoderHttpIdentityAction) {
    if (
      !authority.isCurrent() ||
      !open ||
      !canAct ||
      !visibleContext ||
      !actions.includes(action) ||
      inFlight.current
    )
      return;
    inFlight.current = true;
    setBusy(true);
    setFeedback(null);
    const current = epoch.current;
    try {
      // Recovery/rebind confirmation belongs to this one backend command.
      const value = await qoderHttpAuthApi.action(visibleContext, action);
      // The original owner may complete after this view closes or switches.
      // Re-read backend facts; never publish the old action context to another view.
      if (
        value.status === "completed" &&
        value.context.providerId === visibleContext.providerId
      ) {
        void queryClient.invalidateQueries({
          queryKey: ["applicationOverview", "codex"],
        });
        void queryClient.invalidateQueries({ queryKey: ["serviceSources"] });
      }
      if (epoch.current !== current) return;
      setDeclared(false);
      if (value.status === "completed" || value.status === "changed") {
        if (value.context.providerId !== providerId) {
          setContext(null);
          setFeedback({ role: "alert", message: uncertainMessage });
          return;
        }
        setContext(value.context);
        if (value.status === "changed")
          setFeedback({
            role: "status",
            message: t("qoderHttpIdentity.changed", {
              defaultValue: "状态已变化，请根据当前状态重新选择操作。",
            }),
          });
      } else if (value.status === "cancelled") {
        setFeedback({
          role: "status",
          message: t("qoderHttpIdentity.cancelled", {
            defaultValue: "已取消，未提交新的身份建立请求。",
          }),
        });
      } else if (value.status === "failed") {
        setContext(null);
        setFeedback({
          role: "alert",
          message: failureMessages[value.code] ?? uncertainMessage,
        });
      }
    } catch {
      if (epoch.current === current) {
        setDeclared(false);
        setContext(null);
        setFeedback({ role: "alert", message: uncertainMessage });
      }
    } finally {
      inFlight.current = false;
      // This ends local busy state only. Closing the view never cancels IPC.
      if (mounted.current) setBusy(false);
    }
  }

  function changeOpen(value: boolean) {
    authority.invalidate();
    ++epoch.current;
    setOpen(value);
  }
  return {
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
  };
}
