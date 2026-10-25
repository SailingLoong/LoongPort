import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  accountPlatformPresenceKey,
  type PlatformAccountSummary,
  type PlatformPresenceSnapshot,
} from "./useAccountPlatformPresence";
import {
  automaticClaimKey,
  automaticClaimSetting,
  automaticPolicyStamp,
  sameAutomaticClaimOwner,
  setAutomaticClaimSetting,
  setZCodeAutomaticSelection,
} from "./automaticClaimSettings";

export function AutomaticClaimControls({
  accounts,
  scope,
  unavailable,
  disabled,
  onRefresh,
  onBusyChange,
  unconfirmed,
  onUnconfirmed,
}: {
  accounts: PlatformAccountSummary[];
  scope: string;
  unavailable: ReadonlySet<string>;
  disabled: boolean;
  onRefresh: () => Promise<unknown>;
  onBusyChange: (busy: boolean) => void;
  unconfirmed: Record<string, string>;
  onUnconfirmed: (issues: Record<string, string>) => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const inFlight = useRef(false);
  const [busy, setBusy] = useState(false);
  const [completed, setCompleted] = useState<{
    count: number;
    scope: string;
  } | null>(null);
  const uncertain = (account: PlatformAccountSummary) =>
    unconfirmed[automaticClaimKey(account)] === automaticPolicyStamp(account);
  const rows = accounts.map((account) => ({
    account,
    ...automaticClaimSetting(account),
  }));
  const available = rows.filter(
    (row) =>
      row.enabled !== undefined &&
      !unavailable.has(row.account.platform) &&
      !uncertain(row.account),
  );
  const enabled = available.filter((row) => row.enabled).length;
  const unresolved =
    unavailable.size > 0 ||
    rows.some(
      (row) =>
        uncertain(row.account) ||
        (row.enabled === undefined &&
          row.reason !== "claimSettings.reason.unsupported"),
    );
  const status = unresolved
    ? t("claimSettings.pending")
    : enabled === 0
      ? t("claimSettings.off")
      : enabled === available.length
        ? t("claimSettings.allOn")
        : t("claimSettings.partial");
  const candidates = (enable: boolean) =>
    rows
      .filter(
        (row) =>
          !unavailable.has(row.account.platform) &&
          (enable
            ? !uncertain(row.account) && row.enabled === false && row.canEnable
            : (row.enabled === true || uncertain(row.account)) &&
              row.canDisable),
      )
      .map((row) => row.account);
  const enableTargets = candidates(true);
  const disableTargets = candidates(false);
  async function apply(enable: boolean) {
    if (inFlight.current || disabled) return;
    // This snapshot alone defines consent. Later imports never join this operation.
    const targets = candidates(enable);
    if (!targets.length) return;
    inFlight.current = true;
    setBusy(true);
    onBusyChange(true);
    setCompleted(null);
    try {
      await client.cancelQueries({ queryKey: accountPlatformPresenceKey });
      const refreshed = await onRefresh().catch(() => undefined);
      const data = (
        refreshed as { data?: PlatformPresenceSnapshot } | undefined
      )?.data;
      const fresh = (account: PlatformAccountSummary) => {
        const latest = data?.accounts?.find(
          (row) => automaticClaimKey(row) === automaticClaimKey(account),
        );
        if (
          !latest ||
          data?.unavailable.includes(account.platform) ||
          !sameAutomaticClaimOwner(account, latest)
        )
          throw new Error("Owner changed or unavailable");
        const setting = automaticClaimSetting(latest);
        if (enable ? !setting.canEnable : !setting.canDisable)
          throw new Error("Policy unavailable");
        return latest;
      };
      const zcode = targets.filter((row) => row.snapshot?.kind === "zcode");
      const individual = targets.filter(
        (row) => row.snapshot?.kind !== "zcode",
      );
      const operations = individual.map((account) => {
        const operation = {
          accounts: [account],
          run: async () => {
            const latest = fresh(account);
            operation.accounts = [latest];
            return [await setAutomaticClaimSetting(latest, enable)];
          },
        };
        return operation;
      });
      if (zcode.length)
        operations.push({
          accounts: zcode,
          run: async () => setZCodeAutomaticSelection(zcode.map(fresh), enable),
        });
      const results = await Promise.allSettled(
        operations.map((operation) => operation.run()),
      );
      const updated: PlatformAccountSummary[] = [];
      const failures: Record<string, string> = {};
      results.forEach((result, index) => {
        if (result.status === "fulfilled") updated.push(...result.value);
        else
          for (const account of operations[index].accounts)
            failures[automaticClaimKey(account)] =
              automaticPolicyStamp(account);
      });
      client.setQueryData<PlatformPresenceSnapshot>(
        accountPlatformPresenceKey,
        (previous) =>
          previous && {
            ...previous,
            accounts: previous.accounts?.map((row) => {
              const original = operations
                .flatMap((operation) => operation.accounts)
                .find(
                  (target) =>
                    automaticClaimKey(target) === automaticClaimKey(row),
                );
              const next = updated.find(
                (value) => automaticClaimKey(value) === automaticClaimKey(row),
              );
              return original &&
                next &&
                automaticPolicyStamp(original) === automaticPolicyStamp(row)
                ? next
                : row;
            }),
          },
      );
      onUnconfirmed(failures);
      setCompleted({ count: updated.length, scope });
      await onRefresh().catch(() => undefined);
    } finally {
      inFlight.current = false;
      setBusy(false);
      onBusyChange(false);
    }
  }
  return (
    <section
      aria-label={t("claimSettings.title")}
      className="rounded-md border border-border bg-card px-4 py-3"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="space-y-1">
          <h3 className="text-sm font-semibold">
            {t("claimSettings.heading")}{" "}
            <span className="ml-2 text-xs font-normal text-muted-foreground">
              {status}
            </span>
          </h3>
          <p className="text-xs text-muted-foreground">
            {t("claimSettings.scope", {
              scope,
              count: accounts.length,
              enabled,
            })}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={disabled || busy || !enableTargets.length}
            onClick={() => void apply(true)}
          >
            {t("claimSettings.enable")}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={disabled || busy || !disableTargets.length}
            onClick={() => void apply(false)}
          >
            {t("claimSettings.disable")}
          </Button>
        </div>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        {t("claimSettings.notice")}
      </p>
      <div aria-live="polite" className="mt-2 space-y-1 text-xs">
        {busy && <p role="status">{t("claimSettings.saving")}</p>}
        {completed !== null && (
          <p role="status">
            {t("claimSettings.saved", { count: completed.count })} ·{" "}
            {completed.scope}
          </p>
        )}
        {rows.map(({ account, reason }) => {
          const key = automaticClaimKey(account);
          const message = uncertain(account)
            ? t("claimSettings.uncertain")
            : unavailable.has(account.platform)
              ? t("claimSettings.readFailed")
              : reason
                ? t(reason)
                : null;
          return message ? (
            <p key={key} role={uncertain(account) ? "alert" : undefined}>
              {account.label}：{message}
            </p>
          ) : null;
        })}
      </div>
    </section>
  );
}
