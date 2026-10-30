import { SERVICES_CONTENT_CLASS_NAME } from "@/components/shell/layout";
import { DialogVisibility } from "@/components/ui/dialog";
import { useAccountsOverviewViewModel } from "./useAccountsOverviewViewModel";
import { AccountOverviewGroup } from "./AccountOverviewGroup";
import { AccountOrderSurface } from "./AccountOrderSurface";
import { ServiceAccountDetailsView as ServicesPageContent } from "./ServiceAccountDetailsView";
import {
  AccountApplicationNavigation,
  AccountWorkspace,
  AccountWorkspaceTarget,
} from "./AccountWorkspace";
import { AccountPlatformTabs } from "./AccountPlatformTabs";
import { AutomaticClaimControls } from "./AutomaticClaimControls";
import { usePreservedViewActive } from "@/components/ui/PreservedView";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import type { ServiceEntryIntent } from "./serviceEntryIntent";
import {
  accountPlatforms,
  accountPlatformLabel,
  type AccountPlatform,
} from "./accountPlatforms";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Plus, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import type { AccountRoute } from "@/components/shell/navigation";
import { getAppDisplayName } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
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

export function ServicesPage(props: ServicesPageProps) {
  const { t } = useTranslation();
  const viewActive = usePreservedViewActive();
  const addTrigger = useRef<HTMLButtonElement>(null);
  const returnAddFocus = useRef(false);
  const addReturnTarget = useRef<HTMLElement | null>(null);
  const openAddHub: ServicesPageProps["onOpenAddHub"] = (tab, options) => {
    addReturnTarget.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    returnAddFocus.current = true;
    if (options) props.onOpenAddHub(tab, options);
    else props.onOpenAddHub(tab);
  };
  const [localPlatform, setLocalPlatform] = useState<
    AccountPlatform | undefined
  >(props.platform);
  const [blocked, setBlocked] = useState(false);
  const [section, setSection] = useState("platform");
  const [claimBusy, setClaimBusy] = useState(false);
  const [claimUnconfirmed, setClaimUnconfirmed] = useState<
    Record<string, string>
  >({});
  const [targets, setTargets] = useState<
    Partial<Record<AccountPlatform, { id: string; request: number }>>
  >({});
  const overview = useAccountsOverviewViewModel({
    activeManager: props.onOpenPlatform ? props.platform : localPlatform,
    accountRoute: props.account,
    blocked: blocked || claimBusy,
    automaticUnconfirmed: claimUnconfirmed,
    onOpenDetails: (target) => openSummary(target.platform, target.id),
  });
  const {
    presence,
    activeManager: active,
    selectedFilter: filter,
    refreshing,
    refresh,
    unavailablePlatforms: unavailable,
    scopeAccounts: visibleAccounts,
    managerIds: displayed,
  } = overview;
  const available = overview.orderedGroups.map((group) => group.id);
  const qoderEdition = overview.orderedGroups.find(
    (group) => group.id === "qoder",
  )?.edition;
  const visibleCount = overview.counts.scope;
  const select = (platform: AccountPlatform | undefined) => {
    if (blocked) return;
    if (props.onOpenPlatform) props.onOpenPlatform(platform);
    else setLocalPlatform(platform);
  };
  const openSummary = (platform: AccountPlatform, id: string) => {
    if (blocked) return;
    setTargets((previous) => ({
      ...previous,
      [platform]: { id, request: (previous[platform]?.request ?? 0) + 1 },
    }));
    select(platform);
  };
  useEffect(() => {
    if (active) setSection("platform");
    else if (props.account) setSection("relay");
  }, [active, props.account]);
  useEffect(() => {
    if (viewActive && returnAddFocus.current) {
      returnAddFocus.current = false;
      const target = addReturnTarget.current;
      if (target?.isConnected) target.focus();
      else addTrigger.current?.focus();
    }
  }, [viewActive, active]);
  return (
    <AccountApplicationNavigation.Provider
      value={(targetApp = props.appId) => props.onOpenApp(targetApp)}
    >
      <Tabs
        value={section}
        onValueChange={setSection}
        className={cn(SERVICES_CONTENT_CLASS_NAME, "space-y-5 py-5")}
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
              disabled={blocked || claimBusy}
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
            orderItems={overview.orderItems}
            onReorder={overview.reorderGroup}
            disabled={blocked || claimBusy}
            onSelect={(value) => {
              if (blocked) return;
              overview.setFilter(value);
              select(undefined);
            }}
          />
          <header className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex min-w-0 max-w-full items-center gap-2">
              {active && (
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={blocked || claimBusy}
                  onClick={() => select(undefined)}
                >
                  <ArrowLeft className="h-4 w-4" />
                  {t("loongport.accounts.backToOverview", {
                    defaultValue: "返回账号",
                  })}
                </Button>
              )}
              <h2 className="min-w-0 truncate text-lg font-semibold">
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
                disabled={
                  blocked || claimBusy || refreshing || presence.isFetching
                }
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
                disabled={blocked || claimBusy}
                onClick={() => {
                  openAddHub("accounts");
                }}
              >
                <Plus className="h-4 w-4" />
                {t("loongport.accounts.addAccount", {
                  defaultValue: "添加账号",
                })}
              </Button>
            </div>
          </header>
          <label className="flex items-center gap-2 text-sm">
            <Checkbox
              checked={overview.preferences.hideExhausted}
              disabled={blocked || claimBusy}
              onCheckedChange={overview.setHideExhausted}
            />
            {t("accountsOverview.hideExhausted")}
          </label>
          {overview.preferencePersistence === "sessionOnly" && (
            <p role="status" className="text-xs text-muted-foreground">
              {t("accountsOverview.sessionOnly")}
            </p>
          )}
          {overview.refreshReadFailed && (
            <p role="alert" className="text-sm text-muted-foreground">
              {t("loongport.accounts.partialPlatformRead")}
            </p>
          )}
          <AutomaticClaimControls
            accounts={visibleAccounts}
            scope={
              filter
                ? accountPlatformLabel(
                    accountPlatforms.find((item) => item.id === filter)!,
                  )
                : t("loongport.accounts.allAccounts", {
                    defaultValue: "全部账号",
                  })
            }
            unavailable={
              filter
                ? new Set(
                    [...unavailable].filter((platform) => platform === filter),
                  )
                : unavailable
            }
            disabled={
              blocked || refreshing || presence.isFetching || presence.isPending
            }
            onRefresh={presence.refetch}
            onBusyChange={setClaimBusy}
            unconfirmed={claimUnconfirmed}
            onUnconfirmed={(issues) =>
              setClaimUnconfirmed((previous) => ({ ...previous, ...issues }))
            }
          />
          {overview.loading && (
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
          {!overview.loading && !available.length && !unavailable.size && (
            <p className="text-sm text-muted-foreground">
              {t("loongport.accounts.emptyPlatforms", {
                defaultValue: "尚无已导入的平台账号，添加账号后将在这里显示。",
              })}
            </p>
          )}
          {!active && (
            <AccountOrderSurface
              items={overview.orderItems}
              onReorder={overview.reorderGroup}
              disabled={blocked || claimBusy}
            >
              {overview.orderedGroups.map((group) => (
                <AccountOverviewGroup
                  key={group.id}
                  group={group}
                  orderItem={overview.orderItems.find(
                    (item) => item.id === group.id,
                  )!}
                  disabled={blocked || claimBusy}
                  onReorder={overview.reorderGroup}
                  onToggle={() => overview.toggleCollapsed(group.id)}
                  onSort={(sort) => overview.setGroupSort(group.id, sort)}
                  onOpenDetails={overview.openDetails}
                />
              ))}
            </AccountOrderSurface>
          )}
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
                    onOpenAddHub={openAddHub}
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
                      onOpenAddHub={openAddHub}
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
