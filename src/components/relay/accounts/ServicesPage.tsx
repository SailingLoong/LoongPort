import { DialogVisibility } from "@/components/ui/dialog";
import { useAccountPlatformPresence } from "./useAccountPlatformPresence";
import { QoderAccountPanel } from "./QoderAccountPanel";
import {
  AccountApplicationNavigation,
  AccountWorkspace,
  AccountWorkspaceTarget,
} from "./AccountWorkspace";
import {
  AccountPlatformTabs,
  AccountPlatformLogo,
} from "./AccountPlatformTabs";
import { AccountOverviewCard } from "./AccountOverviewCard";
import { usePreservedViewActive } from "@/components/ui/PreservedView";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import {
  serviceEntryIntent,
  type ServiceEntryIntent,
} from "./serviceEntryIntent";
import { AuthCenterPanel } from "@/components/settings/AuthCenterPanel";
import {
  accountPlatforms,
  accountPlatformLabel,
  type AccountPlatform,
} from "./accountPlatforms";
import { useContext, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  Loader2,
  Plus,
  RefreshCw,
  Server,
  ShieldCheck,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { AccountRoute } from "@/components/shell/navigation";
import { getAppDisplayName } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
import { RelaySection } from "../RelaySection";
import type { SubscriptionWindow } from "@/lib/api/applicationRouting";
import { formatResetAt } from "@/components/applications/tierMetrics";
import { usageApi } from "@/lib/api/usage";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ServiceSourceList, serviceSourcesKey } from "./ServiceSourceList";
import { fmtUsd } from "@/components/usage/format";
import { RowBalance } from "../RowBalance";
import { useRowBusy } from "../useRowBusy";
import {
  configuredApps,
  useServiceAccounts,
  type ServiceAccount,
} from "./useServiceAccounts";

export interface ServicesPageProps {
  appId: AppId;
  platform?: AccountPlatform;
  onOpenPlatform?: (platform: AccountPlatform | undefined) => void;
  onNavigationBlockedChange?: (blocked: boolean) => void;
  account?: AccountRoute;
  onBack?: () => void;
  onSelectAccount?: (account: AccountRoute | undefined, app: AppId) => void;
  onOpenAddHub: (
    tab: import("../AddHubPage").AddHubTab,
    opts?: { firstVisit?: boolean; intent?: ServiceEntryIntent },
  ) => void;
  onOpenApp: (appId: AppId) => void;
}

const accountName = (account: ServiceAccount) =>
  account.kind === "relay" ? account.row.siteName : account.row.vendorName;

/**
 * 账号卡的进行态/失败态（busy 状态源是模块级的，见 `useRowBusy`）：
 * 登录或导入任一在跑就算「正在导入」；都不跑但留有失败记录则显示失败行。
 * 返回 null = 没有任何值得占一行的状态。
 */
function useAccountActivity() {
  const { isBusy, errorOf } = useRowBusy();
  return (account: ServiceAccount) => {
    const keys = (["login", "provision"] as const).map((action) =>
      account.kind === "relay"
        ? `${action}:${account.id}`
        : `${action}:vendor:${account.id}`,
    );
    if (keys.some((key) => isBusy(key))) {
      return { state: "busy" as const, error: null };
    }
    const error =
      keys.map((key) => errorOf(key)).find((message) => message !== null) ??
      null;
    return error ? { state: "error" as const, error } : null;
  };
}

export function ServicesPage(props: ServicesPageProps) {
  const { t } = useTranslation();
  const viewActive = usePreservedViewActive();
  const addTrigger = useRef<HTMLButtonElement>(null);
  const returnAddFocus = useRef(false);
  const [localPlatform, setLocalPlatform] = useState<
    AccountPlatform | undefined
  >(props.platform);
  const [blocked, setBlocked] = useState(false);
  const [section, setSection] = useState("platform");
  const [filter, setFilter] = useState<AccountPlatform | undefined>();
  const [refreshing, setRefreshing] = useState(false);
  const [entered, setEntered] = useState<AccountPlatform[]>([]);
  const [targets, setTargets] = useState<
    Partial<Record<AccountPlatform, { id: string; request: number }>>
  >({});
  const {
    accounts: serviceAccounts,
    error: serviceAccountsError,
    reload,
  } = useServiceAccounts();
  const routedOpenCode =
    props.account?.kind === "vendor" &&
    serviceAccounts.some(
      (row) =>
        row.kind === "vendor" &&
        row.id === props.account?.id &&
        row.row.vendorId === "opencode",
    );
  const active =
    (props.onOpenPlatform ? props.platform : localPlatform) ??
    (routedOpenCode ? "opencode" : undefined);
  const select = (platform: AccountPlatform | undefined) => {
    if (blocked) return;
    if (props.onOpenPlatform) props.onOpenPlatform(platform);
    else setLocalPlatform(platform);
  };
  useEffect(() => {
    if (active)
      setEntered((previous) =>
        previous.includes(active) ? previous : [...previous, active],
      );
  }, [active]);
  const presence = useAccountPlatformPresence();
  const present = new Set(presence.data?.platforms ?? []);
  const unavailable = new Set(presence.data?.unavailable ?? []);
  if (serviceAccountsError) unavailable.add("opencode");
  if (
    serviceAccounts.some(
      (account) =>
        account.kind === "vendor" && account.row.vendorId === "opencode",
    )
  )
    present.add("opencode");
  const available = accountPlatforms
    .filter((item) => present.has(item.id))
    .map((item) => item.id);
  const filterMissing =
    filter !== undefined &&
    !presence.isPending &&
    !unavailable.has(filter) &&
    !available.includes(filter);
  useEffect(() => {
    if (filterMissing) setFilter(undefined);
  }, [filterMissing]);
  // Only visited managers stay mounted; switching views must not cancel authorization.
  const displayed = [...new Set([...entered, ...(active ? [active] : [])])];
  const openSummary = (platform: AccountPlatform, id: string) => {
    if (blocked) return;
    setTargets((previous) => ({
      ...previous,
      [platform]: { id, request: (previous[platform]?.request ?? 0) + 1 },
    }));
    select(platform);
  };
  const overviewAccounts = (presence.data?.accounts ?? []).filter(
    (row) =>
      row.source !== "workbuddy-inference" ||
      !presence.data?.accounts?.some(
        (other) => other.source === "workbuddy" && other.id === row.id,
      ),
  );
  const visiblePlatforms = filter
    ? available.filter((id) => id === filter)
    : available;
  const qoderRegions = [
    ...new Set(
      overviewAccounts
        .filter((row) => row.platform === "qoder")
        .map((row) => row.identity),
    ),
  ];
  const qoderEdition =
    qoderRegions.length === 1
      ? qoderRegions[0] === "CN"
        ? "CN"
        : qoderRegions[0] === "Global"
          ? "INTL"
          : undefined
      : undefined;
  const visibleCount =
    overviewAccounts.filter((row) => !filter || row.platform === filter)
      .length +
    (!filter || filter === "opencode"
      ? serviceAccounts.filter(
          (row) => row.kind === "vendor" && row.row.vendorId === "opencode",
        ).length
      : 0);
  const refresh = async () => {
    if (refreshing || blocked) return;
    setRefreshing(true);
    try {
      await Promise.all([presence.refetch(), reload()]);
    } finally {
      setRefreshing(false);
    }
  };
  useEffect(() => {
    if (active) setSection("platform");
    else if (props.account) setSection("relay");
  }, [active, props.account]);
  useEffect(() => {
    if (viewActive && returnAddFocus.current) {
      returnAddFocus.current = false;
      if (!active) addTrigger.current?.focus();
    }
  }, [viewActive, active]);
  return (
    <AccountApplicationNavigation.Provider
      value={() => props.onOpenApp(props.appId)}
    >
      <Tabs
        value={section}
        onValueChange={setSection}
        className="mx-auto w-full max-w-6xl space-y-5 px-6 py-5"
      >
        <TabsList
          aria-label={t("loongport.accounts.sourceType", {
            defaultValue: "来源类型",
          })}
          className="h-auto w-full justify-start gap-6 rounded-none border-b border-border bg-transparent p-0"
        >
          {[
            [
              "platform",
              t("loongport.accounts.platformAccounts", {
                defaultValue: "平台账号",
              }),
            ],
            [
              "relay",
              t("loongport.accounts.relayTab", { defaultValue: "中转站" }),
            ],
          ].map(([value, label]) => (
            <TabsTrigger
              key={value}
              value={value}
              disabled={blocked}
              className="rounded-none border-b-2 border-transparent px-1 pb-3 pt-0 text-sm font-medium text-muted-foreground data-[state=active]:border-primary data-[state=active]:bg-transparent data-[state=active]:text-primary data-[state=active]:shadow-none"
            >
              {label}
            </TabsTrigger>
          ))}
        </TabsList>
        <TabsContent
          value="platform"
          forceMount
          hidden={section !== "platform"}
          aria-label={t("loongport.accounts.platformAccounts", {
            defaultValue: "平台账号",
          })}
          className="space-y-4"
        >
          <AccountPlatformTabs
            active={filter}
            editions={{ qoder: qoderEdition }}
            platforms={available}
            disabled={blocked}
            onSelect={(value) => {
              if (blocked) return;
              setFilter(value);
              select(undefined);
            }}
          />
          <header className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex min-w-0 max-w-full items-center gap-2">
              {active && (
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={blocked}
                  onClick={() => select(undefined)}
                >
                  <ArrowLeft className="h-4 w-4" />
                  {t("loongport.accounts.backToOverview", {
                    defaultValue: "返回账号",
                  })}
                </Button>
              )}
              <h2 className="min-w-0 truncate text-base font-semibold">
                {filter
                  ? accountPlatformLabel(
                      accountPlatforms.find((item) => item.id === filter)!,
                    )
                  : t("loongport.accounts.allAccounts", {
                      defaultValue: "全部账号",
                    })}
              </h2>
              <span className="rounded bg-muted px-2 py-0.5 text-xs text-muted-foreground">
                {t("loongport.accounts.accountCount", {
                  defaultValue: "{{count}} 个账号",
                  count: visibleCount,
                })}
              </span>
            </div>
            <div className="flex max-w-full flex-wrap items-center gap-2">
              <Button
                variant="outline"
                className="h-10"
                aria-busy={refreshing || presence.isFetching}
                disabled={blocked || refreshing || presence.isFetching}
                onClick={() => void refresh()}
              >
                <RefreshCw
                  className={cn("h-4 w-4", refreshing && "animate-spin")}
                />
                {t("loongport.accounts.refreshStatus", {
                  defaultValue: "刷新状态",
                })}
              </Button>
              <Button
                ref={addTrigger}
                className="h-10"
                disabled={blocked}
                onClick={() => {
                  returnAddFocus.current = true;
                  props.onOpenAddHub("accounts");
                }}
              >
                <Plus className="h-4 w-4" />
                {t("loongport.accounts.addAccount", {
                  defaultValue: "添加账号",
                })}
              </Button>
            </div>
          </header>
          {presence.isPending && (
            <p role="status" className="text-sm text-muted-foreground">
              {t("loongport.accounts.loadingPlatforms", {
                defaultValue: "正在读取平台账号…",
              })}
            </p>
          )}
          {!!unavailable.size && (
            <p role="alert" className="text-sm text-muted-foreground">
              {t("loongport.accounts.partialPlatformRead", {
                defaultValue:
                  "部分平台账号读取失败，请稍后重试；已有账号未被删除。",
              })}
            </p>
          )}
          {!presence.isPending && !available.length && !unavailable.size && (
            <p className="text-sm text-muted-foreground">
              {t("loongport.accounts.emptyPlatforms", {
                defaultValue: "尚无已导入的平台账号，添加账号后将在这里显示。",
              })}
            </p>
          )}
          {!active &&
            visiblePlatforms.map((platform) => {
              const definition = accountPlatforms.find(
                (item) => item.id === platform,
              )!;
              const summaries = overviewAccounts.filter(
                (row) => row.platform === platform,
              );
              const identities = [
                ...new Set(
                  summaries.map((row) => row.identity).filter(Boolean),
                ),
              ];
              const groupEdition =
                "edition" in definition
                  ? definition.edition === "国际版"
                    ? "INTL"
                    : "CN"
                  : platform === "qoder" && identities.length === 1
                    ? identities[0]
                    : undefined;
              const vendors =
                platform === "opencode"
                  ? serviceAccounts.filter(
                      (row) =>
                        row.kind === "vendor" &&
                        row.row.vendorId === "opencode",
                    )
                  : [];
              return (
                <section
                  key={platform}
                  aria-label={`${accountPlatformLabel(definition)} · ${t("loongport.accounts.overview", { defaultValue: "账号总览" })}`}
                  className="space-y-3"
                >
                  <header className="flex items-center gap-2 pt-1">
                    <AccountPlatformLogo platform={definition} />
                    <h3 className="text-sm font-semibold">{definition.name}</h3>
                    {groupEdition && (
                      <span className="rounded bg-muted px-1 text-[10px] text-muted-foreground">
                        {groupEdition}
                      </span>
                    )}
                    <span className="text-sm text-muted-foreground">
                      {summaries.length + vendors.length}
                    </span>
                  </header>
                  {unavailable.has(platform) && (
                    <p role="status" className="text-xs text-muted-foreground">
                      {t("loongport.accounts.staleSummary", {
                        defaultValue:
                          "读取失败，显示上次读取的账号摘要；当前状态未知。",
                      })}
                    </p>
                  )}
                  <div className="grid grid-cols-1 gap-2.5 lg:grid-cols-2">
                    {summaries.map((row) => (
                      <AccountOverviewCard
                        key={`${row.source}:${row.app ?? ""}:${row.id}`}
                        account={row}
                        unavailable={unavailable.has(platform)}
                        disabled={blocked}
                        onOpen={() =>
                          openSummary(
                            platform,
                            row.app ? `${row.app}:${row.id}` : row.id,
                          )
                        }
                      />
                    ))}
                    {vendors.map((row) => (
                      <AccountOverviewCard
                        key={row.id}
                        account={{
                          source: "vendor",
                          platform,
                          id: String(row.id),
                          label: row.row.accountLabel || accountName(row),
                        }}
                        disabled={blocked}
                        onOpen={() => openSummary(platform, String(row.id))}
                      />
                    ))}
                  </div>
                </section>
              );
            })}
          {displayed.map((platform) => (
            <div
              key={platform}
              hidden={active !== platform}
              className="rounded-xl border border-border bg-card"
            >
              <DialogVisibility
                active={section === "platform" && active === platform}
              >
                <AccountWorkspaceTarget.Provider value={targets[platform]}>
                  <ServicesPageContent
                    {...props}
                    platform={platform}
                    onOpenPlatform={select}
                    onNavigationBlockedChange={(value) => {
                      setBlocked(value);
                      props.onNavigationBlockedChange?.(value);
                    }}
                  />
                  {platform === "openrouter" && (
                    <AccountWorkspace
                      accounts={(presence.data?.connections ?? []).map(
                        (connection) => ({
                          ...connection,
                          id: `${connection.app}:${connection.id}`,
                        }),
                      )}
                      label={(connection) => connection.name}
                    >
                      {(connection) => (
                        <article className="space-y-3">
                          <p className="text-sm text-muted-foreground">
                            {getAppDisplayName(connection.app, t)} · API Key
                          </p>
                          <Button
                            variant="outline"
                            onClick={() => props.onOpenApp(connection.app)}
                          >
                            {t("loongport.accounts.manageConnection", {
                              defaultValue: "管理原有连接",
                            })}
                          </Button>
                        </article>
                      )}
                    </AccountWorkspace>
                  )}
                  {platform === "opencode" && (
                    <ServicesPageContent
                      {...props}
                      platform={undefined}
                      accountKind="opencode"
                    />
                  )}
                </AccountWorkspaceTarget.Provider>
              </DialogVisibility>
            </div>
          ))}
        </TabsContent>
        <TabsContent
          value="relay"
          forceMount
          hidden={section !== "relay"}
          aria-label={t("loongport.accounts.relayAccounts", {
            defaultValue: "中转站与官方 API",
          })}
          className="space-y-4"
        >
          <h2 className="text-lg font-semibold">
            {t("loongport.accounts.relayAccounts", {
              defaultValue: "中转站与官方 API",
            })}
          </h2>
          <ServicesPageContent
            {...props}
            platform={undefined}
            accountKind="relay-and-vendor"
          />
        </TabsContent>
      </Tabs>
    </AccountApplicationNavigation.Provider>
  );
}

function ServicesPageContent({
  appId,
  platform,
  onNavigationBlockedChange,
  account,
  onBack,
  onSelectAccount,
  onOpenAddHub,
  onOpenApp,
  accountKind,
}: ServicesPageProps & { accountKind?: "opencode" | "relay-and-vendor" }) {
  const { t } = useTranslation();
  const target = useContext(AccountWorkspaceTarget);
  const {
    accounts: allAccounts,
    isPending,
    error,
    reload,
  } = useServiceAccounts();
  const accounts = allAccounts.filter((account) =>
    accountKind === "opencode"
      ? account.kind === "vendor" && account.row.vendorId === "opencode"
      : accountKind === "relay-and-vendor"
        ? account.kind !== "vendor" || account.row.vendorId !== "opencode"
        : true,
  );
  const queryClient = useQueryClient();
  const reloadAccounts = async (requireFresh = false) => {
    const [result] = await Promise.all([
      reload(),
      queryClient.invalidateQueries({ queryKey: serviceSourcesKey }),
    ]);
    // React Query refetch resolves an error result by default. Only callers
    // awaiting a just-saved configuration require a verified refresh result.
    if (requireFresh && result.error) throw result.error;
  };
  const activityOf = useAccountActivity();
  const [localSelection, setLocalSelection] = useState<{
    kind: ServiceAccount["kind"];
    id: number;
    appId: AppId;
  } | null>(null);
  const requestedSelection = onSelectAccount
    ? account
      ? { ...account, appId }
      : null
    : localSelection;
  const isOpenCodeSelection =
    requestedSelection &&
    allAccounts.some(
      (row) =>
        row.kind === "vendor" &&
        row.row.vendorId === "opencode" &&
        requestedSelection.kind === "vendor" &&
        row.id === requestedSelection.id,
    );
  const selection =
    requestedSelection &&
    (accountKind === "opencode"
      ? isOpenCodeSelection
      : accountKind === "relay-and-vendor"
        ? !isOpenCodeSelection
        : true)
      ? requestedSelection
      : null;
  const setSelection = (
    next: { kind: AccountRoute["kind"]; id: number; appId: AppId } | null,
  ) => {
    if (onSelectAccount)
      onSelectAccount(next ?? undefined, next?.appId ?? appId);
    else setLocalSelection(next);
  };
  useEffect(() => {
    if (target && accountKind === "opencode")
      setSelection({ kind: "vendor", id: Number(target.id), appId });
  }, [target]);
  const selected =
    selection &&
    accounts.find(
      (account) =>
        account.kind === selection.kind && account.id === selection.id,
    );

  const sourceIntent = platform ? serviceEntryIntent(platform) : undefined;
  if (platform === "qoder") {
    return (
      <section className="page-content space-y-4">
        <QoderAccountPanel />
        <Button variant="outline" onClick={() => onOpenApp("codex")}>
          管理原有 Qoder 配置
        </Button>
      </section>
    );
  }
  if (platform && sourceIntent) {
    return (
      <section className="page-content space-y-4">
        <h2 className="text-xl font-semibold">
          {accountPlatformLabel(
            accountPlatforms.find((item) => item.id === platform)!,
          )}
        </h2>
        <p className="text-sm text-muted-foreground">
          {platform === "openrouter"
            ? "API Key 连接，不代表平台账号签到或积分接入。"
            : "OpenCode 服务来源（Zen / Go / Free），与消费应用选择分开。"}
        </p>
        <Button
          variant="outline"
          onClick={() => onOpenAddHub("accounts", { intent: sourceIntent })}
        >
          添加服务连接
        </Button>
      </section>
    );
  }
  if (platform) {
    return (
      <section className="page-content space-y-6">
        <h2 className="text-xl font-semibold">
          {accountPlatformLabel(
            accountPlatforms.find((item) => item.id === platform)!,
          )}
        </h2>
        <AuthCenterPanel
          key={platform}
          platform={platform}
          editionScoped
          onNavigationBlockedChange={onNavigationBlockedChange}
        />
      </section>
    );
  }

  if (selection) {
    return (
      <section className="page-content space-y-6">
        {!onSelectAccount && (
          <Button
            variant="ghost"
            onClick={() => {
              if (onBack) onBack();
              else setSelection(null);
              void reloadAccounts();
            }}
          >
            <ArrowLeft className="h-4 w-4" />
            {t(onBack ? "common.back" : "loongport.accounts.back")}
          </Button>
        )}
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <p className="text-xs text-muted-foreground">
              {t(`loongport.accounts.${selection.kind}`)}
            </p>
            <h2 className="mt-1 text-xl font-semibold">
              {selected
                ? accountName(selected)
                : t("loongport.accounts.detail")}
            </h2>
            {selected?.row.accountLabel && (
              <p className="text-sm text-muted-foreground">
                {selected.row.accountLabel}
              </p>
            )}
          </div>
          {selected && (
            <div className="flex items-center gap-2">
              <Select
                value={selection.appId}
                onValueChange={(value) =>
                  setSelection({ ...selection, appId: value as AppId })
                }
              >
                <SelectTrigger
                  className="w-44"
                  aria-label={t("loongport.accounts.application")}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {[...selected.apps.keys()].map((app) => (
                    <SelectItem key={app} value={app}>
                      {getAppDisplayName(app, t)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button
                variant="outline"
                onClick={() => onOpenApp(selection.appId)}
              >
                {t("loongport.accounts.openApp")}
                <ArrowRight className="h-4 w-4" />
              </Button>
            </div>
          )}
        </div>
        {selected && (
          <ServiceSourceList
            account={{ kind: selected.kind, id: selected.id }}
            onOpenApp={onOpenApp}
          />
        )}
        {selected?.kind === "relay" && (
          <AccountUsageCard tiers={[...selected.apps.values()]} />
        )}
        {selected?.kind === "relay" && (
          <SubscriptionWindowsCard tiers={[...selected.apps.values()]} />
        )}
        <RelaySection
          key={`${selection.kind}:${selection.id}:${selection.appId}`}
          appId={selection.appId}
          accountActionsOnly
          accountFilter={{ kind: selection.kind, id: selection.id }}
          accountSnapshot={
            selected && selected.apps.has(selection.appId)
              ? selected.kind === "relay"
                ? {
                    kind: "relay",
                    row: selected.apps.get(selection.appId)!,
                    appId: selection.appId,
                  }
                : {
                    kind: "vendor",
                    row: selected.apps.get(selection.appId)!,
                    appId: selection.appId,
                  }
              : null
          }
          onOpenAddHub={onOpenAddHub}
          onAccountChanged={reloadAccounts}
        />
      </section>
    );
  }

  return (
    <RelaySection
      appId={appId}
      accountSnapshot={null}
      onOpenAddHub={onOpenAddHub}
      onAccountChanged={reloadAccounts}
      renderAccounts={(renderActions, renderDelete) => (
        <section className="page-content space-y-6">
          {isPending && (
            <p role="status" className="text-sm text-muted-foreground">
              {t("common.loading")}
            </p>
          )}
          {error && (
            <div
              role="alert"
              className="rounded-xl border border-destructive/30 p-4 text-sm"
            >
              <p>{t("loongport.accounts.loadFailed")}</p>
              <Button
                variant="outline"
                className="mt-3"
                onClick={() => void reloadAccounts()}
              >
                {t("common.refresh")}
              </Button>
            </div>
          )}
          {!isPending && !error && accounts.length === 0 && (
            <div className="rounded-xl border border-dashed p-10 text-center text-sm text-muted-foreground">
              {t("loongport.accounts.empty")}
            </div>
          )}
          <div className="grid gap-2">
            {accounts.map((account) => {
              const Icon = account.kind === "relay" ? Server : ShieldCheck;
              const apps = configuredApps(account);
              const contextApp = account.apps.has(appId)
                ? appId
                : account.apps.keys().next().value!;
              const row = account.apps.get(contextApp)!;
              const activity = activityOf(account);
              return (
                <article
                  key={`${account.kind}:${account.id}`}
                  className="group relative rounded-xl border border-border bg-card p-3 pr-12"
                >
                  {account.kind === "relay"
                    ? renderDelete({
                        kind: "relay",
                        row: account.apps.get(contextApp)!,
                        appId: contextApp,
                      })
                    : renderDelete({
                        kind: "vendor",
                        row: account.apps.get(contextApp)!,
                        appId: contextApp,
                      })}
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex min-w-0 items-center gap-2.5">
                      <div className="rounded-lg bg-muted p-2">
                        <Icon className="h-4 w-4 text-muted-foreground" />
                      </div>
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-2">
                          <h2 className="text-sm font-medium">
                            {accountName(account)}
                          </h2>
                          <span className="rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground">
                            {t(`loongport.accounts.${account.kind}`)}
                          </span>
                        </div>
                        <p className="mt-0.5 break-all text-xs text-muted-foreground">
                          {[
                            account.row.accountLabel,
                            getAppDisplayName(contextApp, t),
                            t(`loongport.accounts.status.${row.status}`),
                          ]
                            .filter(Boolean)
                            .join(" · ")}
                        </p>
                      </div>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-7"
                      onClick={() =>
                        setSelection({
                          kind: account.kind,
                          id: account.id,
                          appId: contextApp,
                        })
                      }
                    >
                      {t("loongport.accounts.detail")}
                      <ArrowRight className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                  {activity?.state === "busy" && (
                    <p
                      role="status"
                      className="mt-2 flex items-center gap-2 text-xs text-muted-foreground"
                    >
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      {t("loongport.accounts.importingTiers")}
                    </p>
                  )}
                  {activity?.state === "error" && (
                    <p
                      role="alert"
                      className="mt-2 flex items-start gap-2 text-xs text-destructive"
                    >
                      <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                      {t("loongport.accounts.importFailedLine", {
                        reason: activity.error ?? "",
                      })}
                    </p>
                  )}
                  <div className="mt-2">
                    {account.kind === "relay"
                      ? renderActions({
                          kind: "relay",
                          row: account.apps.get(contextApp)!,
                          appId: contextApp,
                        })
                      : renderActions({
                          kind: "vendor",
                          row: account.apps.get(contextApp)!,
                          appId: contextApp,
                        })}
                  </div>
                  {(apps.length > 0 || account.row.canQueryBalance) && (
                    <div className="mt-2 flex flex-wrap items-center justify-between gap-2 border-t border-border/60 pt-2">
                      <div className="flex flex-wrap gap-1">
                        {apps.map((app) => (
                          <Button
                            key={app}
                            variant="ghost"
                            size="sm"
                            className="h-6 px-2 text-xs"
                            onClick={() => onOpenApp(app)}
                          >
                            {getAppDisplayName(app, t)}
                            <ArrowRight className="ml-1 h-3 w-3" />
                          </Button>
                        ))}
                      </div>
                      {account.row.canQueryBalance && (
                        <RowBalance
                          rowKind={account.kind}
                          rowId={account.id}
                          enabled={account.row.canQueryBalance}
                        />
                      )}
                    </div>
                  )}
                </article>
              );
            })}
          </div>
        </section>
      )}
    />
  );
}

/**
 * 账号详情的「订阅限额与重置」区：聚合该账号全部档位的窗口。
 *
 * composite 拆档的多个档共享同一把 key ⇒ 窗口数组逐字节相同，按内容去重；
 * 非订阅账号（全部档位窗口为空）整区不渲染，不留空壳。
 */
function SubscriptionWindowsCard({
  tiers,
}: {
  tiers: { tiers: { subscriptionWindows: SubscriptionWindow[] }[] }[];
}) {
  const { t } = useTranslation();
  const windows = (() => {
    const seen = new Set<string>();
    const result: SubscriptionWindow[] = [];
    for (const row of tiers) {
      for (const tier of row.tiers) {
        for (const window of tier.subscriptionWindows) {
          const key = JSON.stringify(window);
          if (!seen.has(key)) {
            seen.add(key);
            result.push(window);
          }
        }
      }
    }
    return result.sort(
      (a, b) =>
        (a.resetAt ?? Number.MAX_SAFE_INTEGER) -
        (b.resetAt ?? Number.MAX_SAFE_INTEGER),
    );
  })();
  if (windows.length === 0) return null;
  return (
    <section className="rounded-xl border border-border bg-card">
      <h3 className="border-b border-border/60 px-4 py-3 text-sm font-medium">
        {t("loongport.accounts.subscriptionWindows")}
      </h3>
      <table className="w-full text-sm">
        <thead className="text-xs text-muted-foreground">
          <tr className="border-b border-border/60">
            <th scope="col" className="px-4 py-2 text-left font-medium">
              {t("loongport.accounts.windowColumn")}
            </th>
            <th scope="col" className="px-4 py-2 text-right font-medium">
              {t("loongport.accounts.usedLimitColumn")}
            </th>
            <th scope="col" className="px-4 py-2 text-right font-medium">
              {t("loongport.accounts.resetColumn")}
            </th>
          </tr>
        </thead>
        <tbody className="tabular-nums">
          {windows.map((window) => (
            <tr
              key={window.kind}
              className="border-b border-border/40 last:border-0"
            >
              <td className="px-4 py-2.5">
                {t(`loongport.accounts.windowKind.${window.kind}`)}
              </td>
              <td className="px-4 py-2.5 text-right">
                {window.usedUsd == null
                  ? `— / ${window.limitUsd.toFixed(2)}`
                  : `${window.usedUsd.toFixed(2)} / ${window.limitUsd.toFixed(2)}`}
              </td>
              <td className="px-4 py-2.5 text-right">
                {window.resetAt == null ? "—" : formatResetAt(window.resetAt)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

/**
 * 账号详情的「用量摘要」区：该账号全部档位近 7/30 天的花费与占比。
 *
 * 数据是一条按 provider id 的窗口聚合命令（明细 + 日汇总双表求和，与使用统计页
 * 同口径）；档位清单来自页面已有的账号快照，不在组件里反查归属。
 * 全部档位窗口内零花费 ⇒ 整区不渲染（新账号不背一张空表）。
 */
function AccountUsageCard({
  tiers,
}: {
  tiers: { tiers: { providerId: string; displayName: string }[] }[];
}) {
  const { t } = useTranslation();
  const entries = tiers.flatMap((row) =>
    row.tiers.map((tier) => ({
      providerId: tier.providerId,
      displayName: tier.displayName,
    })),
  );
  const { data } = useQuery({
    queryKey: ["account-usage", entries.map((entry) => entry.providerId)],
    queryFn: () =>
      usageApi.providersWindowCost(entries.map((e) => e.providerId)),
    staleTime: 60_000,
  });
  const byId = new Map((data ?? []).map((entry) => [entry.providerId, entry]));
  const rows = entries
    .map((entry) => ({
      ...entry,
      cost7: byId.get(entry.providerId)?.stats.costUsd7d ?? 0,
      cost30: byId.get(entry.providerId)?.stats.costUsd30d ?? 0,
    }))
    .filter((row) => row.cost30 > 0);
  if (rows.length === 0) return null;
  const total7 = rows.reduce((sum, row) => sum + row.cost7, 0);
  const total30 = rows.reduce((sum, row) => sum + row.cost30, 0);
  const max = Math.max(...rows.map((row) => row.cost30));
  return (
    <section className="rounded-xl border border-border bg-card">
      <div className="flex flex-wrap items-baseline justify-between gap-2 border-b border-border/60 px-4 py-3">
        <h3 className="text-sm font-medium">
          {t("loongport.accounts.usageTitle")}
        </h3>
        <p className="text-xs text-muted-foreground tabular-nums">
          {t("loongport.accounts.usageHeadline", {
            seven: fmtUsd(total7, 2),
            thirty: fmtUsd(total30, 2),
          })}
        </p>
      </div>
      <ul className="divide-y divide-border/40">
        {rows
          .slice()
          .sort((a, b) => b.cost30 - a.cost30)
          .map((row) => (
            <li
              key={row.providerId}
              className="flex items-center gap-3 px-4 py-2.5"
            >
              <span className="min-w-0 flex-1 truncate text-sm">
                {row.displayName}
              </span>
              <span
                className="h-1.5 rounded-full bg-primary/60"
                style={{
                  width: `${Math.max(4, Math.round((row.cost30 / max) * 100))}%`,
                }}
                aria-hidden="true"
              />
              <span className="w-16 text-right text-sm tabular-nums">
                {fmtUsd(row.cost30, 2)}
              </span>
            </li>
          ))}
      </ul>
    </section>
  );
}
