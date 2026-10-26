import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { AppId } from "@/lib/api";
import {
  applicationOverviewApi,
  type ApplicationConfiguration,
} from "@/lib/api/applicationOverview";
import { Button } from "@/components/ui/button";
import { useTierEditGuard } from "@/components/relay/useTierEditGuard";
import { useApplicationRouting } from "./useApplicationRouting";
import { ExpiryStrategySelect } from "./ExpiryStrategySelect";
import { ApplicationModelSettingsDialog } from "./ApplicationModelSettingsDialog";

/** Preserve the original settings owners outside the route-selection workflow. */
export function ApplicationRouteAdvanced({
  appId,
  disabled,
  onRefresh,
  onBusyChange,
}: {
  appId: AppId;
  disabled: boolean;
  onRefresh: () => Promise<void>;
  onBusyChange: (busy: boolean) => void;
}) {
  const { t } = useTranslation();
  const routing = useApplicationRouting(appId);
  const [modelBusy, setModelBusy] = useState(false);
  useEffect(() => {
    onBusyChange(routing.busy || modelBusy);
    return () => onBusyChange(false);
  }, [routing.busy, modelBusy, onBusyChange]);
  const actionsDisabled =
    disabled || routing.busy || routing.writeBlocked || routing.isFetching;
  const overview = useQuery({
    queryKey: ["applicationOverview", appId],
    queryFn: () => applicationOverviewApi.get(appId),
    enabled: appId === "codex",
    retry: false,
  });
  const [target, setTarget] = useState<ApplicationConfiguration | null>(null);
  const { requestEdit, editDialogs } = useTierEditGuard(appId, onRefresh);
  return (
    <div className="mb-5 space-y-4 border-b border-border-default pb-5">
      <ExpiryStrategySelect appId={appId} disabled={actionsDisabled} />
      <div className="space-y-2">
        {(routing.data?.tiers ?? []).map((tier) => {
          const blocked = tier.skipReason === "blocked";
          const canBlock =
            routing.data?.autoFailoverEnabled === true || blocked;
          const hasError =
            tier.skipReason === "circuit_open" ||
            Boolean(tier.lastError) ||
            (tier.consecutiveFailures ?? 0) > 0;
          if (!canBlock && !hasError) return null;
          return (
            <div
              key={tier.providerId}
              className="flex flex-wrap items-center justify-between gap-3 rounded-lg border p-3"
            >
              <span className="min-w-0 break-words text-sm">{tier.name}</span>
              <div className="flex flex-wrap gap-2">
                {canBlock && (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={actionsDisabled}
                    aria-label={`${t(blocked ? "applications.unblockTier" : "applications.blockTier")} · ${tier.name}`}
                    onClick={() =>
                      void routing
                        .blockTier({
                          providerId: tier.providerId,
                          blocked: !blocked,
                        })
                        .catch(() => undefined)
                    }
                  >
                    {t(
                      blocked
                        ? "applications.unblockTier"
                        : "applications.blockTier",
                    )}
                  </Button>
                )}
                {hasError && (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={actionsDisabled}
                    aria-label={`${t("applications.resetTierErrors")} · ${tier.name}`}
                    onClick={() =>
                      void routing
                        .resetTierErrors({ providerId: tier.providerId })
                        .catch(() => undefined)
                    }
                  >
                    {t("applications.resetTierErrors")}
                  </Button>
                )}
              </div>
            </div>
          );
        })}
      </div>
      {appId === "codex" && (
        <section className="space-y-2">
          <h3 className="text-sm font-medium">
            {t("applications.modelSettings.title")}
          </h3>
          {overview.error ? (
            <p role="alert" className="text-sm text-muted-foreground">
              {t("applications.loadFailed")}
              <Button
                variant="link"
                size="sm"
                disabled={overview.isFetching}
                onClick={() => void overview.refetch()}
              >
                {t("common.refresh")}
              </Button>
            </p>
          ) : overview.isPending ? (
            <p role="status" className="text-sm text-muted-foreground">
              {t("common.loading")}
            </p>
          ) : (
            overview.data?.configurations.map((item) => (
              <div
                key={item.providerId}
                className="flex items-center justify-between gap-3 rounded-lg border p-3"
              >
                <span className="min-w-0 break-words text-sm">
                  {item.configurationName ?? item.name}
                </span>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={disabled || overview.isFetching}
                  aria-label={`${t("applications.modelSettings.open")} · ${item.configurationName ?? item.name}`}
                  onClick={() => setTarget(item)}
                >
                  {t("applications.modelSettings.open")}
                </Button>
              </div>
            ))
          )}
        </section>
      )}
      {target && (
        <ApplicationModelSettingsDialog
          key={`${appId}:${target.providerId}`}
          appId={appId}
          providerId={target.providerId}
          name={target.configurationName ?? target.name}
          open
          onOpenChange={(open) => {
            if (!open) setTarget(null);
          }}
          onSaved={onRefresh}
          onBusyChange={setModelBusy}
          onAdvanced={() =>
            requestEdit({
              providerId: target.providerId,
              displayName: target.configurationName ?? target.name,
              isCurrent: target.presentation.isCurrent,
              kind: target.source === "relay" ? "tier" : "vendor",
            })
          }
        />
      )}
      {editDialogs}
    </div>
  );
}
