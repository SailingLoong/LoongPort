import type { TFunction } from "i18next";
import type { WorkBuddyNativeRowIdentity } from "./workbuddyNativeRowIdentity";
import type { PlatformAccountSummary } from "./useAccountPlatformPresence";
import type { AccountPlatform } from "./accountPlatforms";
import type { AccountOverviewSort } from "./accountOverviewPreferences";

export interface AccountOverviewProjectionContext {
  nowMs: number;
  locale: string;
  t: TFunction;
  freshness: "fresh" | "stale" | "unknown";
  automaticUnconfirmed?: boolean;
  nativeIdentity?: WorkBuddyNativeRowIdentity;
  nativeSwitchSupported?: boolean;
  automaticPolicy?: { enabled: boolean | undefined; reason: string | null };
}
export interface AccountRowVM {
  rowKey: string;
  nativeIdentity: WorkBuddyNativeRowIdentity;
  groupId: AccountPlatform;
  accountId: string;
  name: string;
  identity?: string;
  facts: { label: string; value: string }[];
  lastSuccess: string;
  nextExpiry: string;
  earliestKnownExpiryMs: number | null;
  balance: number | null;
  availability: "available" | "exhausted" | "unknown";
  claimIssue: string | null;
  detailTarget: { platform: AccountPlatform; id: string };
  switchAction: { enabled: boolean; label: string; reason: string };
}
const number = (value: unknown): number | null =>
  typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : null;
const creditNumber = (value: unknown): number | null => {
  if (typeof value !== "string" || !/^\d+(?:\.\d+)?$/.test(value.trim()))
    return null;
  return number(Number(value));
};
type Right = number | null | "unlimited";
/** Remaining-right facts only; never inference health or native admission. */
function remainingRights(
  account: PlatformAccountSummary,
  nowMs: number,
): Right[] {
  const snapshot = account.snapshot;
  if (snapshot?.kind === "workbuddy") {
    const value = snapshot.value;
    const rights: Right[] = [number(value.credits?.totalRemaining)];
    for (const batch of value.credits?.packages ?? []) {
      if (batch.expireAt != null && batch.expireAt <= nowMs) continue;
      rights.push(number(batch.remaining));
    }
    const expiry = value.nextKnownCreditExpiry;
    if (
      value.creditExpiryStatus === "known" &&
      expiry &&
      Number.isFinite(expiry.expiresAtMs) &&
      expiry.expiresAtMs > nowMs
    )
      rights.push(number(expiry.remaining));
    return rights;
  }
  if (snapshot?.kind === "minimax") {
    const value = snapshot.value.entitlement;
    if (!value) return [null];
    const rights: Right[] = [creditNumber(value.remainingCredits)];
    if (value.quotaState === "unavailable") {
      rights.push(null);
    } else if (value.quota) {
      for (const window of [value.quota.fiveHour, value.quota.weekly]) {
        const remaining = number(window?.remainingPercent);
        rights.push(
          window?.unlimited === true
            ? "unlimited"
            : remaining != null && remaining <= 100
              ? remaining
              : null,
        );
      }
    } else if (value.hasTokenPlan !== false) rights.push(null);
    return rights;
  }
  return [null];
}

/** Pure display projection. No API, query, storage, timer or account mutation. */
export function projectAccountOverviewRow(
  account: PlatformAccountSummary,
  context: AccountOverviewProjectionContext,
): AccountRowVM {
  const { t, locale, nowMs, automaticUnconfirmed = false } = context;
  const unavailable = context.freshness !== "fresh";
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
      (future ? value <= nowMs : value > nowMs)
    )
      return null;
    return new Intl.DateTimeFormat(locale, {
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
  const policy = context.automaticPolicy ?? {
    enabled: undefined,
    reason: null,
  };
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
  const expiry =
    snapshot?.kind === "workbuddy" &&
    snapshot.value.creditExpiryStatus === "known"
      ? snapshot.value.nextKnownCreditExpiry
      : null;
  const earliestKnownExpiryMs =
    !unavailable &&
    expiry &&
    typeof expiry.expiresAtMs === "number" &&
    Number.isFinite(expiry.expiresAtMs) &&
    Number.isFinite(new Date(expiry.expiresAtMs).getTime()) &&
    expiry.expiresAtMs > nowMs &&
    (expiry.remaining == null || (number(expiry.remaining) ?? 0) > 0)
      ? expiry.expiresAtMs
      : null;
  const rights = unavailable ? [null] : remainingRights(account, nowMs);
  const availability = rights.some(
    (right) => right === "unlimited" || (right != null && right > 0),
  )
    ? "available"
    : rights.length > 0 && rights.every((right) => right === 0)
      ? "exhausted"
      : "unknown";
  const balance = unavailable
    ? null
    : snapshot?.kind === "workbuddy"
      ? number(snapshot.value.credits?.totalRemaining)
      : snapshot?.kind === "minimax"
        ? creditNumber(snapshot.value.entitlement?.remainingCredits)
        : null;
  return {
    rowKey: JSON.stringify([account.source, account.app ?? null, account.id]),
    nativeIdentity: context.nativeIdentity ?? {
      status: "unsupported",
      selection: null,
    },
    groupId: account.platform,
    accountId: account.id,
    name: account.label,
    identity: account.identity,
    facts: facts.map(([label, value]) => ({
      label,
      value: unavailable ? unknown : value,
    })),
    lastSuccess: unavailable
      ? rewardText("pending", "数据待更新")
      : lastSuccess,
    nextExpiry: unavailable ? rewardText("pending", "数据待更新") : nextExpiry,
    earliestKnownExpiryMs,
    balance,
    availability,
    claimIssue: claimIssue ? (unavailable ? unknown : claimIssue) : null,
    detailTarget: {
      platform: account.platform,
      id: account.app ? `${account.app}:${account.id}` : account.id,
    },
    switchAction: {
      enabled:
        account.source === "workbuddy" &&
        account.platform === "workbuddy" &&
        context.freshness === "fresh" &&
        context.nativeSwitchSupported === true &&
        context.nativeIdentity?.status === "ready",
      label: t("accountsOverview.switch", { defaultValue: "切换" }),
      reason: t("accountsOverview.switchUnavailable", {
        defaultValue: "当前尚无可验证的客户端切换能力",
      }),
    },
  };
}

const compareKey = (a: AccountRowVM, b: AccountRowVM) =>
  a.rowKey < b.rowKey ? -1 : a.rowKey > b.rowKey ? 1 : 0;
function knownFirst(a: number | null, b: number | null, direction = 1) {
  return a === null
    ? b === null
      ? 0
      : 1
    : b === null
      ? -1
      : direction * (a - b);
}
export function sortAccountOverviewRows(
  rows: readonly AccountRowVM[],
  sort: AccountOverviewSort,
): AccountRowVM[] {
  return [...rows].sort((a, b) => {
    const order =
      sort === "expiryAsc"
        ? knownFirst(a.earliestKnownExpiryMs, b.earliestKnownExpiryMs)
        : sort === "balanceDesc"
          ? knownFirst(a.balance, b.balance, -1)
          : a.name.localeCompare(b.name);
    return order || compareKey(a, b);
  });
}
