import { useQoderCommandAuthority } from "./useQoderCommandAuthority";
import { confirmsQoderAutomaticWrite } from "./useQoderAutomaticClaim";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { QoderAutomaticContext } from "@/lib/api/qoderDaily";
import { qoderHttpAuthApi } from "@/lib/api/qoderHttpAuth";
import {
  qoderCnCampaignsApi,
  type QoderCnCampaignDeclaration,
  type QoderCnCampaignFailure,
  type QoderCnCampaignReadResult,
  type QoderCnBenefitRunResult,
} from "@/lib/api/qoderCnCampaigns";

const changedDefault = "账号或凭据状态已变化，请重新打开查询。";
const unavailableDefault =
  "当前 CN 凭据来源不可用，请检查原账号配置与身份状态。";
const failures: Record<QoderCnCampaignFailure, string> = {
  invalid_source: unavailableDefault,
  source_changed: changedDefault,
  owner_changed: changedDefault,
  owner_unavailable: "凭据保管状态不可用，请检查保管库和原账号身份后重新查询。",
  identity_required: "请先使用此账号的推理身份或独立授权入口建立身份。",
  identity_pending:
    "原账号身份操作尚未完成，请先查看原身份入口；本查询不会重发或刷新。",
  identity_unknown:
    "原账号身份操作结果未知，请先在原身份入口恢复；本查询不会重发或刷新。",
  identity_expired:
    "账号身份已过期，请使用原账号身份流程恢复；本查询不会刷新凭据。",
  identity_mismatch: "响应身份不一致，未接受活动状态。请检查原账号身份。",
  authentication_required:
    "服务拒绝了当前授权，请在原账号身份入口检查或重新授权。",
  verification_required:
    "服务要求进一步验证或拒绝访问，已停止查询，不会绕过挑战。",
  rate_limited: "服务暂时限流，请稍后手动查询。",
  redirect_rejected: "服务返回了重定向，已拒绝转发凭据。",
  http_error: "活动服务请求失败，请稍后手动查询。",
  service_rejected: "活动服务未接受本次查询。",
  invalid_response: "活动响应无法安全识别，未推断签到或奖励状态。",
  response_too_large: "活动响应超过安全大小，已停止读取。",
  timeout: "活动查询超时，请稍后手动查询。",
  network_error: "活动服务连接失败，请检查网络后手动查询。",
};

export function useQoderCnCampaigns({
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
  const changed = t("qoderCnCampaigns.changed", {
    defaultValue: changedDefault,
  });
  const unavailable = t("qoderCnCampaigns.unavailable", {
    defaultValue: unavailableDefault,
  });
  const uncertain = t("qoderCnCampaigns.uncertain", {
    defaultValue: "活动查询未完成，请重新打开查看状态。",
  });
  const [open, setOpen] = useState(false);
  const [declaration, setDeclaration] =
    useState<QoderCnCampaignDeclaration | null>(null);
  const [result, setResult] = useState<Extract<
    QoderCnCampaignReadResult,
    { status: "observed" }
  > | null>(null);
  const [benefits, setBenefits] = useState<Extract<
    QoderCnBenefitRunResult,
    { status: "completed" }
  > | null>(null);
  const [policy, setPolicy] = useState<QoderAutomaticContext | null>(null);
  const [policyBusy, setPolicyBusy] = useState(false);
  const policyInFlight = useRef(false);
  // Only unresolved command receipts live here; backend remains the policy owner.
  const policyWrites = useRef(
    new Map<
      string,
      { expected: QoderAutomaticContext; confirmed?: QoderAutomaticContext }
    >(),
  );
  const policyUncertain = () => {
    const receipt = policyWrites.current.get(providerId);
    return !!receipt && !receipt.confirmed;
  };
  const policyUnknown = t("qoderCnCampaigns.policyUnknown", {
    defaultValue: "自动领取设置结果未确认，请重新打开查询状态；不会重发设置。",
  });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const epoch = useRef(0);
  const inFlight = useRef(false);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useLayoutEffect(() => {
    const current = ++epoch.current;
    setDeclaration(null);
    setResult(null);
    setBenefits(null);
    setPolicy(null);
    setError(null);
    if (open && !disabled) {
      // Local original-owner context only. No network refresh on view activation.
      qoderHttpAuthApi
        .context(providerId)
        .then((context) => {
          if (epoch.current !== current) return;
          if (
            context.providerId !== providerId ||
            (sourceGeneration &&
              context.credentialGeneration !== sourceGeneration)
          ) {
            setError(changed);
            return;
          }
          if (
            !context.credentialGeneration ||
            (context.source !== "chinaQoderPat" &&
              context.source !== "chinaQoderDevice")
          ) {
            setError(unavailable);
            return;
          }
          setDeclaration({
            providerId,
            credentialGeneration: context.credentialGeneration,
            source: context.source,
          });
          // Original local policy only: opening this dialog never enrolls or contacts Qoder.
          void qoderCnCampaignsApi
            .automaticContext(providerId)
            .then((value) => {
              if (epoch.current !== current) return;
              if (
                value.providerId !== providerId ||
                value.credentialGeneration !== context.credentialGeneration ||
                value.policyVersion !== 2
              ) {
                setError(changed);
                return;
              }
              const receipt = policyWrites.current.get(providerId);
              if (
                receipt &&
                confirmsQoderAutomaticWrite(value, receipt.expected) &&
                (!receipt.confirmed ||
                  value.revision >= receipt.confirmed.revision)
              )
                receipt.confirmed = value;
              // A read begun before a late command receipt cannot regress it.
              const confirmed = receipt?.confirmed;
              const accepted =
                confirmed &&
                value.credentialGeneration === confirmed.credentialGeneration &&
                value.policyVersion === confirmed.policyVersion &&
                value.revision < confirmed.revision
                  ? confirmed
                  : value;
              setPolicy(accepted);
              if (policyUncertain()) setError(policyUnknown);
            })
            .catch(() => {
              if (epoch.current === current) setError(unavailable);
            });
        })
        .catch(() => {
          if (epoch.current === current) setError(unavailable);
        });
    }
    return () => {
      ++epoch.current;
    };
  }, [
    open,
    providerId,
    sourceGeneration,
    disabled,
    changed,
    unavailable,
    policyUnknown,
  ]);

  async function read() {
    if (
      !authority.isCurrent() ||
      !open ||
      !declaration ||
      disabled ||
      inFlight.current
    )
      return;
    const current = epoch.current;
    inFlight.current = true;
    setBusy(true);
    setResult(null);
    setBenefits(null);
    setError(null);
    try {
      const value = await qoderCnCampaignsApi.read(declaration);
      if (epoch.current !== current) return;
      if (value.status === "failed") {
        setError(
          t(`qoderCnCampaigns.errors.${value.code}`, {
            defaultValue: failures[value.code] ?? uncertain,
          }),
        );
        return;
      }
      if (
        value.providerId !== declaration.providerId ||
        value.credentialGeneration !== declaration.credentialGeneration ||
        value.source !== declaration.source
      ) {
        setError(changed);
        return;
      }
      setResult(value);
    } catch {
      if (epoch.current === current) setError(uncertain);
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(false);
    }
  }
  async function claim() {
    if (
      !authority.isCurrent() ||
      !open ||
      !declaration ||
      disabled ||
      inFlight.current
    )
      return;
    const current = epoch.current;
    inFlight.current = true;
    setBusy(true);
    setBenefits(null);
    setResult(null);
    setError(null);
    try {
      const value = await qoderCnCampaignsApi.claim(declaration);
      if (epoch.current !== current) return;
      if (value.status === "failed") {
        setError(
          t(`qoderCnCampaigns.errors.${value.code}`, {
            defaultValue: failures[value.code] ?? uncertain,
          }),
        );
      } else if (
        value.providerId !== declaration.providerId ||
        value.credentialGeneration !== declaration.credentialGeneration
      ) {
        setError(changed);
      } else {
        setBenefits(value);
      }
    } catch {
      if (epoch.current === current)
        setError(
          t("qoderCnCampaigns.claimUnknown", {
            defaultValue:
              "领取结果未确认。下次操作只会查询未确认活动，不会重复发送领取。",
          }),
        );
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(false);
    }
  }
  async function setAutomatic() {
    if (
      !authority.isCurrent() ||
      !open ||
      !policy ||
      !declaration ||
      disabled ||
      policyInFlight.current ||
      policyUncertain() ||
      (!policy.enabled && !policy.canEnable)
    )
      return;
    const current = epoch.current;
    policyInFlight.current = true;
    policyWrites.current.set(providerId, { expected: policy });
    setPolicyBusy(true);
    setError(null);
    try {
      const value = await qoderCnCampaignsApi.setAutomatic(
        policy,
        !policy.enabled,
        declaration.source,
      );
      // A late receipt can resolve its original write, never revive the old UI.
      const confirmed = confirmsQoderAutomaticWrite(value, policy);
      const receipt = policyWrites.current.get(providerId);
      if (
        confirmed &&
        receipt?.expected === policy &&
        (!receipt.confirmed || value.revision >= receipt.confirmed.revision)
      )
        receipt.confirmed = value;
      if (epoch.current !== current) return;
      if (
        !confirmed ||
        value.providerId !== declaration.providerId ||
        value.credentialGeneration !== declaration.credentialGeneration ||
        value.policyVersion !== 2
      ) {
        setPolicy(null);
        setError(changed);
      } else {
        ++epoch.current;
        setPolicy(value);
        setBenefits(null);
        setResult(null);
      }
    } catch {
      if (epoch.current === current) {
        ++epoch.current;
        setPolicy(null);
        setBenefits(null);
        setResult(null);
        setError(policyUnknown);
      }
    } finally {
      policyInFlight.current = false;
      if (mounted.current) setPolicyBusy(false);
    }
  }
  function changeOpen(value: boolean) {
    authority.invalidate();
    ++epoch.current;
    setOpen(value);
  }
  const canRead = !!declaration && !disabled && !busy;
  const canSetAutomatic =
    !!policy &&
    !!declaration &&
    !disabled &&
    !policyBusy &&
    !policyUncertain() &&
    (policy.enabled || policy.canEnable);
  return {
    open,
    changeOpen,
    declaration,
    result,
    benefits,
    policy,
    error,
    busy,
    canRead,
    canSetAutomatic,
    read,
    claim,
    setAutomatic,
  };
}
