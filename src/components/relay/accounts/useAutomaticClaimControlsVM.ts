import { useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { PlatformAccountSummary } from "./useAccountPlatformPresence";
import {
  automaticClaimKey,
  automaticClaimSetting,
  automaticPolicyStamp,
  sameAutomaticClaimOwner,
} from "./automaticClaimSettings";
import {
  createAutomaticClaimCommand,
  type AutomaticClaimOutcome,
} from "./automaticClaimCommand";

export interface AutomaticClaimControlsInput {
  accounts: PlatformAccountSummary[];
  scope: string;
  unavailable: ReadonlySet<string>;
  disabled: boolean;
  onRefresh: () => Promise<unknown>;
  onBusyChange: (busy: boolean) => void;
  unconfirmed: Record<string, string>;
  onUnconfirmed: (issues: Record<string, string>) => void;
}

/** Headless projection and action owner for the automatic policy controls. */
export function useAutomaticClaimControlsVM(
  input: AutomaticClaimControlsInput,
) {
  const client = useQueryClient();
  const latest = useRef(input);
  latest.current = input;
  const command = useMemo(
    () =>
      createAutomaticClaimCommand({
        client,
        refresh: () => latest.current.onRefresh(),
      }),
    [client],
  );
  const [busy, setBusy] = useState(false);
  const [blocked, setBlocked] = useState<{
    epoch: object;
    reasons: Record<string, "readFailed" | "notDispatched">;
  } | null>(null);
  const [completed, setCompleted] = useState<{
    count: number;
    scope: string;
    epoch: object;
    outcomes: AutomaticClaimOutcome[];
  } | null>(null);
  const identity = JSON.stringify([
    input.scope,
    input.accounts.map(automaticClaimKey),
  ]);
  const scope = useRef({ identity, accounts: input.accounts, epoch: {} });
  const ownersUnchanged = scope.current.accounts.every((previous, index) => {
    const current = input.accounts[index];
    if (!current) return false;
    // Unsupported rows have no policy owner to fence. Native owners use the
    // command generation predicate; ZCode consent continuity is checked below.
    if (!previous.snapshot || previous.snapshot.kind === "auth")
      return previous.snapshot?.kind === current.snapshot?.kind;
    if (previous.snapshot.kind === "zcode")
      return (
        current.snapshot?.kind === "zcode" &&
        previous.snapshot.dataRoot === current.snapshot.dataRoot
      );
    return sameAutomaticClaimOwner(previous, current);
  });
  if (scope.current.identity !== identity || !ownersUnchanged)
    scope.current = { identity, accounts: input.accounts, epoch: {} };
  else scope.current.accounts = input.accounts;
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const inFlight = useRef(false);
  // React props may not rerender between two awaited actions in the same turn.
  // Retain only command uncertainty here to fence that synchronous replay gap.
  const pendingUncertain = useRef<Record<string, string>>({});
  const rows = input.accounts.map((account) => {
    const uncertain =
      input.unconfirmed[automaticClaimKey(account)] ===
      automaticPolicyStamp(account);
    const blockedReason =
      blocked?.epoch === scope.current.epoch
        ? blocked.reasons[automaticClaimKey(account)]
        : undefined;
    const notDispatched = blockedReason !== undefined;
    const setting = automaticClaimSetting(account);
    const unavailable = input.unavailable.has(account.platform);
    return {
      account,
      key: automaticClaimKey(account),
      ...setting,
      uncertain,
      blocked: notDispatched,
      unavailable,
      message: uncertain
        ? "claimSettings.uncertain"
        : unavailable || blockedReason === "readFailed"
          ? "claimSettings.readFailed"
          : notDispatched
            ? "claimSettings.notDispatched"
            : setting.reason,
      alert: uncertain || notDispatched,
    };
  });
  const available = rows.filter(
    (row) => row.enabled !== undefined && !row.unavailable && !row.uncertain,
  );
  const enabled = available.filter((row) => row.enabled).length;
  const unresolved =
    input.unavailable.size > 0 ||
    rows.some(
      (row) =>
        row.uncertain ||
        row.blocked ||
        (row.enabled === undefined &&
          row.reason !== "claimSettings.reason.unsupported"),
    );
  const status = unresolved
    ? "claimSettings.pending"
    : enabled === 0
      ? "claimSettings.off"
      : enabled === available.length
        ? "claimSettings.allOn"
        : "claimSettings.partial";
  const targets = (enable: boolean) =>
    rows
      .filter(
        (row) =>
          !row.unavailable &&
          (enable
            ? !row.uncertain &&
              pendingUncertain.current[row.key] !==
                automaticPolicyStamp(row.account) &&
              row.enabled === false &&
              row.canEnable
            : (row.enabled === true ||
                row.uncertain ||
                pendingUncertain.current[row.key] ===
                  automaticPolicyStamp(row.account)) &&
              row.canDisable),
      )
      .map((row) => row.account);
  async function apply(enable: boolean) {
    if (inFlight.current || input.disabled) return;
    const frozen = targets(enable);
    if (!frozen.length) return;
    const epoch = scope.current.epoch;
    const clickedScope = input.scope;
    inFlight.current = true;
    setBusy(true);
    input.onBusyChange(true);
    setCompleted(null);
    setBlocked(null);
    try {
      const outcomes = await command.execute({
        targets: frozen,
        enabled: enable,
      });
      const issues: Record<string, string> = {};
      const notDispatched: Record<string, "readFailed" | "notDispatched"> = {};
      for (const outcome of outcomes) {
        const key = automaticClaimKey(outcome.account);
        if (outcome.status === "uncertain") {
          issues[key] = outcome.stamp;
          pendingUncertain.current[key] = outcome.stamp;
        } else if (
          outcome.status === "confirmed" &&
          pendingUncertain.current[key] === outcome.stamp
        ) {
          delete pendingUncertain.current[key];
        }
        if (outcome.status === "notDispatched")
          notDispatched[key] = outcome.reason;
      }
      // Account-keyed issues remain attached to the original owner, even after navigation.
      input.onUnconfirmed(issues);
      if (mounted.current && scope.current.epoch === epoch) {
        setBlocked({ epoch, reasons: notDispatched });
        const count = outcomes.filter(
          (result) => result.status === "confirmed",
        ).length;
        setCompleted(
          count ? { count, scope: clickedScope, epoch, outcomes } : null,
        );
      }
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(false);
      input.onBusyChange(false);
    }
  }
  // ZCode consent revisions advance on a successful write. A new token is not
  // interchangeable with the clicked token: only this command's receipt proves
  // continuity. Dispatch retains the stricter clicked-consent owner predicate.
  const completionOwnersCurrent = completed?.outcomes.every((outcome) => {
    if (outcome.account.snapshot?.kind !== "zcode") return true;
    const current = input.accounts.find(
      (row) => automaticClaimKey(row) === automaticClaimKey(outcome.account),
    );
    return (
      current !== undefined &&
      (sameAutomaticClaimOwner(outcome.account, current) ||
        (outcome.status === "confirmed" &&
          sameAutomaticClaimOwner(outcome.snapshot, current)))
    );
  });
  if (completed?.epoch === scope.current.epoch && !completionOwnersCurrent)
    scope.current = { ...scope.current, epoch: {} };
  return {
    rows,
    enabled,
    status,
    busy,
    completed:
      completed?.epoch === scope.current.epoch && completionOwnersCurrent
        ? completed
        : null,
    enableDisabled: input.disabled || busy || !targets(true).length,
    disableDisabled: input.disabled || busy || !targets(false).length,
    enable: () => apply(true),
    disable: () => apply(false),
  };
}
