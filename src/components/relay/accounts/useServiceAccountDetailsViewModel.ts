import { useCallback, useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { AccountRoute } from "@/components/shell/navigation";
import { getAppDisplayName } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
import { usageApi } from "@/lib/api/usage";
import { fmtUsd } from "@/components/usage/format";
import { formatResetAt } from "@/components/applications/tierMetrics";
import { useRowBusy } from "../useRowBusy";
import { serviceSourcesKey } from "./serviceSourceKeys";
import { useServiceAccounts } from "./useServiceAccounts";
import {
  accountPlatforms,
  accountPlatformLabel,
  type AccountPlatform,
} from "./accountPlatforms";
import {
  serviceEntryIntent,
  type ServiceEntryIntent,
} from "./serviceEntryIntent";
import {
  inServiceAccountScope,
  projectServiceAccounts,
  serviceAccountName,
  serviceAccountSnapshot,
  projectSubscriptionWindows,
  projectAccountUsage,
  type ServiceAccountScope,
  type ServiceAccountSelection,
} from "./serviceAccountDetailsProjection";

export interface ServiceAccountDetailsOptions {
  appId: AppId;
  platform?: AccountPlatform;
  accountKind?: ServiceAccountScope;
  account?: AccountRoute;
  target?: { id: string; request: number };
  onSelectAccount?: (account: AccountRoute | undefined, app: AppId) => void;
  onBack?: () => void;
  onOpenApp: (app: AppId) => void;
  onOpenAddHub?: (
    tab: "accounts",
    opts?: { intent?: ServiceEntryIntent },
  ) => void;
}

function projectEntry(platform: AccountPlatform | undefined) {
  if (!platform) return { kind: "accounts" as const };
  const title = accountPlatformLabel(
    accountPlatforms.find((item) => item.id === platform)!,
  );
  const intent = serviceEntryIntent(platform);
  if (platform === "qoder") return { kind: "qoder" as const };
  if (intent)
    return {
      kind: "source" as const,
      title,
      intent,
      description:
        platform === "openrouter"
          ? "API Key 连接，不代表平台账号签到或积分接入。"
          : "OpenCode 服务来源（Zen / Go / Free），与消费应用选择分开。",
    };
  return { kind: "platform" as const, title, platform };
}

/** One orchestration owner for the existing global account/detail consumers. */
export function useServiceAccountDetailsViewModel(
  options: ServiceAccountDetailsOptions,
) {
  const {
    appId,
    platform,
    accountKind,
    account: route,
    target,
    onSelectAccount,
    onBack,
    onOpenApp,
    onOpenAddHub,
  } = options;
  const { t } = useTranslation();
  const inventory = useServiceAccounts();
  const client = useQueryClient();
  const activity = useRowBusy();
  const [localSelection, setLocalSelection] =
    useState<ServiceAccountSelection | null>(null);
  const requested = onSelectAccount
    ? route
      ? { ...route, appId }
      : null
    : localSelection;
  const accounts = inventory.accounts.filter((account) =>
    inServiceAccountScope(account, accountKind),
  );
  const requestedAccount = requested
    ? inventory.accounts.find(
        (account) =>
          account.kind === requested.kind && account.id === requested.id,
      )
    : undefined;
  // Unknown routed accounts retain the existing detail/recovery owner. A known
  // OpenCode route is isolated from the ordinary vendor manager.
  const isOpenCode =
    requestedAccount?.kind === "vendor" &&
    requestedAccount.row.vendorId === "opencode";
  const selection =
    requested &&
    (accountKind === "opencode"
      ? isOpenCode
      : accountKind === "relay-and-vendor"
        ? !isOpenCode
        : true)
      ? requested
      : null;
  const selected = selection
    ? accounts.find(
        (account) =>
          account.kind === selection.kind && account.id === selection.id,
      )
    : undefined;
  const setSelection = useCallback(
    (next: ServiceAccountSelection | null) => {
      if (onSelectAccount)
        onSelectAccount(next ?? undefined, next?.appId ?? appId);
      else setLocalSelection(next);
    },
    [onSelectAccount, appId],
  );
  const handledTarget =
    useRef<ServiceAccountDetailsOptions["target"]>(undefined);
  useEffect(() => {
    if (handledTarget.current === target) return;
    handledTarget.current = target;
    if (target && accountKind === "opencode")
      setSelection({ kind: "vendor", id: Number(target.id), appId });
  }, [target, accountKind, appId, setSelection]);
  const reloadAccounts = useCallback(
    async (requireFresh = false) => {
      const [result] = await Promise.all([
        inventory.reload(),
        client.invalidateQueries({ queryKey: serviceSourcesKey }),
      ]);
      if (requireFresh && result.error) throw result.error;
    },
    [inventory.reload, client],
  );
  const back = () => {
    if (onBack) onBack();
    else setSelection(null);
    void reloadAccounts();
  };
  const entry = projectEntry(platform);
  const tiers =
    entry.kind === "accounts" && selected?.kind === "relay"
      ? [...selected.apps.values()]
      : [];
  const entries = tiers.flatMap((row) =>
    row.tiers.map((tier) => ({
      providerId: tier.providerId,
      displayName: tier.displayName,
    })),
  );
  const usageQuery = useQuery({
    queryKey: ["account-usage", entries.map((entry) => entry.providerId)],
    queryFn: () =>
      usageApi.providersWindowCost(entries.map((entry) => entry.providerId)),
    staleTime: 60_000,
    enabled: entry.kind === "accounts" && selected?.kind === "relay",
  });
  const usage = projectAccountUsage(entries, usageQuery.data ?? []);
  const detail = selection
    ? {
        key: `${selection.kind}:${selection.id}:${selection.appId}`,
        account: { kind: selection.kind, id: selection.id },
        appId: selection.appId,
        name: selected
          ? serviceAccountName(selected)
          : t("loongport.accounts.detail"),
        label: selected?.row.accountLabel,
        kindLabel: t(`loongport.accounts.${selection.kind}`),
        showBack: !onSelectAccount,
        backLabel: t(onBack ? "common.back" : "loongport.accounts.back"),
        apps: selected
          ? [...selected.apps.keys()].map((id) => ({
              id,
              label: getAppDisplayName(id, t),
            }))
          : [],
        hasAccount: Boolean(selected),
        snapshot: serviceAccountSnapshot(selected, selection.appId),
        chooseApp: (id: AppId) => setSelection({ ...selection, appId: id }),
        openApp: () => onOpenApp(selection.appId),
        windows: projectSubscriptionWindows(tiers).map((window) => ({
          ...window,
          label: t(`loongport.accounts.windowKind.${window.kind}`),
          resetLabel:
            window.resetAt == null ? "—" : formatResetAt(window.resetAt),
        })),
        usage: usage
          ? {
              ...usage,
              headline: t("loongport.accounts.usageHeadline", {
                seven: fmtUsd(usage.total7, 2),
                thirty: fmtUsd(usage.total30, 2),
              }),
              rows: usage.rows.map((row) => ({
                ...row,
                costLabel: fmtUsd(row.cost30, 2),
              })),
            }
          : null,
      }
    : null;
  const rows = projectServiceAccounts(accounts, appId).map((projected) => {
    const { account, contextApp, actionTarget } = projected;
    const keys = (["login", "provision"] as const).map((action) =>
      account.kind === "relay"
        ? `${action}:${account.id}`
        : `${action}:vendor:${account.id}`,
    );
    const busy = keys.some(activity.isBusy),
      error =
        keys.map(activity.errorOf).find((message) => message !== null) ?? null;
    return {
      key: projected.key,
      name: projected.name,
      icon: account.kind,
      kindLabel: t(`loongport.accounts.${account.kind}`),
      subtitle: [
        account.row.accountLabel,
        getAppDisplayName(contextApp, t),
        t(`loongport.accounts.status.${actionTarget.row.status}`),
      ]
        .filter(Boolean)
        .join(" · "),
      actionTarget,
      activity: busy
        ? { state: "busy" as const, error: null }
        : error
          ? { state: "error" as const, error }
          : null,
      openDetails: () =>
        setSelection({ kind: account.kind, id: account.id, appId: contextApp }),
      apps: projected.apps.map((id) => ({
        id,
        label: getAppDisplayName(id, t),
        open: () => onOpenApp(id),
      })),
      balance: account.row.canQueryBalance
        ? { rowKind: account.kind, rowId: account.id, enabled: true }
        : null,
    };
  });
  return {
    entry,
    detail,
    rows,
    isPending: inventory.isPending,
    error: inventory.error,
    reloadAccounts,
    back,
    openQoderConfiguration: () => onOpenApp("codex"),
    openSourceEntry: () => {
      if (entry.kind === "source")
        onOpenAddHub?.("accounts", { intent: entry.intent });
    },
  };
}

export type ServiceAccountDetailsViewModel = ReturnType<
  typeof useServiceAccountDetailsViewModel
>;
