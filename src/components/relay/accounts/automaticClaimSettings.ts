import { AutomaticClaimNotDispatchedError } from "./automaticClaimDispatch";
import { type WorkBuddyAccount, workbuddyApi } from "@/lib/api/workbuddy";
import { minimaxCodeApi } from "@/lib/api/minimaxCode";
import { type TraeAccountView, traeAccountsApi } from "@/lib/api/traeAccounts";
import { qoderDailyApi } from "@/lib/api/qoderDaily";
import { qoderCnCampaignsApi } from "@/lib/api/qoderCnCampaigns";
import { zcodeClaimApi } from "@/lib/api/zcodeClaim";
import type { PlatformAccountSummary } from "./useAccountPlatformPresence";

type Account = PlatformAccountSummary;
export const automaticClaimKey = (account: Account) =>
  `${account.source}:${account.app ?? ""}:${account.id}`;

/** Presentation of owner policy. Never derives eligibility from credits or inference. */
export function automaticClaimSetting(account: Account) {
  const snapshot = account.snapshot;
  let enabled: boolean | undefined;
  let canEnable = false;
  let canDisable = false;
  let reason: string | null = null;
  switch (snapshot?.kind) {
    case "workbuddy": {
      const a = snapshot.value;
      enabled = a.autoClaimEnabled;
      canDisable =
        a.generation !== undefined &&
        a.autoClaimRevision !== undefined &&
        a.autoClaimPolicyVersion !== undefined;
      canEnable = a.canSetAutomatic === true && canDisable;
      if (a.autoClaimBlocked) reason = "claimSettings.reason.actionRequired";
      break;
    }
    case "minimax": {
      const a = snapshot.value;
      enabled = a.autoClaimEnabled;
      canDisable =
        a.authGeneration !== undefined && a.autoClaimRevision !== undefined;
      canEnable = a.authorized === true && canDisable;
      if (a.autoClaimFailure)
        reason = `minimaxCode.errors.${a.autoClaimFailure}`;
      break;
    }
    case "trae": {
      const a = snapshot.value;
      enabled = a.autoCheckinEnabled;
      canDisable =
        a.generation !== undefined && a.autoCheckinRevision !== undefined;
      canEnable = a.canSetAutomatic === true && canDisable;
      if (a.checkinAttemptStatus === "unknown")
        reason = "claimSettings.reason.pending";
      else if (a.checkinAttemptStatus === "ineligible")
        reason = "claimSettings.reason.ineligible";
      break;
    }
    case "qoder":
      enabled = snapshot.value.enabled;
      canEnable = snapshot.value.canEnable;
      canDisable = true;
      if (snapshot.value.reason) reason = "claimSettings.reason.unavailable";
      break;
    case "zcode":
      enabled =
        snapshot.value.enabled &&
        snapshot.value.participants.includes(account.id);
      canEnable = !snapshot.value.busy && !!snapshot.value.consentRevision;
      canDisable = canEnable;
      if (snapshot.value.busy) reason = "claimSettings.reason.busy";
      else if (snapshot.value.records[account.id]?.reason)
        reason = "claimSettings.reason.actionRequired";
      break;
  }
  return {
    enabled,
    canEnable,
    canDisable,
    reason:
      reason ??
      (account.automaticReadFailed
        ? "claimSettings.reason.readFailed"
        : !snapshot ||
            !["workbuddy", "minimax", "trae", "qoder", "zcode"].includes(
              snapshot.kind,
            )
          ? "claimSettings.reason.unsupported"
          : enabled === undefined
            ? "claimSettings.reason.unknown"
            : !canEnable && !enabled
              ? "claimSettings.reason.unavailable"
              : null),
  };
}

/** A setter receipt confirms only the requested policy on the reviewed owner. */
export function confirmsWorkBuddyAutomaticPolicy(
  previous: WorkBuddyAccount,
  next: WorkBuddyAccount,
  enabled: boolean,
): boolean {
  return (
    next.id === previous.id &&
    next.generation === previous.generation &&
    next.autoClaimPolicyVersion === previous.autoClaimPolicyVersion &&
    next.autoClaimRevision > previous.autoClaimRevision &&
    next.autoClaimEnabled === enabled
  );
}

export function confirmsTraeAutomaticPolicy(
  previous: TraeAccountView,
  next: TraeAccountView,
  enabled: boolean,
): boolean {
  return (
    next.id === previous.id &&
    next.generation === previous.generation &&
    next.autoCheckinRevision !== undefined &&
    next.autoCheckinRevision > (previous.autoCheckinRevision ?? 0) &&
    next.autoCheckinEnabled === enabled
  );
}

/** Set explicit existing snapshots through their original owner, retaining its concurrency checks. */
export async function setAutomaticClaimSetting(
  account: Account,
  enabled: boolean,
): Promise<Account> {
  const snapshot = account.snapshot;
  if (!snapshot)
    throw new AutomaticClaimNotDispatchedError("Unavailable policy");
  if (
    snapshot.kind !== "auth" &&
    snapshot.kind !== "zcode" &&
    (snapshot.kind === "qoder"
      ? snapshot.value.providerId
      : snapshot.value.id) !== account.id
  )
    throw new AutomaticClaimNotDispatchedError("Owner changed");
  switch (snapshot.kind) {
    case "workbuddy": {
      const next = await workbuddyApi.setAutomatic(snapshot.value, enabled);
      if (!confirmsWorkBuddyAutomaticPolicy(snapshot.value, next, enabled))
        throw new Error("Unconfirmed policy");
      return { ...account, snapshot: { kind: "workbuddy", value: next } };
    }
    case "minimax": {
      const next = await minimaxCodeApi.setAutomatic(snapshot.value, enabled);
      if (
        next.id !== account.id ||
        next.authGeneration !== snapshot.value.authGeneration ||
        next.autoClaimRevision === undefined ||
        next.autoClaimRevision <= (snapshot.value.autoClaimRevision ?? 0) ||
        next.autoClaimEnabled !== enabled
      )
        throw new Error("Unconfirmed policy");
      return { ...account, snapshot: { kind: "minimax", value: next } };
    }
    case "trae": {
      const next = await traeAccountsApi.setAutomatic(snapshot.value, enabled);
      if (!confirmsTraeAutomaticPolicy(snapshot.value, next, enabled))
        throw new Error("Unconfirmed policy");
      return { ...account, snapshot: { kind: "trae", value: next } };
    }
    case "qoder": {
      const next =
        snapshot.source === "internationalQoderPat"
          ? await qoderDailyApi.setAutomatic(snapshot.value, enabled)
          : await qoderCnCampaignsApi.setAutomatic(
              snapshot.value,
              enabled,
              snapshot.source,
            );
      if (
        next.providerId !== account.id ||
        next.credentialGeneration !== snapshot.value.credentialGeneration ||
        next.policyVersion !== snapshot.value.policyVersion ||
        next.revision <= snapshot.value.revision ||
        next.enabled !== enabled
      )
        throw new Error("Unconfirmed policy");
      return { ...account, snapshot: { ...snapshot, value: next } };
    }
    default:
      throw new AutomaticClaimNotDispatchedError("Unavailable policy");
  }
}

// ZCode owns a participant list, so update the selected set once, preserving outsiders.
export async function setZCodeAutomaticSelection(
  accounts: Account[],
  enabled: boolean,
): Promise<Account[]> {
  const snapshot = accounts[0]?.snapshot;
  if (snapshot?.kind !== "zcode")
    throw new AutomaticClaimNotDispatchedError("Unavailable policy");
  const current = await zcodeClaimApi
    .state(snapshot.dataRoot)
    .catch((cause: unknown) => {
      throw new AutomaticClaimNotDispatchedError("Policy read unavailable", {
        cause,
      });
    });
  if (current.busy) throw new AutomaticClaimNotDispatchedError("Owner busy");
  if (
    !snapshot.value.consentRevision ||
    accounts.some((account) => {
      const reviewed = account.snapshot;
      return (
        reviewed?.kind !== "zcode" ||
        reviewed.dataRoot !== snapshot.dataRoot ||
        reviewed.value.consentRevision !== snapshot.value.consentRevision
      );
    }) ||
    current.consentRevision !== snapshot.value.consentRevision
  )
    throw new AutomaticClaimNotDispatchedError("Changed policy");
  const selected = new Set(accounts.map((account) => account.id));
  const participants = enabled
    ? [
        ...new Set([
          ...(current.enabled ? current.participants : []),
          ...selected,
        ]),
      ]
    : current.participants.filter((id) => !selected.has(id));
  const next = await zcodeClaimApi.setAuto(
    snapshot.dataRoot,
    enabled || (current.enabled && participants.length > 0),
    participants,
    snapshot.value.consentRevision,
  );
  if (
    !next.consentRevision ||
    next.consentRevision === snapshot.value.consentRevision ||
    accounts.some(
      (account) =>
        (next.enabled && next.participants.includes(account.id)) !== enabled,
    )
  )
    throw new Error("Unconfirmed policy");
  return accounts.map((account) => ({
    ...account,
    snapshot: { ...snapshot, value: next },
  }));
}

/** Only policy identity/revision is compared; quota refreshes cannot resolve an uncertain write. */
export function automaticPolicyStamp(account: Account): string {
  const s = account.snapshot;
  switch (s?.kind) {
    case "workbuddy":
      return JSON.stringify([
        s.value.generation,
        s.value.autoClaimRevision,
        s.value.autoClaimPolicyVersion,
      ]);
    case "minimax":
      return JSON.stringify([
        s.value.authGeneration,
        s.value.autoClaimRevision,
      ]);
    case "trae":
      return JSON.stringify([s.value.generation, s.value.autoCheckinRevision]);
    case "qoder":
      return JSON.stringify([
        s.value.credentialGeneration,
        s.value.revision,
        s.value.policyVersion,
      ]);
    case "zcode":
      return JSON.stringify([
        s.dataRoot,
        s.value.consentRevision,
        s.value.enabled && s.value.participants.includes(account.id),
      ]);
    default:
      return "unknown";
  }
}

export function sameAutomaticClaimOwner(a: Account, b: Account): boolean {
  if (automaticClaimKey(a) !== automaticClaimKey(b)) return false;
  const x = a.snapshot,
    y = b.snapshot;
  if (!x || !y || x.kind !== y.kind) return false;
  switch (x.kind) {
    case "workbuddy":
      return (
        y.kind === "workbuddy" &&
        x.value.generation === y.value.generation &&
        x.value.autoClaimPolicyVersion === y.value.autoClaimPolicyVersion
      );
    case "minimax":
      return (
        y.kind === "minimax" &&
        x.value.authGeneration === y.value.authGeneration
      );
    case "trae":
      return y.kind === "trae" && x.value.generation === y.value.generation;
    case "qoder":
      return (
        y.kind === "qoder" &&
        x.value.credentialGeneration === y.value.credentialGeneration &&
        x.value.policyVersion === y.value.policyVersion &&
        x.source === y.source
      );
    case "zcode":
      return (
        y.kind === "zcode" &&
        x.dataRoot === y.dataRoot &&
        !!x.value.consentRevision &&
        x.value.consentRevision === y.value.consentRevision
      );
    default:
      return false;
  }
}
