import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  Check,
  ChevronDown,
  CircleHelp,
  ListFilter,
  Plus,
  Search,
  Settings2,
} from "lucide-react";
import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import type { AccountRoute } from "@/components/shell/navigation";
import { isProxyAppId } from "@/config/appConfig";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { SwitchTierConfirmDialog } from "@/components/relay/SwitchTierConfirmDialog";
import { orderProfilesApi } from "@/lib/api/orderProfiles";
import { extractErrorMessage } from "@/utils/errorUtils";
import { useApplicationOverview } from "./useApplicationOverview";
import { useApplicationRouting } from "./useApplicationRouting";
import { ApplicationTierTable, visibleTierIds } from "./ApplicationTierTable";
import { OrderProfilesMenu } from "./OrderProfilesMenu";
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

interface Props {
  appId: AppId;
  providers: Record<string, Provider>;
  onSwitchProvider: (provider: Provider) => void | Promise<void>;
  onOpenAccount: (account: AccountRoute) => void;
  onAdd: () => void;
  children: ReactNode;
}
export function ApplicationWorkspace({
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
  // This is an optimistic UI snapshot, replaced by the next backend result.
  const [order, setOrder] = useState<{
    source: typeof routing.data;
    ids: string[];
  } | null>(null);
  // 顺序草稿（路由类应用）：拖拽只改这里；载入配置档会整体替换成档内 id 集
  // （不垫底——链外档位从视图消失，应用后即出链）；点「应用」才写库。
  const [stagedIds, setStagedIds] = useState<string[] | null>(null);
  const configurations = model.data?.configurations ?? [];
  const tiers = routing.data?.tiers ?? [];
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
  // 路由类应用的顺序调整（拖拽/排序）只进草稿，点「应用」才写库；非路由类应用
  // （无故障切换概念）维持拖拽即时保存。
  const draftOrdering = isProxyAppId(appId);
  // 链编辑只在故障切换开启时存在（2026-09-17 用户定调）：关着时顺序与配置档
  // 没有任何运行时作用，整套编辑面（应用/取消/配置档/优先级列/拖拽/屏蔽）不出现。
  // 筛选/搜索/指标排序保留——纯查看，与链无关。
  const chainEditing = draftOrdering && failoverEnabled;
  // 故障切换一关就丢弃未应用的草稿：不留幽灵待应用，重开时从存储链干净起步。
  useEffect(() => {
    if (draftOrdering && !failoverEnabled) {
      setStagedIds(null);
      setSort(null);
    }
  }, [draftOrdering, failoverEnabled]);
  const baseIds =
    stagedIds ??
    (order && order.source === routing.data ? order.ids : storedIds);
  // 视图排序只重排展示，不落库；默认序 = 数据库档位序（拖拽维护）。
  const orderedIds = sort ? sortTierIds(baseIds, tiers, sort) : baseIds;
  const changeOrder = async (next: string[]) => {
    if (saving || routing.busy) return;
    if (draftOrdering) {
      setStagedIds(next);
      return;
    }
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
  // 应用目标 = 当前可见 ∧ 未屏蔽的档位，按显示序（2026-09-16 用户定调：
  // 筛选一变目标就变，不需要先拖一下；可见性判定与表格共用 visibleTierIds）。
  // 应用写入的就是它——链外档位不是后备，上游删掉的幽灵由应用清理。
  const blockedIds = new Set(
    tiers
      .filter((tier) => tier.skipReason === "blocked")
      .map((tier) => tier.providerId),
  );
  const targetIds = visibleTierIds({
    orderedIds,
    configurations: new Map(
      configurations.map((item) => [item.providerId, item]),
    ),
    metrics: new Map(tiers.map((tier) => [tier.providerId, tier])),
    search,
    accountFilter,
    modelFilter,
  }).filter((id) => !blockedIds.has(id));
  // 参照 = 存储链去掉被屏蔽的 id（幽灵保留——上游有删减时按钮亮起，应用即清理；
  // 屏蔽是即时生效的显式动作，单独屏蔽不制造待应用）。
  const referenceIds = (routing.data?.chainIds ?? storedIds).filter(
    (id) => !blockedIds.has(id),
  );
  const matchesAppliedChain =
    targetIds.length === referenceIds.length &&
    targetIds.every((id, index) => id === referenceIds[index]);
  // 待应用计数 = 应用目标的大小（「链里将有几个」），视图与已应用链一致时为 0。
  const pendingOrderCount =
    chainEditing && targetIds.length > 0 && !matchesAppliedChain
      ? targetIds.length
      : 0;
  // 配置档状态（列表 + 当前配置文件名）：应用此顺序默认保存进当前档。
  const client = useQueryClient();
  const { data: profilesState } = useQuery({
    queryKey: ["orderProfiles", appId],
    queryFn: () => orderProfilesApi.list(appId),
  });
  const refreshProfiles = () =>
    client.invalidateQueries({ queryKey: ["orderProfiles", appId] });
  const applyOrder = async () => {
    if (pendingOrderCount === 0 || saving || routing.busy) return;
    const previousOrder = order;
    const appliedSet = new Set(targetIds);
    const switchModel = modelFilter;
    setSaving(true);
    // 乐观快照 = 应用目标在前、链外档位跟后（镜像后端展示序），后端结果一到即替换。
    setOrder({
      source: routing.data,
      ids: [...targetIds, ...storedIds.filter((id) => !appliedSet.has(id))],
    });
    try {
      await routing.setOrder(targetIds);
      setStagedIds(null);
      setSort(null);
      // 链已生效；快照进当前配置档失败只警告——链是活事实，配置档可手动再存。
      const currentProfile = profilesState?.current;
      if (currentProfile) {
        try {
          await orderProfilesApi.save(appId, currentProfile, targetIds);
        } catch (error) {
          toast.warning(t("applications.orderProfileSaveFailed"), {
            description: extractErrorMessage(error) || undefined,
          });
        }
        void refreshProfiles();
      }
      // 筛选模型 + 应用 = 切模型（2026-09-17 用户定调）：当前档不在应用目标里
      // （它不服务这个模型/被筛出）时，把目标里第一个可用档位设为当前，走标准
      // 切换编排（codex 弹「退出并切换」确认 → 退 → 切 → 重开）。只有调序/
      // 筛账号的应用不碰进程；切换弹窗该取消取消，取消不影响已应用的链。
      if (switchModel) {
        const currentId = configurations.find(
          (item) => item.presentation.isCurrent,
        )?.providerId;
        if (currentId == null || !appliedSet.has(currentId)) {
          const unavailable = new Set(
            tiers
              .filter((tier) => tier.skipReason === "circuit_open")
              .map((tier) => tier.providerId),
          );
          const candidate = targetIds
            .filter((id) => id !== currentId && !unavailable.has(id))
            .map((id) => configurations.find((item) => item.providerId === id))
            .find((item) => item?.canSelect);
          if (candidate) {
            void model.select(candidate);
          } else {
            toast.info(t("applications.applyOrderNoSwitchableTier"));
          }
        }
      }
    } catch {
      setOrder(previousOrder);
    } finally {
      setSaving(false);
    }
  };
  // 撤回 = 丢弃未应用的改动（拖拽暂存、临时排序、筛选与搜索一起清），
  // 回到存储链的全量视图。
  const discardOrder = () => {
    setStagedIds(null);
    setSort(null);
    setAccountFilter(null);
    setModelFilter(null);
    setSearch("");
  };
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
  // 模型筛选的选项 = 各档位实际会用的模型（effectiveModel 优先），去重排序；
  // 聚合范围跟随账号筛选——选了账号后分数必须只数该账号的档位，不能仍报全量
  // （2026-09-17 用户报的 bug：7 档筛剩 5，模型分数仍是 7/7）。
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
          model: tier.effectiveModel,
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
      const model = tier?.model ?? item.model;
      if (!model) continue;
      const entry = stats.get(model) ?? { model, available: 0, total: 0 };
      entry.total += 1;
      if (tier?.available ?? true) entry.available += 1;
      stats.set(model, entry);
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
  // 模型下拉的显隐看**全量**模型数：账号筛选收窄到单模型时下拉仍要可见——
  // 分数（该账号可用/总数）本身就是用户要看的信息。
  const hasMultipleModelsOverall = useMemo(() => {
    const byId = new Map(
      tiers.map((tier) => [tier.providerId, tier.effectiveModel]),
    );
    const models = new Set(
      configurations
        .map((item) => byId.get(item.providerId) ?? item.model)
        .filter((model): model is string => Boolean(model)),
    );
    return models.size > 1;
  }, [configurations, tiers]);
  const routingModel = routing.data?.model ?? null;
  const orderBusy =
    saving || routing.busy || routing.isPending || Boolean(routing.error);
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
              {isProxyAppId(appId) && (
                <div className="inline-flex items-center gap-1">
                  <label className="inline-flex cursor-pointer items-center gap-2 text-sm">
                    <Switch
                      checked={routing.data?.autoFailoverEnabled ?? false}
                      disabled={
                        routing.busy ||
                        routing.isPending ||
                        Boolean(routing.error)
                      }
                      aria-label={t("applications.autoFailover")}
                      onCheckedChange={(checked) => {
                        void routing
                          .setFailover(checked)
                          .catch(() => undefined);
                      }}
                    />
                    {t("applications.autoFailover")}
                  </label>
                  {/* 问号在 label 外：悬停说明语义，点击不触发开关。 */}
                  <TooltipProvider delayDuration={250}>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <span
                          tabIndex={0}
                          aria-label={t("applications.autoFailoverHint")}
                          className="inline-flex cursor-help items-center rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
                        >
                          <CircleHelp className="h-3.5 w-3.5 text-muted-foreground/60 hover:text-muted-foreground" />
                        </span>
                      </TooltipTrigger>
                      <TooltipContent
                        side="bottom"
                        className="max-w-xs leading-relaxed"
                      >
                        {t("applications.autoFailoverHint")}
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                </div>
              )}
              {chainEditing && pendingOrderCount > 0 && (
                <>
                  {/* 取消比主操作轻一级（ghost）：丢弃未应用的拖拽/排序/筛选，
                      回到已应用的链视图。 */}
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={discardOrder}
                    disabled={orderBusy}
                    className="h-7 text-xs"
                  >
                    {t("applications.discardOrder")}
                  </Button>
                  <Button
                    size="sm"
                    onClick={() => {
                      void applyOrder();
                    }}
                    disabled={orderBusy}
                    className="h-7 gap-1.5 text-xs"
                  >
                    <Check className="h-3.5 w-3.5" />
                    {t("applications.applyOrder")}
                    <span className="tabular-nums">({pendingOrderCount})</span>
                  </Button>
                </>
              )}
              {chainEditing && (
                <OrderProfilesMenu
                  appType={appId}
                  state={profilesState}
                  targetIds={targetIds}
                  storedIds={storedIds}
                  onLoadDraft={(ids) => {
                    // 载入配置档 = 进草稿：清临时排序，照常「应用/取消」。
                    setSort(null);
                    setStagedIds(ids);
                  }}
                />
              )}
            </div>
            {(!isProxyAppId(appId) || failoverEnabled) && (
              <p className="mt-2 text-xs leading-5 text-muted-foreground">
                {t(
                  isProxyAppId(appId)
                    ? "applications.priorityHint"
                    : "applications.orderHint",
                )}
              </p>
            )}
          </div>
          <Button variant="outline" onClick={onAdd}>
            <Plus className="h-4 w-4" />
            {t("applications.addService")}
          </Button>
        </header>
        <div className="flex flex-wrap items-center gap-3">
          {accounts.length > 1 && (
            <Select
              value={accountFilter ?? "all"}
              onValueChange={(value) =>
                setAccountFilter(value === "all" ? null : value)
              }
            >
              <SelectTrigger
                className="w-48"
                aria-label={t("applications.accountFilter")}
              >
                <span className="flex items-center gap-2 truncate">
                  <ListFilter className="h-4 w-4 shrink-0 text-muted-foreground" />
                  <SelectValue />
                </span>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  {t("applications.allAccounts")}
                </SelectItem>
                {accounts.map((account) => (
                  <SelectItem key={account.key} value={account.key}>
                    {account.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          {hasMultipleModelsOverall && (
            <Select
              value={modelFilter ?? "all"}
              onValueChange={(value) =>
                setModelFilter(value === "all" ? null : value)
              }
            >
              <SelectTrigger
                className="w-64"
                aria-label={t("applications.modelFilter")}
              >
                <span className="flex items-center gap-2 truncate">
                  <ListFilter className="h-4 w-4 shrink-0 text-muted-foreground" />
                  <SelectValue />
                </span>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  {t("applications.allModels")}
                </SelectItem>
                {tierModels.map((option) => (
                  <SelectItem
                    key={option.model}
                    value={option.model}
                    className={
                      option.available === 0 ? "text-muted-foreground" : ""
                    }
                  >
                    <span className="flex items-center gap-2">
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
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          <div className="relative min-w-60 max-w-md flex-1">
            <Search className="pointer-events-none absolute left-3 top-3 h-4 w-4 text-muted-foreground" />
            <Input
              type="search"
              aria-label={t("applications.search")}
              placeholder={t("applications.search")}
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              className="pl-9"
            />
          </div>
        </div>
        {routing.data?.autoFailoverEnabled &&
          routing.data.routingActive === false && (
            <div
              role="status"
              className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-sm"
            >
              <span>{t("applications.routingPaused")}</span>
              <Button
                variant="outline"
                size="sm"
                disabled={routing.busy}
                onClick={() => {
                  void routing.setFailover(true).catch(() => undefined);
                }}
              >
                {t("applications.resumeRouting")}
              </Button>
            </div>
          )}
        {(model.isPending || routing.isPending) && (
          <p role="status" className="text-sm text-muted-foreground">
            {t("common.loading")}
          </p>
        )}
        {(model.error || routing.error) && (
          <div
            role="alert"
            className="flex items-center justify-between gap-3 rounded-lg border border-destructive/30 p-3 text-sm"
          >
            <span>{t("applications.loadFailed")}</span>
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
        <ApplicationTierTable
          configurations={configurations}
          tiers={tiers}
          orderedIds={orderedIds}
          failoverEnabled={failoverEnabled}
          search={search}
          accountFilter={accountFilter}
          modelFilter={modelFilter}
          additive={model.data?.isAdditive ?? false}
          busy={model.busy}
          orderBusy={orderBusy}
          sort={sort}
          onSort={sortBy}
          onReorder={(next) => {
            void changeOrder(next);
          }}
          onSelect={(item) => {
            void model.select(item);
          }}
          onOpenAccount={onOpenAccount}
          onBlockTier={(providerId, blocked) => {
            void routing
              .blockTier({ providerId, blocked })
              .catch(() => undefined);
          }}
        />
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
          {children}
        </CollapsibleContent>
      </Collapsible>
      <SwitchTierConfirmDialog
        targetName={model.confirmation}
        onCancel={model.cancel}
        onSwitch={model.confirm}
      />
    </div>
  );
}
