import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AccountPlatform } from "./accountPlatforms";
import {
  automaticClaimKey,
  automaticClaimSetting,
  automaticPolicyStamp,
} from "./automaticClaimSettings";
import { useAccountPlatformPresence } from "./useAccountPlatformPresence";
import { useServiceAccounts } from "./useServiceAccounts";
import { useWorkBuddyNativeCatalog } from "./useWorkBuddyNativeCatalog";
import { resolveWorkBuddyNativeIdentity } from "./workbuddyNativeRowIdentity";
import {
  projectAccountOverviewRow,
  type AccountRowVM,
} from "./accountOverviewProjection";
import {
  createAccountOverviewPreferencesRepository,
  normalizeAccountOverviewPreferences,
  type AccountOverviewPreferences,
  type AccountOverviewSort,
} from "./accountOverviewPreferences";

import {
  mergeAccountsOverviewSources,
  projectAccountsOverviewGroups,
} from "./accountOverviewGroups";
export type { AccountOverviewGroupVM } from "./accountOverviewGroups";

interface Options {
  preferencesRepository?: ReturnType<
    typeof createAccountOverviewPreferencesRepository
  >;
  activeManager?: AccountPlatform;
  accountRoute?: { kind: "relay" | "vendor"; id: number };
  blocked?: boolean;
  onOpenDetails?: (target: AccountRowVM["detailTarget"]) => void;
  automaticUnconfirmed?: Record<string, string>;
  clock?: () => number;
}
/** Presentation state only. Original query/service owners retain all account writes. */
export function useAccountsOverviewViewModel(options: Options = {}) {
  const { t, i18n } = useTranslation();
  const presence = useAccountPlatformPresence();
  const services = useServiceAccounts();
  const activeManager =
    options.activeManager ??
    (options.accountRoute?.kind === "vendor" &&
    services.accounts.some(
      (row) =>
        row.kind === "vendor" &&
        row.id === options.accountRoute?.id &&
        row.row.vendorId === "opencode",
    )
      ? "opencode"
      : undefined);
  const [repository] = useState(
    () =>
      options.preferencesRepository ??
      createAccountOverviewPreferencesRepository(),
  );
  const [preferences, setPreferences] = useState(() => repository.read());
  const preferencesRef = useRef(preferences);
  const [preferencePersistence, setPreferencePersistence] = useState<
    "unchanged" | "persisted" | "sessionOnly"
  >("unchanged");
  const [selectedFilter, selectFilter] = useState<AccountPlatform>();
  const [visitedManagers, setVisitedManagers] = useState<AccountPlatform[]>(
    () => (activeManager ? [activeManager] : []),
  );
  const [manualRefreshing, setManualRefreshing] = useState(false);
  const [refreshReadFailed, setRefreshReadFailed] = useState(false);
  const refreshPending = useRef<Promise<void> | null>(null);
  useEffect(() => {
    if (activeManager)
      setVisitedManagers((previous) =>
        previous.includes(activeManager)
          ? previous
          : [...previous, activeManager],
      );
  }, [activeManager]);
  const managerIds = [
    ...new Set([...visitedManagers, ...(activeManager ? [activeManager] : [])]),
  ];

  const allAccounts = mergeAccountsOverviewSources(
    presence.data?.accounts ?? [],
    services.accounts,
  );
  const unavailablePlatforms = new Set(presence.data?.unavailable ?? []);
  if (services.error) unavailablePlatforms.add("opencode");
  const nativeCatalog = useWorkBuddyNativeCatalog({
    accounts: allAccounts,
    revision: presence.dataUpdatedAt,
    fresh:
      !presence.isPending &&
      !presence.isFetching &&
      !presence.error &&
      !unavailablePlatforms.has("workbuddy"),
  });
  const readPending = (id: AccountPlatform) =>
    id === "opencode" ? Boolean(services.isPending) : presence.isPending;
  const present = new Set([
    ...(presence.data?.platforms ?? []),
    ...allAccounts.map((row) => row.platform),
    ...unavailablePlatforms,
  ]);
  if (selectedFilter && readPending(selectedFilter))
    present.add(selectedFilter);
  const orderedGroupIds = preferences.groupOrder.filter((id) =>
    present.has(id),
  );
  const filterMissing =
    selectedFilter !== undefined &&
    !readPending(selectedFilter) &&
    !unavailablePlatforms.has(selectedFilter) &&
    !present.has(selectedFilter);
  useEffect(() => {
    if (filterMissing) selectFilter(undefined);
  }, [filterMissing]);

  const clock = options.clock ?? Date.now;
  const [, tickClock] = useState(0);
  const nowMs = clock();
  const locale = i18n.resolvedLanguage || i18n.language || "en";
  const projectedRows = allAccounts.map((account) => {
    const freshness = unavailablePlatforms.has(account.platform)
      ? "stale"
      : readPending(account.platform)
        ? "unknown"
        : "fresh";
    return projectAccountOverviewRow(account, {
      t,
      locale,
      nowMs,
      freshness,
      automaticPolicy: automaticClaimSetting(account),
      nativeIdentity: resolveWorkBuddyNativeIdentity(
        account,
        allAccounts,
        nativeCatalog,
      ),
      nativeSwitchSupported:
        nativeCatalog.state === "success" &&
        nativeCatalog.data?.activationAvailable === true,
      automaticUnconfirmed:
        options.automaticUnconfirmed?.[automaticClaimKey(account)] ===
        automaticPolicyStamp(account),
    });
  });
  const knownExpiries = projectedRows.flatMap((row) =>
    row.earliestKnownExpiryMs === null ? [] : [row.earliestKnownExpiryMs],
  );
  const nextDisplayExpiry = knownExpiries.length
    ? Math.min(...knownExpiries)
    : null;
  useEffect(() => {
    if (nextDisplayExpiry === null) return;
    const timer = setTimeout(
      () => tickClock((value) => value + 1),
      Math.max(1, Math.min(nextDisplayExpiry - clock() + 1, 2_147_483_647)),
    );
    return () => clearTimeout(timer);
  }, [nextDisplayExpiry, clock]);

  const orderedGroups = projectAccountsOverviewGroups({
    orderedGroupIds,
    projectedRows,
    preferences,
    selectedFilter,
    unavailablePlatforms,
    unknownPlatforms: new Set(orderedGroupIds.filter(readPending)),
  });
  const orderItems = orderedGroups.map((group, index) => ({
    id: group.id,
    label: group.label,
    previous: orderedGroups[index - 1]?.id,
    next: orderedGroups[index + 1]?.id,
  }));
  const scopeAccounts = allAccounts.filter(
    (account) =>
      selectedFilter === undefined || account.platform === selectedFilter,
  );
  const scopedGroups = orderedGroups.filter((group) => group.visible);
  const counts = {
    total: allAccounts.length,
    scope: scopeAccounts.length,
    visible: scopedGroups.reduce(
      (total, group) => total + group.rows.length,
      0,
    ),
    hiddenExhausted: scopedGroups.reduce(
      (total, group) => total + group.hiddenExhaustedCount,
      0,
    ),
  };

  const updatePreferences = useCallback(
    (
      update: (
        previous: AccountOverviewPreferences,
      ) => AccountOverviewPreferences,
    ) => {
      if (options.blocked) return;
      const previous = preferencesRef.current;
      const next = normalizeAccountOverviewPreferences(update(previous));
      if (JSON.stringify(previous) === JSON.stringify(next)) return;
      // Persist outside React's state updater/effects: StrictMode cannot replay a write.
      preferencesRef.current = next;
      setPreferences(next);
      const outcome = repository.save(next);
      setPreferencePersistence(outcome.persisted ? "persisted" : "sessionOnly");
    },
    [options.blocked, repository],
  );
  const reorderGroup = useCallback(
    (id: AccountPlatform, over: AccountPlatform) => {
      updatePreferences((previous) => {
        const from = previous.groupOrder.indexOf(id),
          to = previous.groupOrder.indexOf(over);
        if (from < 0 || to < 0 || from === to) return previous;
        const next = [...previous.groupOrder];
        next.splice(to, 0, next.splice(from, 1)[0]);
        return { ...previous, groupOrder: next };
      });
    },
    [updatePreferences],
  );
  const toggleCollapsed = useCallback(
    (id: AccountPlatform) =>
      updatePreferences((previous) => ({
        ...previous,
        collapsedGroups: previous.collapsedGroups.includes(id)
          ? previous.collapsedGroups.filter((group) => group !== id)
          : [...previous.collapsedGroups, id],
      })),
    [updatePreferences],
  );
  const setGroupSort = useCallback(
    (id: AccountPlatform, sort: AccountOverviewSort) =>
      updatePreferences((previous) => ({
        ...previous,
        sortByGroup: { ...previous.sortByGroup, [id]: sort },
      })),
    [updatePreferences],
  );
  const setHideExhausted = useCallback(
    (hideExhausted: boolean) =>
      updatePreferences((previous) => ({ ...previous, hideExhausted })),
    [updatePreferences],
  );
  const setFilter = useCallback(
    (id: AccountPlatform | undefined) => {
      if (!options.blocked) selectFilter(id);
    },
    [options.blocked],
  );
  const currentRows = useRef(projectedRows);
  currentRows.current = projectedRows;
  const openDetails = useCallback(
    (rowKey: string) => {
      if (options.blocked) return;
      const row = currentRows.current.find(
        (candidate) => candidate.rowKey === rowKey,
      );
      if (row) options.onOpenDetails?.(row.detailTarget);
    },
    [options.blocked, options.onOpenDetails],
  );
  const refresh = useCallback((): Promise<void> => {
    if (options.blocked) return Promise.resolve();
    if (refreshPending.current) return refreshPending.current;
    setManualRefreshing(true);
    setRefreshReadFailed(false);
    let finish!: () => void;
    const pending = new Promise<void>((resolve) => {
      finish = resolve;
    });
    refreshPending.current = pending;
    const read = <T>(query: () => Promise<T>): Promise<T> => {
      try {
        return Promise.resolve(query());
      } catch (error) {
        return Promise.reject(error);
      }
    };
    void Promise.allSettled([
      read(presence.refetch),
      read(services.reload),
    ]).then((results) => {
      // Activity may hide this retained owner; completion still updates its state.
      setRefreshReadFailed(
        results.some(
          (result) =>
            result.status === "rejected" || result.value?.isError === true,
        ),
      );
      refreshPending.current = null;
      setManualRefreshing(false);
      finish();
    });
    return pending;
  }, [options.blocked, presence.refetch, services.reload]);
  return {
    orderedGroups,
    orderItems,
    activeManager,
    presence,
    selectedFilter,
    scopeAccounts,
    counts,
    managerIds,
    preferences,
    preferencePersistence,
    unavailablePlatforms,
    refreshing: manualRefreshing || presence.isFetching,
    loading: presence.isPending || Boolean(services.isPending),
    refreshReadFailed,
    setFilter,
    toggleCollapsed,
    setGroupSort,
    setHideExhausted,
    reorderGroup,
    refresh,
    openDetails,
  };
}
