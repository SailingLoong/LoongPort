import type { QueryClient } from "@tanstack/react-query";
import { AutomaticClaimNotDispatchedError } from "./automaticClaimDispatch";
import {
  accountPlatformPresenceKey,
  type PlatformAccountSummary as Account,
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

export type AutomaticClaimOutcome = {
  account: Account;
  stamp: string;
} & (
  | { status: "confirmed"; snapshot: Account }
  | { status: "notDispatched"; reason: "readFailed" | "notDispatched" }
  | { status: "uncertain" }
);
export interface AutomaticClaimRequest {
  /** The caller freezes the consent scope at the user action. */
  targets: readonly Account[];
  enabled: boolean;
}

// Revisions are ordered only after sameAutomaticClaimOwner has proved identity.
// ZCode's opaque consent revision is already compared by that owner predicate.
function policyRevision(account: Account): number | undefined {
  const snapshot = account.snapshot;
  switch (snapshot?.kind) {
    case "workbuddy":
    case "minimax":
      return snapshot.value.autoClaimRevision;
    case "trae":
      return snapshot.value.autoCheckinRevision;
    case "qoder":
      return snapshot.value.revision;
    default:
      return undefined;
  }
}

/** Coordinates existing transaction owners and their existing Query cache; never replays a write. */
export function createAutomaticClaimCommand({
  client,
  refresh,
}: {
  client: QueryClient;
  refresh: () => Promise<unknown>;
}) {
  let inFlight: Promise<AutomaticClaimOutcome[]> | undefined;
  async function run(targets: Account[], enabled: boolean) {
    const outcome = (
      account: Account,
      status: "notDispatched" | "uncertain",
      reason: "readFailed" | "notDispatched" = "notDispatched",
    ): AutomaticClaimOutcome => {
      const stamp = automaticPolicyStamp(account);
      return status === "notDispatched"
        ? { account, status, stamp, reason }
        : { account, status, stamp };
    };
    let data: PlatformPresenceSnapshot | undefined;
    try {
      await client.cancelQueries({ queryKey: accountPlatformPresenceKey });
      const result = (await refresh()) as
        | {
            data?: PlatformPresenceSnapshot;
            error?: unknown;
            isError?: boolean;
          }
        | undefined;
      if (result?.isError || result?.error != null || !result?.data)
        return targets.map((account) =>
          outcome(account, "notDispatched", "readFailed"),
        );
      data = result.data;
    } catch {
      return targets.map((account) =>
        outcome(account, "notDispatched", "readFailed"),
      );
    }
    const fresh = (account: Account): Account | undefined => {
      const latest = data?.accounts?.find(
        (row) => automaticClaimKey(row) === automaticClaimKey(account),
      );
      if (
        !latest ||
        data?.unavailable.includes(account.platform) ||
        !sameAutomaticClaimOwner(account, latest)
      )
        return;
      const clickedRevision = policyRevision(account);
      const latestRevision = policyRevision(latest);
      if (
        clickedRevision !== undefined &&
        (latestRevision === undefined || latestRevision < clickedRevision)
      )
        return;
      const setting = automaticClaimSetting(latest);
      if (enabled ? !setting.canEnable : !setting.canDisable) return;
      return latest;
    };
    const groups = targets
      .filter((row) => row.snapshot?.kind !== "zcode")
      .map((row) => [row]);
    const zcode = targets.filter((row) => row.snapshot?.kind === "zcode");
    if (zcode.length) groups.push(zcode);
    const submitted: Account[][] = groups.map(() => []);
    const settled = await Promise.allSettled(
      groups.map(async (group, index): Promise<AutomaticClaimOutcome[]> => {
        const latest = group.map(fresh);
        if (latest.some((row) => !row))
          return group.map((account) =>
            outcome(
              account,
              "notDispatched",
              data?.unavailable.includes(account.platform)
                ? "readFailed"
                : "notDispatched",
            ),
          );
        const accounts = latest as Account[];
        submitted[index] = accounts;
        const updated =
          accounts[0].snapshot?.kind === "zcode"
            ? await setZCodeAutomaticSelection(accounts, enabled)
            : [await setAutomaticClaimSetting(accounts[0], enabled)];
        return accounts.map((account, i) => ({
          account,
          stamp: automaticPolicyStamp(account),
          status: "confirmed",
          snapshot: updated[i],
        }));
      }),
    );
    const outcomes = settled.flatMap((result, index) =>
      result.status === "fulfilled"
        ? result.value
        : (submitted[index].length ? submitted[index] : groups[index]).map(
            (account) =>
              outcome(
                account,
                submitted[index].length &&
                  !(result.reason instanceof AutomaticClaimNotDispatchedError)
                  ? "uncertain"
                  : "notDispatched",
              ),
          ),
    );
    client.setQueryData<PlatformPresenceSnapshot>(
      accountPlatformPresenceKey,
      (previous) =>
        previous && {
          ...previous,
          accounts: previous.accounts?.map((row) => {
            const confirmed = outcomes.find(
              (value) =>
                value.status === "confirmed" &&
                automaticClaimKey(value.account) === automaticClaimKey(row),
            );
            return confirmed?.status === "confirmed" &&
              sameAutomaticClaimOwner(confirmed.account, row) &&
              confirmed.stamp === automaticPolicyStamp(row)
              ? confirmed.snapshot
              : row;
          }),
        },
    );
    await refresh().catch(() => undefined);
    return outcomes;
  }
  return {
    execute({
      targets,
      enabled,
    }: AutomaticClaimRequest): Promise<AutomaticClaimOutcome[]> {
      if (inFlight) return inFlight;
      const frozen = [...targets];
      if (!frozen.length) return Promise.resolve([]);
      inFlight = run(frozen, enabled).finally(() => {
        inFlight = undefined;
      });
      return inFlight;
    },
  };
}
