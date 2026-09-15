import { useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, Plus, Search, Settings2 } from "lucide-react";
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
import { ModelPicker } from "./ModelPicker";
import { ApplicationTierTable } from "./ApplicationTierTable";
import {
  defaultDescending,
  sortTierIds,
  type TierMetric,
  type TierSort,
} from "./tierMetrics";

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
  const baseIds =
    order && order.source === routing.data ? order.ids : storedIds;
  // 视图排序只重排展示，不落库；默认序 = 数据库档位序（拖拽维护）。
  const orderedIds = sort ? sortTierIds(baseIds, tiers, sort) : baseIds;
  const changeOrder = async (next: string[]) => {
    if (saving || routing.busy) return;
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
  const tierModels = useMemo(() => {
    const byId = new Map(
      tiers.map((tier) => [tier.providerId, tier.effectiveModel]),
    );
    const models = new Set<string>();
    for (const item of configurations) {
      const model = byId.get(item.providerId) ?? item.model;
      if (model) models.add(model);
    }
    const routingModel = routing.data?.model;
    return [...models]
      .sort((a, b) => a.localeCompare(b))
      .sort((a, b) => (a === routingModel ? -1 : b === routingModel ? 1 : 0));
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
          {isProxyAppId(appId) &&
            routing.data?.routingActive &&
            Boolean(routing.data?.modelOptions?.length) && (
              <ModelPicker
                model={routing.data?.model ?? null}
                modelOptions={routing.data?.modelOptions ?? []}
                disabled={routing.busy}
                onSelect={(value) => {
                  void routing.setModel(value).catch(() => undefined);
                }}
              />
            )}
          {tierModels.length > 1 && (
            <Select
              value={modelFilter ?? "all"}
              onValueChange={(value) =>
                setModelFilter(value === "all" ? null : value)
              }
            >
              <SelectTrigger
                className="w-56"
                aria-label={t("applications.modelFilter")}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  {t("applications.allModels")}
                </SelectItem>
                {tierModels.map((modelOption) => (
                  <SelectItem key={modelOption} value={modelOption}>
                    {modelOption === routingModel
                      ? `⚡ ${modelOption}`
                      : modelOption}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          {accounts.length > 1 && (
            <Select
              value={accountFilter ?? "all"}
              onValueChange={(value) =>
                setAccountFilter(value === "all" ? null : value)
              }
            >
              <SelectTrigger
                className="w-56"
                aria-label={t("applications.accountFilter")}
              >
                <SelectValue />
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
          storedIds={baseIds}
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
