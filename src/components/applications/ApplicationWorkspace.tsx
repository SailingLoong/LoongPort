import { useMemo, useState, type ReactNode } from "react";
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
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { SwitchTierConfirmDialog } from "@/components/relay/SwitchTierConfirmDialog";
import { useApplicationOverview } from "./useApplicationOverview";
import { useApplicationRouting } from "./useApplicationRouting";
import { ApplicationTierTable } from "./ApplicationTierTable";
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
  // 故障切换开启时的暂存顺序：拖拽只改这里，点「应用此顺序」才写库；
  // 关闭故障切换时拖拽照旧即时落库（无应用按钮）。
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
  // 草稿语义统一（2026-09-16 用户定调）：路由类应用的任何调整（拖拽/排序）
  // 都只进草稿，点「应用」才写库——与故障切换开关状态无关；非路由类应用
  // （无故障切换概念）维持拖拽即时保存。
  const draftOrdering = isProxyAppId(appId);
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
  // 「应用此顺序」应用的是**当前显示序**（2026-09-16 定调）：拖拽暂存与指标排序
  // 排出来的顺序同样算——排序视图下也想「就按这个顺序切换」。待应用计数 =
  // 显示序与存储序位置不同的档位数；两者一致（纯筛选视图）就没有待应用。
  const pendingOrderCount = (() => {
    if (!draftOrdering) return 0;
    if (orderedIds.length !== storedIds.length) return orderedIds.length;
    let count = 0;
    for (let index = 0; index < orderedIds.length; index += 1) {
      if (orderedIds[index] !== storedIds[index]) count += 1;
    }
    return count;
  })();
  const applyStagedOrder = async () => {
    if (pendingOrderCount === 0 || saving || routing.busy) return;
    const previousOrder = order;
    setSaving(true);
    setOrder({ source: routing.data, ids: orderedIds });
    try {
      await routing.setOrder(orderedIds);
      setStagedIds(null);
      setSort(null);
    } catch {
      setOrder(previousOrder);
    } finally {
      setSaving(false);
    }
  };
  // 撤回 = 丢弃未应用的改动（拖拽暂存与临时排序一起清），回到存储序。
  const discardStagedOrder = () => {
    setStagedIds(null);
    setSort(null);
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
  // 当前路由模型置顶加 ⚡，故障切换场景「选模型 → 过滤看链上还有谁」一步到位。
  // 每个模型带 可用/总数 分数：不可用只算「已嗅探到的错误」（circuit_open=
  // 限流/网络/余额等真实失败累积触发的熔断）与用户屏蔽；模型不匹配/能力声明/
  // 位置语义一概不扣——它们对别的模型恒真，不是错误（2026-09-16 用户定调）。
  // 全不可用的模型置灰但可选（选完正好逐行看原因）。
  const tierModels = useMemo(() => {
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
  }, [configurations, tiers, routing.data?.model]);
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
                      void routing.setFailover(checked).catch(() => undefined);
                    }}
                  />
                  {t("applications.autoFailover")}
                </label>
              )}
              {draftOrdering && pendingOrderCount > 0 && (
                <>
                  {/* 取消比主操作轻一级（ghost）：丢弃未应用的拖拽/排序，
                      回到之前的配置（存储序）。 */}
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={discardStagedOrder}
                    disabled={orderBusy}
                    className="h-7 text-xs"
                  >
                    {t("applications.discardOrder")}
                  </Button>
                  <Button
                    size="sm"
                    onClick={() => {
                      void applyStagedOrder();
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
            </div>
            <p className="mt-2 text-xs leading-5 text-muted-foreground">
              {t(
                isProxyAppId(appId)
                  ? "applications.priorityHint"
                  : "applications.orderHint",
              )}
            </p>
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
          {tierModels.length > 1 && (
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
