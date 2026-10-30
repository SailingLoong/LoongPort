import {
  accountPlatforms,
  accountPlatformLabel,
  type AccountPlatform,
} from "./accountPlatforms";
import type { PlatformAccountSummary } from "./useAccountPlatformPresence";
import type { ServiceAccount } from "./useServiceAccounts";
import {
  sortAccountOverviewRows,
  type AccountRowVM,
  type AccountOverviewProjectionContext,
} from "./accountOverviewProjection";
import type {
  AccountOverviewPreferences,
  AccountOverviewSort,
} from "./accountOverviewPreferences";

export interface AccountOverviewGroupVM {
  id: AccountPlatform;
  name: string;
  label: string;
  definition: (typeof accountPlatforms)[number];
  edition?: "CN" | "INTL";
  rows: AccountRowVM[];
  totalCount: number;
  hiddenExhaustedCount: number;
  collapsed: boolean;
  visible: boolean;
  sort: AccountOverviewSort;
  freshness: AccountOverviewProjectionContext["freshness"];
}

/** Combine original snapshots once; no routing, query or storage writes. */
export function mergeAccountsOverviewSources(
  accounts: readonly PlatformAccountSummary[],
  serviceAccounts: readonly ServiceAccount[],
): PlatformAccountSummary[] {
  const sourceAccounts = accounts;
  // Preserve the original HTTP shadow rule; IDs from unrelated owners are not merged.
  const allAccounts: PlatformAccountSummary[] = sourceAccounts.filter(
    (row) =>
      row.source !== "workbuddy-inference" ||
      !sourceAccounts.some(
        (other) => other.source === "workbuddy" && other.id === row.id,
      ),
  );
  // One conversion, shared by group rows, scope counts and details.
  for (const account of serviceAccounts) {
    if (account.kind !== "vendor" || account.row.vendorId !== "opencode")
      continue;
    allAccounts.push({
      source: "vendor",
      platform: "opencode",
      id: String(account.id),
      label: account.row.accountLabel || account.row.vendorName,
    });
  }
  return allAccounts;
}

export function projectAccountsOverviewGroups({
  orderedGroupIds,
  projectedRows,
  preferences,
  selectedFilter,
  unavailablePlatforms,
  unknownPlatforms,
}: {
  orderedGroupIds: readonly AccountPlatform[];
  projectedRows: readonly AccountRowVM[];
  preferences: AccountOverviewPreferences;
  selectedFilter?: AccountPlatform;
  unavailablePlatforms: ReadonlySet<AccountPlatform>;
  unknownPlatforms: ReadonlySet<AccountPlatform>;
}): AccountOverviewGroupVM[] {
  const orderedGroups: AccountOverviewGroupVM[] = orderedGroupIds.map((id) => {
    const definition = accountPlatforms.find((group) => group.id === id)!;
    const rawRows = projectedRows.filter((row) => row.groupId === id);
    const hiddenExhaustedCount = preferences.hideExhausted
      ? rawRows.filter((row) => row.availability === "exhausted").length
      : 0;
    const sort = preferences.sortByGroup[id] ?? "expiryAsc";
    const regions = [...new Set(rawRows.map((row) => row.identity))];
    const edition =
      "edition" in definition
        ? definition.edition === "国际版"
          ? "INTL"
          : "CN"
        : id === "qoder" && regions.length === 1
          ? regions[0] === "CN"
            ? "CN"
            : regions[0] === "Global"
              ? "INTL"
              : undefined
          : undefined;
    return {
      id,
      name: definition.name,
      label: accountPlatformLabel(definition),
      definition,
      edition,
      rows: sortAccountOverviewRows(
        rawRows.filter(
          (row) =>
            !preferences.hideExhausted || row.availability !== "exhausted",
        ),
        sort,
      ),
      totalCount: rawRows.length,
      hiddenExhaustedCount,
      collapsed: preferences.collapsedGroups.includes(id),
      visible: selectedFilter === undefined || selectedFilter === id,
      sort,
      freshness: unavailablePlatforms.has(id)
        ? "stale"
        : unknownPlatforms.has(id)
          ? "unknown"
          : "fresh",
    };
  });
  return orderedGroups;
}
