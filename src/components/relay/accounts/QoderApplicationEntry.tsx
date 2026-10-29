import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { getAppDisplayName } from "@/config/appConfig";
import {
  serviceSourcesApi,
  type ServiceSource,
} from "@/lib/api/serviceSources";
import type { AppId } from "@/lib/api/types";
import type { Provider } from "@/types";
import { AccountDetailSection } from "./AccountWorkspace";
import { serviceSourcesKey } from "./ServiceSourceList";

/** Join only the backend-verified original Codex binding. A Provider ID is not an account ID. */
export function qoderApplicationSources(
  provider: Provider,
  sources: ServiceSource[],
): ServiceSource[] {
  const type = provider.meta?.providerType;
  const platform =
    type === "qoder_sdk"
      ? "qoder_sdk"
      : type === "qoder_cn" || type === "qoder_cn_device"
        ? "qoder_cn"
        : undefined;
  if (!platform) return [];
  return sources.flatMap((source) => {
    if (
      source.account.kind !== "nativeAccount" ||
      source.account.platform !== platform
    )
      return [];
    const bindings = source.bindings.filter(
      (binding) =>
        binding.appId === "codex" && binding.providerId === provider.id,
    );
    // The native projection rejects repeated app bindings; never choose among duplicates.
    return bindings.length === 1 ? [{ ...source, bindings }] : [];
  });
}

/** Reads existing bindings; navigation delegates to the original application owner.
 * No identity establishment, selection, route apply, model probing or credential writes.
 */
export function QoderApplicationEntry({
  provider,
  sourceGeneration,
  onOpenApp,
  defaultApp,
  onOpenDefault,
  disabled = false,
}: {
  provider: Provider;
  sourceGeneration?: string;
  onOpenApp?: (app: AppId) => void;
  defaultApp?: AppId;
  onOpenDefault?: () => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const query = useQuery({
    queryKey: [
      ...serviceSourcesKey,
      "qoderApplicationEntry",
      provider.id,
      sourceGeneration,
    ],
    queryFn: serviceSourcesApi.list,
    enabled: !disabled,
  });
  const sources = qoderApplicationSources(provider, query.data ?? []);
  return (
    <AccountDetailSection
      title={t("qoderApplicationEntry.title", {
        defaultValue: "当前配置用于哪些应用",
      })}
    >
      <p className="text-xs text-muted-foreground">
        {t("qoderApplicationEntry.description", {
          defaultValue:
            "仅显示当前 Codex 供应商配置的已验证应用绑定。保存配置不代表推理已接通，实际运行仍需验证。",
        })}
      </p>
      {!onOpenApp && (
        <p className="text-xs text-muted-foreground">
          {t("qoderApplicationEntry.unavailable", {
            defaultValue: "当前页面无法打开原应用，请返回账号管理页面查看。",
          })}
        </p>
      )}
      {disabled ? null : query.isPending ? (
        <p role="status">
          {t("qoderApplicationEntry.loading", {
            defaultValue: "正在读取应用绑定…",
          })}
        </p>
      ) : query.error ? (
        <p role="alert">
          {t("qoderApplicationEntry.error", {
            defaultValue: "应用绑定读取失败，请在原应用中查看配置。",
          })}
        </p>
      ) : sources.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          {t("qoderApplicationEntry.empty", {
            defaultValue:
              "当前配置暂无后端确认的应用绑定，请在 Codex 原配置中查看。",
          })}
        </p>
      ) : (
        sources.map((source) => (
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
                  key={JSON.stringify([binding.appId, binding.providerId])}
                  variant="outline"
                  size="sm"
                  className="h-auto min-h-8 gap-2 py-1.5"
                  disabled={!onOpenApp}
                  onClick={() => onOpenApp?.(binding.appId)}
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
        ))
      )}
      {onOpenDefault &&
        (defaultApp !== "codex" ||
          !query.isSuccess ||
          sources.length === 0) && (
          <Button variant="outline" onClick={onOpenDefault}>
            {defaultApp
              ? t("qoderApplicationEntry.defaultNavigationNamed", {
                  defaultValue: "到 {{app}} 中查看与配置",
                  app: getAppDisplayName(defaultApp, t),
                })
              : t("qoderApplicationEntry.defaultNavigation", {
                  defaultValue: "到应用中查看与配置",
                })}
            <ArrowRight className="ml-2 h-4 w-4" />
          </Button>
        )}
    </AccountDetailSection>
  );
}
