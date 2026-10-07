import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type {
  SessionCheckDisplay,
  AccountCheck,
} from "@/lib/api/zcodeAccounts";

/** Official amounts and windows are separate observations; never sum or convert them. */
export function ZCodeAccountEvidence({
  value,
  quotaPresentation = "previous",
}: {
  value: SessionCheckDisplay;
  quotaPresentation?: "previous" | "checking" | "current" | "unknown";
}) {
  const { t } = useTranslation();
  const copy = (
    key: string,
    defaultValue: string,
    values?: Record<string, string | number>,
  ) => t(`zcode.accounts.${key}`, { defaultValue, ...values });
  const unknown = copy("quotaUnknown", "Unknown");
  const states = {
    accepted: copy("checkAccepted", "Accepted"),
    unavailable: copy("checkUnavailable", "Unavailable"),
    unknown: copy("checkUnknown", "Not verified"),
  };
  const entitlements = {
    available: copy("entitlementAvailable", "Entitlement available"),
    unavailable: copy("entitlementUnavailable", "Entitlement unavailable"),
    pending: copy("entitlementPending", "Entitlement pending"),
    unknown: copy("entitlementUnknown", "Entitlement unknown"),
  };
  const reasons = {
    unverified: copy("reasonUnverified", "Not yet verified"),
    missingCredential: copy(
      "reasonMissingCredential",
      "Required credential missing",
    ),
    invalidCredential: copy("reasonInvalidCredential", "Invalid credential"),
    appVersionUnknown: copy(
      "reasonAppVersionUnknown",
      "Client version unknown",
    ),
    authRejected: copy("reasonAuthRejected", "Authorization rejected"),
    businessRejected: copy(
      "reasonBusinessRejected",
      "Business session rejected",
    ),
    malformedResponse: copy(
      "reasonMalformedResponse",
      "Unrecognized official response",
    ),
    timeout: copy("reasonTimeout", "Query timed out"),
    network: copy("reasonNetwork", "Network query failed"),
  };
  const time = (v: number, milliseconds = false) => {
    const date = new Date(milliseconds ? v : v * 1000);
    return Number.isFinite(date.getTime()) ? date.toISOString() : unknown;
  };
  const optional = (label: string, content: ReactNode) =>
    content === null || content === undefined || content === "" ? null : (
      <p>
        {label}: {content}
      </p>
    );
  const amount = (label: string, v: number | null, unit?: string | null) => (
    <p>
      {label}: {v === null ? unknown : v}
      {v !== null && unit ? ` ${unit}` : ""}
    </p>
  );
  const check = (label: string, result: AccountCheck) => (
    <div className="space-y-0.5">
      <p>
        {label}: {states[result.state]}
        {result.reason ? ` · ${reasons[result.reason]}` : ""}
      </p>
      {result.checkedAt !== null && (
        <p>
          {copy("evidenceCheckedAt", "Checked: {{time}}", {
            time: time(result.checkedAt),
          })}
        </p>
      )}
      {result.latestFailure && (
        <p>
          {copy(
            "evidenceLatestFailure",
            "Latest query: {{reason}} · {{time}}",
            {
              reason: reasons[result.latestFailure.reason],
              time: time(result.latestFailure.checkedAt),
            },
          )}
        </p>
      )}
    </div>
  );
  return (
    <div className="mt-2 space-y-3 text-xs text-muted-foreground">
      <p>
        {quotaPresentation === "current"
          ? copy("quotaCurrentCheck", "Quota returned by this check")
          : quotaPresentation === "checking"
            ? copy(
                "quotaPreviousChecking",
                "Previous quota values · checking for updates",
              )
            : quotaPresentation === "unknown"
              ? copy(
                  "quotaPreviousNotCurrent",
                  "Previous quota values · not current",
                )
              : copy("quotaLastKnown", "Last known quota values")}
      </p>
      {check(copy("businessSession", "Business session"), value.business.check)}
      <div className="space-y-1">
        {check("Start Plan", value.start.check)}
        <p>{entitlements[value.start.entitlement]}</p>
        {value.start.effectiveAtSeconds !== null &&
          optional(
            copy("effectiveAt", "Effective at"),
            time(value.start.effectiveAtSeconds),
          )}
        {check(copy("startQuota", "Start quota"), value.start.quota)}
        {(quotaPresentation === "unknown" ||
          value.start.quota.state === "unknown") && (
          <p>
            {copy(
              "startQuotaCurrentUnknown",
              "Current Start quota is unknown.",
            )}
          </p>
        )}
        {quotaPresentation === "current" &&
          value.start.quota.state !== "accepted" &&
          value.start.buckets.length > 0 && (
            <p>
              {copy(
                "quotaPreviousNotCurrent",
                "Previous quota values · not current",
              )}
            </p>
          )}
        {value.start.serverTimeSeconds !== null &&
          optional(
            copy("serverTime", "Official server time"),
            time(value.start.serverTimeSeconds),
          )}
        {value.start.plans.map((plan, index) => (
          <div
            key={index}
            className="space-y-1 rounded border border-border-default p-2"
          >
            {optional(copy("planName", "Plan"), plan.name)}
            {optional(copy("planInstance", "Plan instance"), plan.userPlanId)}
            {optional(copy("planId", "Plan ID"), plan.planId)}
            {optional(
              copy("officialPlanStatus", "Official plan status"),
              plan.status,
            )}
            {plan.startsAtSeconds !== null &&
              optional(
                copy("startsAt", "Starts at"),
                time(plan.startsAtSeconds),
              )}
            {plan.endsAtSeconds !== null &&
              optional(copy("endsAt", "Ends at"), time(plan.endsAtSeconds))}
            {plan.entitlements.map((entry, entryIndex) => (
              <div key={entryIndex}>
                {optional(
                  copy("entitlementName", "Entitlement"),
                  entry.showName,
                )}
                {optional(
                  copy("entitlementId", "Entitlement ID"),
                  entry.entitlementId,
                )}
                {optional(copy("quotaPeriod", "Period"), entry.period)}
                {entry.effectiveAtSeconds !== null &&
                  optional(
                    copy("effectiveAt", "Effective at"),
                    time(entry.effectiveAtSeconds),
                  )}
              </div>
            ))}
          </div>
        ))}
        {value.start.buckets.map((bucket, index) => (
          <div
            key={index}
            className="space-y-1 rounded border border-border-default p-2"
            role="group"
            aria-label={copy("quotaBucket", "Quota bucket {{number}}", {
              number: index + 1,
            })}
          >
            {optional(copy("quotaName", "Quota"), bucket.showName)}
            {optional(copy("bucketId", "Bucket ID"), bucket.bucketId)}
            {optional(copy("planInstance", "Plan instance"), bucket.userPlanId)}
            {optional(copy("planId", "Plan ID"), bucket.planId)}
            {optional(
              copy("entitlementId", "Entitlement ID"),
              bucket.entitlementId,
            )}
            {optional(copy("quotaMeter", "Meter"), bucket.meter)}
            <p>
              {copy("quotaUnit", "Unit")}:{" "}
              {bucket.unitType ?? copy("quotaUnitMissing", "Not provided")}
            </p>
            {amount(
              copy("quotaTotal", "Total"),
              bucket.totalUnits,
              bucket.unitType,
            )}
            {amount(
              copy("quotaUsed", "Used"),
              bucket.usedUnits,
              bucket.unitType,
            )}
            {amount(
              copy("quotaReserved", "Reserved"),
              bucket.reservedUnits,
              bucket.unitType,
            )}
            {amount(
              copy("quotaRemaining", "Remaining"),
              bucket.remainingUnits,
              bucket.unitType,
            )}
            {amount(
              copy("quotaAvailable", "Available"),
              bucket.availableUnits,
              bucket.unitType,
            )}
            {bucket.periodStartSeconds !== null &&
              optional(
                copy("periodStartsAt", "Window starts"),
                time(bucket.periodStartSeconds),
              )}
            {bucket.periodEndSeconds !== null &&
              optional(
                copy("periodEndsAt", "Window ends"),
                time(bucket.periodEndSeconds),
              )}
            {bucket.expiresAtSeconds !== null &&
              optional(
                copy("expiresAt", "Expires at"),
                time(bucket.expiresAtSeconds),
              )}
          </div>
        ))}
      </div>
      <div className="space-y-1">
        {check("Coding Plan", value.coding.check)}
        <p>{entitlements[value.coding.entitlement]}</p>
        {check(
          copy("codingSubscription", "Coding subscription"),
          value.coding.subscription,
        )}
        {value.coding.subscriptions.map((subscription, index) => (
          <div
            key={index}
            className="space-y-1 rounded border border-border-default p-2"
          >
            {optional(copy("planName", "Plan"), subscription.productName)}
            {optional(copy("planId", "Plan ID"), subscription.productId)}
            {optional(
              copy("officialPlanStatus", "Official plan status"),
              subscription.status,
            )}
            {optional(
              copy("billingCycle", "Billing cycle"),
              subscription.billingCycle,
            )}
            {optional(
              copy("nextRenewal", "Next renewal"),
              subscription.nextRenewTime,
            )}
            {optional(
              copy("officialValidity", "Official validity"),
              subscription.valid,
            )}
          </div>
        ))}
        {check(copy("codingQuota", "Coding quota"), value.coding.quota)}
        {(quotaPresentation === "unknown" ||
          value.coding.quota.state === "unknown") && (
          <p>
            {copy(
              "codingQuotaCurrentUnknown",
              "Current Coding quota is unknown.",
            )}
          </p>
        )}
        {quotaPresentation === "current" &&
          value.coding.quota.state !== "accepted" &&
          value.coding.limits.length > 0 && (
            <p>
              {copy(
                "quotaPreviousNotCurrent",
                "Previous quota values · not current",
              )}
            </p>
          )}
        {value.coding.limits.map((limit, index) => (
          <div
            key={index}
            className="space-y-1 rounded border border-border-default p-2"
            role="group"
            aria-label={copy("codingLimit", "Coding limit {{number}}", {
              number: index + 1,
            })}
          >
            <p>{limit.limitType}</p>
            <p>
              {copy("quotaUnit", "Unit")}:{" "}
              {limit.displayUnit ?? copy("quotaUnitMissing", "Not provided")}
            </p>
            <p>
              {copy("quotaWindow", "Window")}:{" "}
              {limit.windowLabel ?? copy("quotaWindowMissing", "Not provided")}
            </p>
            {amount(copy("officialUsage", "Official usage value"), limit.usage)}
            {amount(
              copy("quotaCurrentValue", "Current value"),
              limit.currentValue,
              limit.displayUnit,
            )}
            {amount(
              copy("quotaRemaining", "Remaining"),
              limit.remaining,
              limit.displayUnit,
            )}
            {amount(
              copy("quotaPercentage", "Percentage"),
              limit.percentage,
              "%",
            )}
            {limit.nextResetTimeMs !== null &&
              optional(
                copy("nextReset", "Next reset"),
                time(limit.nextResetTimeMs, true),
              )}
            {limit.usageDetails.map((entry, entryIndex) => (
              <div key={entryIndex}>
                {optional(
                  copy("usageModel", "Model"),
                  entry.displayName ?? entry.modelCode,
                )}
                {amount(
                  copy("officialUsage", "Official usage value"),
                  entry.usage,
                )}
              </div>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}
