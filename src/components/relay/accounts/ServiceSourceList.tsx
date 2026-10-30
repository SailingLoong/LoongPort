import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { getAppDisplayName } from "@/config/appConfig";
import type { AccountRoute } from "@/components/shell/navigation";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { PROVIDER_SWITCHED, VENDOR_ACCOUNTS_CHANGED } from "@/lib/api/events";
import { serviceSourcesApi } from "@/lib/api/serviceSources";
import type { AppId } from "@/lib/api/types";

import { serviceSourcesKey } from "./serviceSourceKeys";

// Existing application/Qoder consumers retain the same canonical key during migration.
export { serviceSourcesKey } from "./serviceSourceKeys";

/** Existing bindings only. Source discovery and application selection retain their owners. */
export function ServiceSourceList({
  account,
  onOpenApp,
}: {
  account: AccountRoute;
  onOpenApp: (app: AppId) => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const query = useQuery({
    queryKey: serviceSourcesKey,
    queryFn: serviceSourcesApi.list,
  });
  const invalidate = () =>
    client.invalidateQueries({ queryKey: serviceSourcesKey });
  useTauriEvent(PROVIDER_SWITCHED, invalidate);
  useTauriEvent(VENDOR_ACCOUNTS_CHANGED, invalidate);
  const sources = query.data?.filter(
    (source) =>
      source.account.kind === account.kind && source.account.id === account.id,
  );
  return (
    <section className="space-y-3" aria-labelledby="account-shared-sources">
      <div>
        <h3 id="account-shared-sources" className="text-sm font-medium">
          {t("loongport.sources.title")}
        </h3>
        <p className="mt-1 text-xs text-muted-foreground">
          {t("loongport.sources.description")}
        </p>
      </div>
      {query.isPending ? (
        <p role="status" className="text-sm text-muted-foreground">
          {t("common.loading")}
        </p>
      ) : query.error ? (
        <div role="alert" className="space-y-2 text-sm">
          <p>{t("loongport.sources.loadFailed")}</p>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void query.refetch()}
          >
            {t("common.retry")}
          </Button>
        </div>
      ) : !sources?.length ? (
        <p className="text-sm text-muted-foreground">
          {t("loongport.sources.empty")}
        </p>
      ) : (
        <div className="grid gap-2">
          {sources.map((source) => (
            <article
              key={source.sourceId}
              className="rounded-lg border border-border p-3"
            >
              <h4 className="text-sm font-medium">
                {source.configurationName ?? source.name}
              </h4>
              <div className="mt-2 flex flex-wrap gap-2">
                {source.bindings.map((binding) => (
                  <Button
                    key={binding.appId}
                    variant="outline"
                    size="sm"
                    className="h-auto min-h-8 gap-2 py-1.5"
                    onClick={() => onOpenApp(binding.appId)}
                  >
                    <span>{getAppDisplayName(binding.appId, t)}</span>
                    {binding.model && (
                      <span className="max-w-48 truncate font-normal text-muted-foreground">
                        {binding.model}
                      </span>
                    )}
                    {binding.usageState !== "inactive" && (
                      <span className="text-xs text-primary">
                        {t(
                          binding.usageState === "enabled"
                            ? "applications.enabled"
                            : "applications.configured",
                        )}
                      </span>
                    )}
                    <ArrowRight className="h-3.5 w-3.5 shrink-0" />
                  </Button>
                ))}
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}
