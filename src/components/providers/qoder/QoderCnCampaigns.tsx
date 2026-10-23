import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
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
const benefitReasons: Record<string, string> = {
  invalid_activity: "活动信息不完整或无法安全识别。",
  identity_mismatch: "服务返回的账号身份不一致。",
  unsupported_action: "此活动不是直接领取福利。",
  unavailable: "服务提示此活动当前不可用。",
  achievement_required: "需先完成额外任务，本应用不会自动完成。",
  additional_condition:
    "存在额外条件，本应用不会购买、订阅、扣积分或代为同意。",
  ambiguous_campaign: "同一活动返回了互相冲突的信息。",
  not_claimable: "此活动当前不是可领取状态。",
  campaign_not_started: "活动尚未开始。",
  campaign_ended: "活动已经结束。",
  clock_unverified: "当前时间无法确认，未发送领取。",
  previous_unknown: "已有未确认请求，只查询原活动，不会重复领取。",
  prior_owner_changed:
    "原账号或凭据来源已改变，旧活动记录仅供查询，未自动重新领取。",
  same_campaign_no_new_window: "尚未观察到新的可领取状态，未重复领取。",
  reconciled_no_new_round:
    "先前领取曾结果未知，虽已查到已领取，但缺少明确新活动区间，不能自动重新领取。",
  reconciliation_mismatch: "活动信息已改变，不能确认先前请求的结果。",
  claim_outcome_unknown: "领取结果未确认，只查询恢复，不会重发。",
};
const labels = {
  claimable: "可领取（活动状态）",
  claimed: "已领取（活动状态）",
  unknown: "活动状态未知",
};

/** View activation is local only; claims and continuing consent require explicit actions. */
export function QoderCnCampaigns({
  providerId,
  sourceGeneration,
  disabled = false,
}: {
  providerId: string;
  sourceGeneration?: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const title = t("qoderCnCampaigns.title", { defaultValue: "CN 活动状态" });
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
              setPolicy(value);
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
  }, [open, providerId, sourceGeneration, disabled, changed, unavailable]);

  async function read() {
    if (!declaration || disabled || inFlight.current) return;
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
    if (!declaration || disabled || inFlight.current) return;
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
    if (!policy || !declaration || disabled || policyInFlight.current) return;
    const current = epoch.current;
    policyInFlight.current = true;
    setPolicyBusy(true);
    setError(null);
    try {
      const value = await qoderCnCampaignsApi.setAutomatic(
        policy,
        !policy.enabled,
        declaration.source,
      );
      if (epoch.current !== current) return;
      if (
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
        setError(
          t("qoderCnCampaigns.policyUnknown", {
            defaultValue:
              "自动领取设置结果未确认，请重新打开查询状态；不会重发设置。",
          }),
        );
      }
    } finally {
      policyInFlight.current = false;
      if (mounted.current) setPolicyBusy(false);
    }
  }
  return (
    <Dialog
      open={open && !disabled}
      onOpenChange={(value) => {
        ++epoch.current;
        setOpen(value);
      }}
    >
      <DialogTrigger asChild>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={disabled}
          onClick={(event) => event.stopPropagation()}
        >
          {title}
        </Button>
      </DialogTrigger>
      <DialogContent onClick={(event) => event.stopPropagation()}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            {t("qoderCnCampaigns.description", {
              defaultValue:
                "可查询或领取此账号当前可领取的免费福利。不会购买、订阅、扣积分或完成额外任务；遇到额外条件会跳过。未读取或提交官方客户端运行时身份，服务要求验证时会停止。",
            })}
          </DialogDescription>
        </DialogHeader>
        <p className="text-sm text-muted-foreground">
          {t("qoderCnCampaigns.boundary", {
            defaultValue:
              "自动领取默认关闭，需为此账号单独开启。应用运行且保管库可用时定期检查；关闭后不再派发后续领取，但不能撤回已经发送的请求。活动“已领取”不代表今日签到成功或本次新增积分。",
          })}
        </p>
        <Button
          type="button"
          disabled={!declaration || disabled || busy}
          onClick={() => void read()}
        >
          {busy
            ? t("qoderCnCampaigns.loading", { defaultValue: "正在处理…" })
            : t("qoderCnCampaigns.query", { defaultValue: "查询活动状态" })}
        </Button>
        <Button
          type="button"
          disabled={!declaration || disabled || busy}
          onClick={() => void claim()}
        >
          {t("qoderCnCampaigns.claim", { defaultValue: "领取免费福利" })}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={
            !policy ||
            !declaration ||
            disabled ||
            policyBusy ||
            (!policy.enabled && !policy.canEnable)
          }
          onClick={() => void setAutomatic()}
        >
          {policy?.enabled
            ? t("qoderCnCampaigns.disableAutomatic", {
                defaultValue: "关闭自动领取免费福利",
              })
            : t("qoderCnCampaigns.enableAutomatic", {
                defaultValue: "开启自动领取免费福利",
              })}
        </Button>
        {policy && !policy.enabled && !policy.canEnable && (
          <p role="status">
            {t("qoderCnCampaigns.policyUnavailable", {
              defaultValue:
                "当前无法开启自动领取，请先检查原账号身份和凭据保管状态。",
            })}
          </p>
        )}
        {busy && (
          <p role="status">
            {t("qoderCnCampaigns.inFlight", {
              defaultValue:
                "操作已提交；关闭窗口不会撤回已经发送的请求。未确认领取只查询恢复，不会重发。",
            })}
          </p>
        )}
        {error && <p role="alert">{error}</p>}
        {benefits &&
          (benefits.activities.length === 0 ? (
            <p role="status">
              {t("qoderCnCampaigns.noBenefits", {
                defaultValue: "当前没有可处理的免费福利。",
              })}
            </p>
          ) : (
            <ul
              className="max-h-64 space-y-2 overflow-auto"
              aria-label={t("qoderCnCampaigns.claimResults", {
                defaultValue: "免费福利领取结果",
              })}
            >
              {benefits.activities.map((activity) => (
                <li key={activity.campaignId} className="rounded border p-2">
                  <p className="break-all">{activity.campaignId}</p>
                  <p>
                    {t(`qoderCnCampaigns.outcomes.${activity.outcome}`, {
                      defaultValue: {
                        claimedDeltaUnknown: "已确认领取状态，本次新增数量未知",
                        observedClaimed: "活动显示已领取，未重复发送领取",
                        unknown: "结果未确认，仅查询恢复，不会重复领取",
                        skipped: "已跳过：活动不满足领取条件或含额外要求",
                      }[activity.outcome],
                    })}
                  </p>
                  {activity.reason && (
                    <p className="text-sm text-muted-foreground">
                      {t(`qoderCnCampaigns.reasons.${activity.reason}`, {
                        defaultValue:
                          benefitReasons[activity.reason] ??
                          "活动条件或结果尚未确认，未追加操作。",
                      })}
                    </p>
                  )}
                </li>
              ))}
            </ul>
          ))}
        {result &&
          (result.campaigns.length === 0 ? (
            <p role="status">
              {t("qoderCnCampaigns.empty", {
                defaultValue:
                  "未返回可展示的福利活动；不能据此推断今日签到或资格。",
              })}
            </p>
          ) : (
            <ul
              className="max-h-64 space-y-2 overflow-auto"
              aria-label={t("qoderCnCampaigns.list", {
                defaultValue: "通用福利活动状态",
              })}
            >
              {result.campaigns.map((campaign) => (
                <li key={campaign.campaignId} className="rounded border p-2">
                  <p className="break-all">
                    {campaign.campaignKey ?? campaign.campaignId}
                  </p>
                  <p>
                    {t(`qoderCnCampaigns.status.${campaign.claimStatus}`, {
                      defaultValue: labels[campaign.claimStatus],
                    })}
                  </p>
                </li>
              ))}
            </ul>
          ))}
      </DialogContent>
    </Dialog>
  );
}
