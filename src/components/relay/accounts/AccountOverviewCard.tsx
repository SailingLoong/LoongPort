import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { automaticClaimSetting } from "./automaticClaimSettings";
import type { PlatformAccountSummary } from "./useAccountPlatformPresence";

/** Display only the original owner's snapshots. Claim receipts never imply inference health. */
export function AccountOverviewCard({
  account,
  unavailable = false,
  disabled = false,
  automaticUnconfirmed = false,
  onOpen,
}: {
  account: PlatformAccountSummary;
  unavailable?: boolean;
  disabled?: boolean;
  automaticUnconfirmed?: boolean;
  onOpen: () => void;
}) {
  const { t, i18n } = useTranslation();
  const unknown = t("common.unknown", { defaultValue: "未知" });
  const unreported = t("loongport.accounts.unreported", {
    defaultValue: "未提供",
  });
  const automatic = (enabled: boolean | undefined) =>
    enabled === undefined
      ? unreported
      : enabled
        ? t("loongport.accounts.automaticEnabled", { defaultValue: "开启" })
        : t("claimSettings.off", { defaultValue: "自动领取未开启" });
  let authorization = unknown;
  let quota = unknown;
  let autoClaim = unreported;
  let inference = t("loongport.accounts.inferenceUnverified", {
    defaultValue: "未验证",
  });
  const snapshot = account.snapshot;
  const expiryAt =
    snapshot?.kind === "workbuddy" &&
    snapshot.value.creditExpiryStatus === "known"
      ? snapshot.value.nextKnownCreditExpiry?.expiresAtMs
      : undefined;
  const [clockTick, setClockTick] = useState(0);
  useEffect(() => {
    if (
      unavailable ||
      typeof expiryAt !== "number" ||
      !Number.isFinite(expiryAt)
    )
      return;
    const delay = expiryAt - Date.now();
    if (delay <= 0) return;
    // Presentation only: invalidate the displayed deadline, never claim or refresh.
    const timer = setTimeout(
      () => setClockTick((tick) => tick + 1),
      Math.min(delay + 1, 2_147_483_647),
    );
    return () => clearTimeout(timer);
  }, [account.id, expiryAt, unavailable, clockTick]);
  const rewardText = (key: string, defaultValue: string) =>
    t(`accountRewardGlance.${key}`, { defaultValue });
  const amountText = (value: unknown, unit: unknown) => {
    if (typeof value !== "number" || !Number.isFinite(value) || value < 0)
      return rewardText("amountUnknown", "额度未提供");
    const label =
      typeof unit === "string" && unit.trim()
        ? unit === "points"
          ? rewardText("points", "积分")
          : unit === "workbuddy:credits"
            ? rewardText("workbuddyCredits", "积分")
            : unit
        : rewardText("unitUnknown", "单位未提供");
    return `${value} ${label}`;
  };
  const dateText = (value: unknown, future: boolean) => {
    if (typeof value !== "number" || !Number.isFinite(value) || value <= 0)
      return null;
    const date = new Date(value);
    if (
      !Number.isFinite(date.getTime()) ||
      (future ? value <= Date.now() : value > Date.now())
    )
      return null;
    return new Intl.DateTimeFormat(i18n.resolvedLanguage || i18n.language, {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      timeZoneName: "short",
    }).format(date);
  };
  const creditedText = (value: unknown, unit: unknown) => {
    const prefix =
      typeof value === "number" && Number.isFinite(value) && value >= 0
        ? "+"
        : "";
    return `${prefix}${amountText(value, unit)}`;
  };
  let lastSuccess = rewardText("noSuccess", "暂无可核实成功领取记录");
  let nextExpiry = rewardText("noExpiry", "到期时间未提供");
  if (snapshot?.kind === "workbuddy") {
    const {
      lastSuccessfulClaim,
      claimState,
      credited,
      creditExpiryStatus,
      nextKnownCreditExpiry,
    } = snapshot.value;
    if (lastSuccessfulClaim) {
      const date =
        lastSuccessfulClaim.timeSource === "localReconciled"
          ? dateText(lastSuccessfulClaim.confirmedAtMs, false)
          : null;
      const time = date
        ? `${rewardText("confirmationTime", "本地确认时间")} ${date}`
        : rewardText("timeUnknown", "时间未记录");
      lastSuccess = `${time} · ${creditedText(lastSuccessfulClaim.credited, lastSuccessfulClaim.unit)}`;
    } else if (claimState === "claimed") {
      lastSuccess = `${rewardText("timeUnknown", "时间未记录")} · ${creditedText(credited, null)}`;
    }
    if (creditExpiryStatus === "stale")
      nextExpiry = rewardText("expiryPending", "到期数据待更新");
    if (creditExpiryStatus === "known") {
      const expiry = nextKnownCreditExpiry;
      const time = dateText(expiry?.expiresAtMs, true);
      if (
        expiry &&
        time &&
        (expiry.remaining == null ||
          (typeof expiry.remaining === "number" &&
            Number.isFinite(expiry.remaining) &&
            expiry.remaining > 0))
      )
        nextExpiry = `${time} · ${t("accountRewardGlance.remaining", { defaultValue: "剩余 {{amount}}", amount: amountText(expiry.remaining, expiry.unit) })}`;
      else nextExpiry = rewardText("expiryPending", "到期数据待更新");
    }
  } else if (
    snapshot?.kind === "minimax" &&
    snapshot.value.operation?.status === "claimed"
  ) {
    lastSuccess = `${rewardText("timeUnknown", "时间未记录")} · ${creditedText(snapshot.value.operation.creditAmount, null)}`;
  }
  if (snapshot?.kind === "auth") {
    authorization =
      snapshot.value.requires_reauth || snapshot.value.reauth_required
        ? t("loongport.accounts.reauthorizationNeeded", {
            defaultValue: "需要重新授权",
          })
        : t("loongport.accounts.saved", { defaultValue: "已保存" });
  } else if (snapshot?.kind === "workbuddy") {
    const remaining = snapshot.value.credits?.totalRemaining;
    if (remaining !== null && remaining !== undefined)
      quota = String(remaining);
    autoClaim = automatic(snapshot.value.autoClaimEnabled);
  } else if (snapshot?.kind === "minimax") {
    authorization =
      snapshot.value.authorized === true
        ? t("loongport.accounts.authorized", { defaultValue: "已授权" })
        : snapshot.value.authorized === false
          ? t("loongport.accounts.notAuthorized", { defaultValue: "未授权" })
          : unknown;
    const entitlement = snapshot.value.entitlement;
    if (entitlement?.remainingCredits != null)
      quota = entitlement.remainingCredits;
    else if (entitlement?.quota?.fiveHour.remainingPercent != null)
      quota = `${entitlement.quota.fiveHour.remainingPercent}% (5h)`;
    autoClaim = automatic(snapshot.value.autoClaimEnabled);
  } else if (snapshot?.kind === "trae") {
    authorization =
      snapshot.value.status === "stored"
        ? t("loongport.accounts.saved", { defaultValue: "已保存" })
        : unknown;
    autoClaim = automatic(snapshot.value.autoCheckinEnabled);
  }
  const policy = automaticClaimSetting(account);
  if (snapshot?.kind === "qoder" || snapshot?.kind === "zcode")
    autoClaim = automatic(policy.enabled);
  const claimIssue = automaticUnconfirmed
    ? t("claimSettings.uncertain", {
        defaultValue: "设置结果未确认，请刷新后查看",
      })
    : policy.reason
      ? t(policy.reason)
      : null;
  if (automaticUnconfirmed)
    autoClaim = t("common.unknown", { defaultValue: "未知" });
  if (account.inferenceState) {
    const states = {
      paused: t("workbuddy.inference.state.paused", {
        defaultValue: "WorkBuddy 模型请求已暂停；账号管理和国内签到仍可用",
      }),
      ready: t("loongport.accounts.inferenceCredentialsReady", {
        defaultValue: "凭据就绪",
      }),
      needsAuthorization: t("loongport.accounts.inferenceNeedsAuthorization", {
        defaultValue: "待授权",
      }),
      refreshRequired: t("loongport.accounts.inferenceRefreshRequired", {
        defaultValue: "需刷新凭据",
      }),
      expired: t("loongport.accounts.inferenceExpired", {
        defaultValue: "凭据已过期",
      }),
    };
    inference = states[account.inferenceState];
  }
  const facts = [
    [
      t("loongport.accounts.authorization", { defaultValue: "授权" }),
      authorization,
    ],
    [
      t("loongport.accounts.quotaCredits", { defaultValue: "额度 / 积分" }),
      quota,
    ],
    [
      t("loongport.accounts.automaticClaim", { defaultValue: "自动领取" }),
      autoClaim,
    ],
    [
      t("loongport.accounts.inferenceUsage", { defaultValue: "推理消费" }),
      inference,
    ],
  ];
  return (
    <button
      type="button"
      aria-label={t("loongport.accounts.viewAccount", {
        defaultValue: "查看 {{name}}",
        name: account.label,
      })}
      disabled={disabled}
      onClick={onOpen}
      className="block w-full min-w-0 rounded-md border border-border bg-card p-4 text-left transition-colors hover:border-primary/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60"
    >
      <span className="mb-3 flex min-w-0 items-center justify-between gap-3">
        <span
          className="min-w-0 truncate text-sm font-semibold"
          title={account.label}
        >
          {account.label}
        </span>
        {account.identity && (
          <span className="shrink-0 rounded bg-muted px-2 py-0.5 text-xs text-muted-foreground">
            {account.identity}
          </span>
        )}
      </span>
      <span className="grid grid-cols-2 gap-x-3 gap-y-3 sm:grid-cols-4">
        {facts.map(([label, value]) => (
          <span key={label} className="min-w-0 text-xs leading-5">
            <span className="block text-muted-foreground">{label}</span>
            <span className="block break-words">
              {unavailable ? unknown : value}
            </span>
          </span>
        ))}
      </span>
      {claimIssue && (
        <span className="mt-1 block break-words text-xs text-muted-foreground">
          {unavailable ? unknown : claimIssue}
        </span>
      )}
      <span className="mt-4 grid gap-2 border-t border-border pt-3 text-xs leading-5">
        {[
          [rewardText("lastSuccess", "最近成功"), lastSuccess],
          [rewardText("nextExpiry", "即将到期"), nextExpiry],
        ].map(([label, value]) => (
          <span
            key={label}
            className="grid grid-cols-[5rem_minmax(0,1fr)] gap-2"
          >
            <span className="text-muted-foreground">{label}</span>
            <span className="min-w-0 break-words">
              {unavailable ? rewardText("pending", "数据待更新") : value}
            </span>
          </span>
        ))}
      </span>
    </button>
  );
}
