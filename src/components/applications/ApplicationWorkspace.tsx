import { ApplicationStartupCheckpoint } from "./ApplicationStartupCheckpoint";
import {
  useEffect,
  useImperativeHandle,
  useMemo,
  useState,
  type ReactNode,
  type Ref,
} from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  ChevronDown,
  ListFilter,
  Plus,
  Search,
  Settings2,
} from "lucide-react";
import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import type { ApplicationRoutingTier } from "@/lib/api/applicationRouting";
import type { AccountRoute } from "@/components/shell/navigation";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { VendorConnectionReviewDialog } from "@/components/relay/VendorConnectionReviewDialog";
import { SwitchTierConfirmDialog } from "@/components/relay/SwitchTierConfirmDialog";
import { useApplicationOverview } from "./useApplicationOverview";
import { useApplicationRouting } from "./useApplicationRouting";
import { ApplicationTierTable } from "./ApplicationTierTable";
import { TierVerificationProvider } from "@/components/relay/model-verification/TierVerificationProvider";
import {
  defaultDescending,
  sortTierIds,
  type TierMetric,
  type TierSort,
} from "./tierMetrics";

/** 模型可用性分数：分子着色（全可用绿 / 部分可用橙 / 全不可用灰，同 utilizationColor 语义），分母恒灰。 */
function ModelAvailability({
  available,
  total,
}: {
  available: number;
  total: number;
}) {
  const numerator =
    available === total
      ? "text-green-600 dark:text-green-400"
      : available > 0
        ? "text-orange-500 dark:text-orange-400"
        : "text-muted-foreground";
  return (
    <span className="flex shrink-0 items-baseline gap-0.5 tabular-nums">
      <span className={numerator}>{available}</span>
      <span className="text-muted-foreground">/{total}</span>
    </span>
  );
}

/** 档位工作台的筛选下拉：Popover + Command（cmdk），列表顶部自带输入筛选。
 *  模型目录（「分组支持」）和账号名单都可能很长，Radix Select 没有筛选能力
 *  （2026-09-21 用户点单）。触发器保持 ListFilter + 当前值 + 尾部箭头的原
 *  形状；「全部」恒为第一项（测试与既有交互都默认它在首位）。 */
function FilterCombobox({
  ariaLabel,
  triggerClassName,
  searchPlaceholder,
  noMatchText,
  allLabel,
  value,
  onChange,
  items,
  disabled = false,
}: {
  disabled?: boolean;
  ariaLabel: string;
  triggerClassName?: string;
  searchPlaceholder: string;
  noMatchText: string;
  allLabel: string;
  value: string | null;
  onChange: (value: string | null) => void;
  items: {
    value: string;
    label: string;
    content?: ReactNode;
    itemClassName?: string;
  }[];
}) {
  const [open, setOpen] = useState(false);
  const selected = items.find((item) => item.value === value);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          disabled={disabled}
          role="combobox"
          aria-expanded={open}
          aria-label={ariaLabel}
          className={cn("justify-between font-normal", triggerClassName)}
        >
          <span className="flex min-w-0 items-center gap-2">
            <ListFilter className="h-4 w-4 shrink-0 text-muted-foreground" />
            <span className="truncate">
              {selected ? selected.label : allLabel}
            </span>
          </span>
          <ChevronDown className="h-4 w-4 shrink-0 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        sideOffset={4}
        collisionPadding={8}
        className="w-72 p-0"
      >
        <Command label={ariaLabel}>
          <CommandInput placeholder={searchPlaceholder} />
          <CommandList>
            <CommandEmpty>{noMatchText}</CommandEmpty>
            <CommandItem
              value="all"
              onSelect={() => {
                onChange(null);
                setOpen(false);
              }}
            >
              <Check
                className={cn(
                  "h-4 w-4 shrink-0",
                  value ? "opacity-0" : "opacity-100",
                )}
              />
              {allLabel}
            </CommandItem>
            {items.map((item) => (
              <CommandItem
                key={item.value}
                value={item.label}
                className={item.itemClassName}
                onSelect={() => {
                  onChange(item.value);
                  setOpen(false);
                }}
              >
                <Check
                  className={cn(
                    "h-4 w-4 shrink-0",
                    value === item.value ? "opacity-100" : "opacity-0",
                  )}
                />
                {item.content ?? item.label}
              </CommandItem>
            ))}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

export interface ApplicationWorkspaceHandle {
  hasPendingChanges: boolean;
  submitting: boolean;
  discard: () => void;
}

interface Props {
  ref?: Ref<ApplicationWorkspaceHandle>;
  appId: AppId;
  providers: Record<string, Provider>;
  onSwitchProvider: (provider: Provider) => void | Promise<void>;
  onOpenAccount: (account: AccountRoute) => void;
  onAdd: () => void;
  children: ReactNode | ((mutationsDisabled: boolean) => ReactNode);
}
/** 档位的可筛模型：模型目录优先（「分组支持」语义），无目录回落当前生效模型。
 *  选项聚合与下拉显隐共用这一份——两处各写一遍必然分叉。 */
function supportedModels(tier: ApplicationRoutingTier): string[] {
  return tier.models.length
    ? tier.models
    : [tier.effectiveModel].filter((model): model is string => Boolean(model));
}
export function ApplicationWorkspace({
  ref,
  appId,
  providers,
  onSwitchProvider,
  onOpenAccount,
  onAdd,
  children,
}: Props) {
  const { t } = useTranslation();
  const model = useApplicationOverview(appId, providers, onSwitchProvider);
  const routing = useApplicationRouting(appId);
  const [managing, setManaging] = useState(false);
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState<TierSort | null>(null);
  const [accountFilter, setAccountFilter] = useState<string | null>(null);
  const [modelFilter, setModelFilter] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [order, setOrder] = useState<{
    source: typeof routing.data;
    ids: string[];
  } | null>(null);
  const configurations = model.data?.configurations ?? [];
  // 引用稳定性：tiers 下游有效应依赖链（验真 summaries 重拉），identity 必须只随
  // routing.data 变 —— `?? []` 每渲染换新引用会在加载期造成「effect→setState→渲染」循环。
  const tiers = useMemo(() => routing.data?.tiers ?? [], [routing.data]);
  // 验真 Provider 的档位集：随 routing 数据换引用（数据变 → 重拉 summaries；
  // 数据不变 → 引用稳定，不触发多余请求）。与 RelaySection 同一形状。
  const verificationProviderIds = useMemo(
    () => [...new Set(tiers.map((tier) => tier.providerId))],
    [tiers],
  );
  const ids = new Set(configurations.map((item) => item.providerId));
  const ranked = tiers
    .map((tier) => tier.providerId)
    .filter((id) => ids.has(id));
  const rankedSet = new Set(ranked);
  const storedIds = [
    ...ranked,
    ...configurations
      .filter((item) => !rankedSet.has(item.providerId))
      .map((item) => item.providerId),
  ];
  const failoverEnabled = Boolean(routing.data?.autoFailoverEnabled);
  // Managed proxy Apps use ApplicationRouteWorkspace. Remaining Apps persist
  // their native display order immediately; filters and metric sort stay local.
  const baseIds =
    order && order.source === routing.data ? order.ids : storedIds;
  // 视图排序只重排展示，不落库；默认序 = 数据库档位序（拖拽维护）。
  const orderedIds = sort ? sortTierIds(baseIds, tiers, sort) : baseIds;
  const changeOrder = async (next: string[]) => {
    if (mutationBusy) return;
    const previousOrder = order;
    setSaving(true);
    setOrder({ source: routing.data, ids: next });
    try {
      await routing.setOrder(next);
    } catch {
      setOrder(previousOrder);
    } finally {
      setSaving(false);
    }
  };
  useImperativeHandle(ref, () => ({
    hasPendingChanges: false,
    // Native writes still block leaving until their existing lifecycle settles.
    submitting: saving || routing.busy || model.busy,
    discard: () => {
      setSearch("");
      setSort(null);
      setAccountFilter(null);
      setModelFilter(null);
    },
  }));
  // 同一指标：默认向 → 反向 → 取消（回到数据库档位序）；换指标：旧排序就地取消。
  const sortBy = (key: TierMetric) => {
    if (sort?.key !== key) {
      setSort({ key, descending: defaultDescending(key) });
      return;
    }
    if (sort.descending === defaultDescending(key)) {
      setSort({ key, descending: !sort.descending });
    } else {
      setSort(null);
    }
  };
  const accounts = useMemo(() => {
    const byKey = new Map<string, { key: string; label: string }>();
    for (const item of configurations) {
      if (!item.account) continue;
      const key = `${item.account.kind}:${item.account.id}`;
      const label =
        [item.serviceName, item.accountLabel].filter(Boolean).join(" · ") ||
        key;
      if (!byKey.has(key)) byKey.set(key, { key, label });
    }
    return [...byKey.values()].sort((a, b) => a.label.localeCompare(b.label));
  }, [configurations]);
  // 模型筛选的选项 = 各档位**模型目录**的并集（「分组支持」语义——一个档位
  // 通常支持多个模型，别只认当前在用的那个）；无目录的档位回落单模型
  // （effectiveModel）。聚合范围跟随账号筛选——选了账号后分数必须只数该账号
  // 的档位，不能仍报全量（2026-09-17 用户报的 bug：7 档筛剩 5，模型分数仍
  // 是 7/7）。
  // 当前路由模型置顶加 ⚡，故障切换场景「选模型 → 过滤看链上还有谁」一步到位。
  // 每个模型带 可用/总数 分数：不可用只算「已嗅探到的错误」（circuit_open=
  // 限流/网络/余额等真实失败累积触发的熔断）与用户屏蔽；模型不匹配/能力声明/
  // 位置语义一概不扣——它们对别的模型恒真，不是错误（2026-09-16 用户定调）。
  // 全不可用的模型置灰但可选（选完正好逐行看原因）。
  const tierModels = useMemo(() => {
    const accountMatch = (item: (typeof configurations)[number]) =>
      !accountFilter ||
      (item.account &&
        `${item.account.kind}:${item.account.id}` === accountFilter);
    const byId = new Map(
      tiers.map((tier) => [
        tier.providerId,
        {
          models: supportedModels(tier),
          available:
            tier.skipReason !== "circuit_open" && tier.skipReason !== "blocked",
        },
      ]),
    );
    const stats = new Map<
      string,
      { model: string; available: number; total: number }
    >();
    for (const item of configurations) {
      if (!accountMatch(item)) continue;
      const tier = byId.get(item.providerId);
      // 目录之外的回显模型（item.model）也算支持——目录可能滞后于实际配置。
      const models = new Set(tier?.models ?? []);
      if (item.model) models.add(item.model);
      for (const model of models) {
        const entry = stats.get(model) ?? { model, available: 0, total: 0 };
        entry.total += 1;
        if (tier?.available ?? true) entry.available += 1;
        stats.set(model, entry);
      }
    }
    const routingModel = routing.data?.model;
    return [...stats.values()]
      .sort((a, b) => a.model.localeCompare(b.model))
      .sort((a, b) =>
        a.model === routingModel ? -1 : b.model === routingModel ? 1 : 0,
      );
  }, [configurations, accountFilter, tiers, routing.data?.model]);
  // 账号筛选收窄后，已选模型可能不在选项里（该账号没有这个模型）——
  // 留着会让触发器显示空值、可见行被滤成零；清掉回到该账号全量视图。
  useEffect(() => {
    if (
      modelFilter &&
      !tierModels.some((option) => option.model === modelFilter)
    ) {
      setModelFilter(null);
    }
  }, [tierModels, modelFilter]);
  // 模型下拉的显隐看**全量**模型数（目录并集）：账号筛选收窄到单模型时下拉
  // 仍要可见——分数（该账号可用/总数）本身就是用户要看的信息。
  const hasMultipleModelsOverall = useMemo(() => {
    const byId = new Map(
      tiers.map((tier) => [tier.providerId, supportedModels(tier)]),
    );
    const models = new Set<string>();
    for (const item of configurations) {
      for (const model of byId.get(item.providerId) ?? []) models.add(model);
      if (item.model) models.add(item.model);
    }
    return models.size > 1;
  }, [configurations, tiers]);
  useEffect(() => {
    if (
      !model.isPending &&
      !model.error &&
      accountFilter &&
      !accounts.some((account) => account.key === accountFilter)
    )
      setAccountFilter(null);
  }, [accounts, accountFilter, model.isPending, model.error, setAccountFilter]);
  const routingModel = routing.data?.model ?? null;
  const orderBusy = saving || routing.busy || model.busy;
  const ownerBlocked =
    routing.modeState?.canWrite === false ||
    Boolean(routing.data && routing.writeBlocked && !routing.error);
  const writesBlocked =
    Boolean(routing.writeBlocked) ||
    ownerBlocked ||
    model.isPending ||
    Boolean(model.error) ||
    routing.isPending ||
    Boolean(routing.error);
  const mutationBusy = orderBusy || writesBlocked;
  const failedReadTimes = [
    ...(model.error ? [model.dataUpdatedAt] : []),
    ...(routing.error || ownerBlocked ? [routing.dataUpdatedAt] : []),
  ];
  const lastReadTime =
    failedReadTimes.length && failedReadTimes.every((time) => time && time > 0)
      ? Math.min(...(failedReadTimes as number[]))
      : 0;
  const lastRead = lastReadTime
    ? new Date(lastReadTime).toLocaleString()
    : undefined;
  return (
    <div className="space-y-5">
      <section className="space-y-4" aria-labelledby={`tiers-${appId}`}>
        <header className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <div className="flex flex-wrap items-center gap-x-5 gap-y-3">
              <h2 id={`tiers-${appId}`} className="text-lg font-semibold">
                {t("applications.availableTiers")}
                <span className="ml-2 text-sm font-normal tabular-nums text-muted-foreground">
                  {configurations.length}
                </span>
              </h2>
              {routing.modeState?.status === "ready" &&
                routing.modeState.mode !== null &&
                routing.modeState.writeProtocol === "native" &&
                !routing.error && (
                  <span className="text-xs text-muted-foreground">
                    {t(
                      routing.modeState.mode === "direct"
                        ? "applications.routingModeDirect"
                        : routing.modeState.attached
                          ? "applications.routingModeProxy"
                          : "applications.routingModeDetached",
                    )}
                  </span>
                )}
            </div>
            <p className="mt-2 text-xs leading-5 text-muted-foreground">
              {t("applications.orderHint")}
            </p>
          </div>
          <Button variant="outline" onClick={onAdd}>
            <Plus className="h-4 w-4" />
            {t("applications.addService")}
          </Button>
        </header>
        <div className="flex flex-wrap items-center gap-3">
          {accounts.length > 1 && (
            <FilterCombobox
              disabled={orderBusy}
              ariaLabel={t("applications.accountFilter")}
              triggerClassName="w-48"
              searchPlaceholder={t("applications.accountFilterPlaceholder")}
              noMatchText={t("applications.filterNoMatch")}
              allLabel={t("applications.allAccounts")}
              value={accountFilter}
              onChange={setAccountFilter}
              items={accounts.map((account) => ({
                value: account.key,
                label: account.label,
              }))}
            />
          )}
          {hasMultipleModelsOverall && (
            <FilterCombobox
              disabled={orderBusy}
              ariaLabel={t("applications.modelFilter")}
              triggerClassName="w-64"
              searchPlaceholder={t("applications.modelFilterPlaceholder")}
              noMatchText={t("applications.filterNoMatch")}
              allLabel={t("applications.allModels")}
              value={modelFilter}
              onChange={setModelFilter}
              items={tierModels.map((option) => ({
                value: option.model,
                label: option.model,
                itemClassName:
                  option.available === 0 ? "text-muted-foreground" : undefined,
                content: (
                  <span className="flex min-w-0 flex-1 items-center gap-2">
                    <span className="truncate">
                      {option.model === routingModel
                        ? `⚡ ${option.model}`
                        : option.model}
                    </span>
                    <ModelAvailability
                      available={option.available}
                      total={option.total}
                    />
                  </span>
                ),
              }))}
            />
          )}
          <div className="relative min-w-60 max-w-md flex-1">
            <Search className="pointer-events-none absolute left-3 top-3 h-4 w-4 text-muted-foreground" />
            <Input
              disabled={orderBusy}
              type="search"
              aria-label={t("applications.search")}
              placeholder={t("applications.search")}
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              className="pl-9"
            />
          </div>
        </div>
        {(model.isPending || routing.isPending) && (
          <p role="status" className="text-sm text-muted-foreground">
            {t("common.loading")}
          </p>
        )}
        {ownerBlocked && (
          <div
            role="alert"
            className="space-y-2 rounded-lg border border-destructive/30 p-3 text-sm"
          >
            <p>{t("applications.operationNeedsVerification")}</p>
            <ApplicationStartupCheckpoint
              appId={appId}
              modeState={routing.modeState}
              onRefresh={() =>
                Promise.all([model.refetch(), routing.refetch()])
              }
            />
            {routing.modeState?.publicationStarted === true && (
              <p>{t("applications.operationPublicationStarted")}</p>
            )}
            {lastRead && (
              <p className="text-xs text-muted-foreground">
                {t("applications.lastSuccessfulRead", { time: lastRead })}
              </p>
            )}
            {!routing.modeState?.blockedReason?.startsWith("upgrade.") && (
              <Button
                variant="outline"
                onClick={() => {
                  void model.refetch();
                  void routing.refetch();
                }}
              >
                {t("applications.recheckOperation")}
              </Button>
            )}
          </div>
        )}
        {!ownerBlocked && (model.error || routing.error) && (
          <div
            role="alert"
            className="flex items-center justify-between gap-3 rounded-lg border border-destructive/30 p-3 text-sm"
          >
            <div>
              <span>{t("applications.loadFailed")}</span>
              {lastRead && (
                <p className="text-xs text-muted-foreground">
                  {t("applications.lastSuccessfulRead", { time: lastRead })}
                </p>
              )}
            </div>
            <Button
              variant="outline"
              onClick={() => {
                void model.refetch();
                void routing.refetch();
              }}
            >
              {t("common.retry")}
            </Button>
          </div>
        )}
        {/* 模型验证的行级宿主：summaries 拉取、验真弹窗与结果变化订阅全在
            Provider 内部；下线时对外不可见（不拉取、入口/徽章不渲染）。 */}
        <TierVerificationProvider
          appId={appId}
          providerIds={verificationProviderIds}
        >
          <ApplicationTierTable
            configurations={configurations}
            tiers={tiers}
            orderedIds={orderedIds}
            failoverEnabled={failoverEnabled}
            orderPending={false}
            reorderEnabled
            loading={model.isPending || routing.isPending}
            failed={Boolean(model.error || routing.error)}
            search={search}
            accountFilter={accountFilter}
            modelFilter={modelFilter}
            additive={model.data?.isAdditive ?? false}
            busy={mutationBusy}
            orderBusy={orderBusy}
            sort={sort}
            onSort={sortBy}
            onReorder={(next) => {
              void changeOrder(next);
            }}
            onSelect={(item) => {
              if (mutationBusy) return;
              // 模型筛选下点「设为当前」：档位与筛选模型一次切过去（用户视角
              // 就是「用这个档位跑这个模型」）；未筛选时纯切档位。
              void model.select(item, undefined, modelFilter ?? undefined);
            }}
            onOpenAccount={onOpenAccount}
            onBlockTier={(providerId, blocked) => {
              void routing
                .blockTier({ providerId, blocked })
                .catch(() => undefined);
            }}
            onResetTierErrors={(providerId) => {
              void routing
                .resetTierErrors({ providerId })
                .catch(() => undefined);
            }}
          />
        </TierVerificationProvider>
        <p className="text-xs text-muted-foreground">
          {t("applications.metricsHint")}
        </p>
      </section>
      <Collapsible
        open={managing}
        onOpenChange={setManaging}
        className="rounded-xl border border-border bg-card p-4"
      >
        <CollapsibleTrigger asChild>
          <Button variant="ghost" className="-ml-2">
            <Settings2 className="h-4 w-4" />
            {t("applications.manageConfigurations")}
            <ChevronDown className="h-4 w-4" />
          </Button>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <p className="mb-5 mt-2 text-sm text-muted-foreground">
            {t("applications.manageDescription")}
          </p>
          {typeof children === "function" ? children(mutationBusy) : children}
        </CollapsibleContent>
      </Collapsible>
      <VendorConnectionReviewDialog
        targetName={model.vendorRecovery?.target.name ?? null}
        disabled={writesBlocked || routing.busy || model.busy}
        onCancel={model.cancelVendorRecovery}
        onConfirm={() => {
          if (!writesBlocked && !routing.busy)
            return model.confirmVendorRecovery();
        }}
      />
      <SwitchTierConfirmDialog
        targetName={model.confirmation}
        onCancel={model.cancel}
        catalog={model.catalog}
        quitRequired={model.quitRequired}
        onPreviewCatalog={(decision) => {
          if (!writesBlocked) return model.previewCatalog(decision);
        }}
        disabled={writesBlocked || routing.busy || model.busy}
        onSwitch={(quit, decision) => {
          if (!writesBlocked) return model.confirm(quit, decision);
        }}
      />
    </div>
  );
}
