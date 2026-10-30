import { useQoderCommandAuthority } from "./useQoderCommandAuthority";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  qoderDailyApi,
  type QoderClaimContext,
  type QoderClaimResult,
} from "@/lib/api/qoderDaily";

export function useQoderDailyClaim(
  providerId: string,
  disabled = false,
  sourceGeneration?: string,
) {
  const { t } = useTranslation();
  const authority = useQoderCommandAuthority();
  const [open, setOpen] = useState(false);
  const [declared, setDeclared] = useState(false);
  const [context, setContext] = useState<QoderClaimContext | null>(null);
  const [result, setResult] = useState<QoderClaimResult | null>(null);
  const [error, setError] = useState(false);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const epoch = useRef(0);
  useEffect(() => {
    const current = ++epoch.current;
    setDeclared(false);
    setContext(null);
    setResult(null);
    setError(false);
    if (open && !disabled) {
      qoderDailyApi
        .context(providerId)
        .then((value) => {
          if (epoch.current === current && value.providerId === providerId)
            setContext(value);
        })
        .catch(() => {
          if (epoch.current === current) setError(true);
        });
    }
    return () => {
      ++epoch.current;
    };
  }, [open, providerId, sourceGeneration, disabled]);
  const unavailable = context?.deviceDependency ?? null;
  const ready =
    !disabled &&
    !busy &&
    declared &&
    context?.providerId === providerId &&
    Boolean(context?.credentialGeneration) &&
    context?.deviceDependency === null;
  async function claim() {
    if (
      !authority.isCurrent() ||
      !open ||
      !ready ||
      !context ||
      inFlight.current
    )
      return;
    inFlight.current = true;
    setBusy(true);
    setError(false);
    setResult(null);
    const current = epoch.current;
    try {
      const value = await qoderDailyApi.claim(context);
      if (epoch.current === current) setResult(value);
    } catch {
      if (epoch.current === current) setError(true);
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }
  const message =
    result &&
    ("Unavailable" in result
      ? result.Unavailable === "AuthFamilyUnverified"
        ? t("qoderDaily.changed", {
            defaultValue: "凭据或声明已变化，请关闭后重新确认。",
          })
        : t("qoderDaily.unavailable", {
            defaultValue:
              "当前缺少领取依赖或没有唯一可领取的 100 Credits 活动。",
          })
      : result.Observed.outcome === "NewlyClaimed" &&
          result.Observed.added_credits === 100
        ? t("qoderDaily.added", {
            defaultValue: "已确认本次新增 100 Credits。",
          })
        : result.Observed.outcome === "AlreadyClaimed"
          ? t("qoderDaily.replayed", {
              defaultValue: "该活动已领取，本次新增 0 Credits。",
            })
          : result.Observed.outcome === "ClaimedDeltaUnknown"
            ? t("qoderDaily.deltaUnknown", {
                defaultValue: "已确认领取记录，本次新增额度未确认。",
              })
            : t("qoderDaily.unknown", {
                defaultValue:
                  "领取结果尚未确认。再次点击只查询原记录，不重新提交。",
              }));
  function changeOpen(value: boolean) {
    authority.invalidate();
    ++epoch.current;
    setOpen(value);
  }
  return {
    open,
    setOpen: changeOpen,
    declared,
    setDeclared,
    context,
    result,
    error,
    busy,
    unavailable,
    ready,
    claim,
    message,
  };
}
