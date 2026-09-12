import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, Plus, Search, Settings2 } from "lucide-react";
import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import type { AccountRoute } from "@/components/shell/navigation";
import { isProxyAppId } from "@/config/appConfig";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
  sortTierIds,
  tierMetrics,
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
  const orderedIds =
    order && order.source === routing.data ? order.ids : storedIds;
  const changeOrder = async (next: string[], nextSort: TierSort | null) => {
    if (saving || routing.busy) return;
    const previousOrder = order,
      previousSort = sort;
    setSaving(true);
    setSort(nextSort);
    setOrder({ source: routing.data, ids: next });
    try {
      await routing.setOrder(next);
    } catch {
      setOrder(previousOrder);
      setSort(previousSort);
    } finally {
      setSaving(false);
    }
  };
  const sortBy = (key: TierMetric) => {
    const descending =
      sort?.key === key
        ? !sort.descending
        : tierMetrics.find((metric) => metric.key === key)!.descending;
    const nextSort = { key, descending };
    void changeOrder(sortTierIds(orderedIds, tiers, nextSort), nextSort);
  };
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
          search={search}
          additive={model.data?.isAdditive ?? false}
          busy={model.busy}
          orderBusy={orderBusy}
          sort={sort}
          onSort={sortBy}
          onReorder={(next) => {
            void changeOrder(next, null);
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
