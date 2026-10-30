import { useContext } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  Loader2,
  Server,
  ShieldCheck,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { AppId } from "@/lib/api/types";
import { AuthCenterPanel } from "@/components/settings/AuthCenterPanel";
import { RelaySection } from "../RelaySection";
import { RowBalance } from "../RowBalance";
import { AccountWorkspaceTarget } from "./AccountWorkspace";
import { QoderAccountPanel } from "./QoderAccountPanel";
import { ServiceSourceList } from "./ServiceSourceList";
import type { ServicesPageProps } from "./ServicesPage";
import {
  useServiceAccountDetailsViewModel,
  type ServiceAccountDetailsViewModel,
} from "./useServiceAccountDetailsViewModel";
import type { ServiceAccountScope } from "./serviceAccountDetailsProjection";

const accountIcons = { relay: Server, vendor: ShieldCheck };

/** Render-only account/detail surface. Inventory and commands retain their VM owner. */
export function ServiceAccountDetailsView(
  props: ServicesPageProps & { accountKind?: ServiceAccountScope },
) {
  const { t } = useTranslation();
  const target = useContext(AccountWorkspaceTarget);
  const vm = useServiceAccountDetailsViewModel({ ...props, target });
  if (vm.entry.kind === "qoder")
    return (
      <section className="page-content space-y-4">
        <QoderAccountPanel defaultApp={props.appId} />
        <Button variant="outline" onClick={vm.openQoderConfiguration}>
          管理原有 Qoder 配置
        </Button>
      </section>
    );
  if (vm.entry.kind === "source")
    return (
      <section className="page-content space-y-4">
        <h2 className="text-xl font-semibold">{vm.entry.title}</h2>
        <p className="text-sm text-muted-foreground">{vm.entry.description}</p>
        <Button variant="outline" onClick={vm.openSourceEntry}>
          添加服务连接
        </Button>
      </section>
    );
  if (vm.entry.kind === "platform")
    return (
      <section className="page-content space-y-6">
        <h2 className="text-xl font-semibold">{vm.entry.title}</h2>
        <AuthCenterPanel
          key={vm.entry.platform}
          platform={vm.entry.platform}
          editionScoped
          onNavigationBlockedChange={props.onNavigationBlockedChange}
        />
      </section>
    );
  const detail = vm.detail;
  if (detail)
    return (
      <section className="page-content space-y-6">
        {detail.showBack && (
          <Button variant="ghost" onClick={vm.back}>
            <ArrowLeft className="h-4 w-4" />
            {detail.backLabel}
          </Button>
        )}
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <p className="text-xs text-muted-foreground">{detail.kindLabel}</p>
            <h2 className="mt-1 text-xl font-semibold">{detail.name}</h2>
            {detail.label && (
              <p className="text-sm text-muted-foreground">{detail.label}</p>
            )}
          </div>
          {detail.hasAccount && (
            <div className="flex items-center gap-2">
              <Select
                value={detail.appId}
                onValueChange={(value) => detail.chooseApp(value as AppId)}
              >
                <SelectTrigger
                  className="w-44"
                  aria-label={t("loongport.accounts.application")}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {detail.apps.map((app) => (
                    <SelectItem key={app.id} value={app.id}>
                      {app.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button variant="outline" onClick={detail.openApp}>
                {t("loongport.accounts.openApp")}
                <ArrowRight className="h-4 w-4" />
              </Button>
            </div>
          )}
        </div>
        {detail.hasAccount && (
          <ServiceSourceList
            account={detail.account}
            onOpenApp={props.onOpenApp}
          />
        )}
        <AccountUsageCard usage={detail.usage} />
        <SubscriptionWindowsCard windows={detail.windows} />
        <RelaySection
          key={detail.key}
          appId={detail.appId}
          accountActionsOnly
          accountFilter={detail.account}
          accountSnapshot={detail.snapshot}
          onOpenAddHub={props.onOpenAddHub}
          onAccountChanged={vm.reloadAccounts}
        />
      </section>
    );
  return (
    <RelaySection
      appId={props.appId}
      accountSnapshot={null}
      onOpenAddHub={props.onOpenAddHub}
      onAccountChanged={vm.reloadAccounts}
      renderAccounts={(renderActions, renderDelete) => (
        <section className="page-content space-y-6">
          {vm.isPending && (
            <p role="status" className="text-sm text-muted-foreground">
              {t("common.loading")}
            </p>
          )}
          {vm.error && (
            <div
              role="alert"
              className="rounded-xl border border-destructive/30 p-4 text-sm"
            >
              <p>{t("loongport.accounts.loadFailed")}</p>
              <Button
                variant="outline"
                className="mt-3"
                onClick={() => void vm.reloadAccounts()}
              >
                {t("common.refresh")}
              </Button>
            </div>
          )}
          {!vm.isPending && !vm.error && vm.rows.length === 0 && (
            <div className="rounded-xl border border-dashed p-10 text-center text-sm text-muted-foreground">
              {t("loongport.accounts.empty")}
            </div>
          )}
          <div className="grid gap-2">
            {vm.rows.map((row) => {
              const Icon = accountIcons[row.icon];
              return (
                <article
                  key={row.key}
                  className="group relative rounded-xl border border-border bg-card p-3 pr-12"
                >
                  {renderDelete(row.actionTarget)}
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex min-w-0 items-center gap-2.5">
                      <div className="rounded-lg bg-muted p-2">
                        <Icon className="h-4 w-4 text-muted-foreground" />
                      </div>
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-2">
                          <h2 className="text-sm font-medium">{row.name}</h2>
                          <span className="rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground">
                            {row.kindLabel}
                          </span>
                        </div>
                        <p className="mt-0.5 break-all text-xs text-muted-foreground">
                          {row.subtitle}
                        </p>
                      </div>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-7"
                      onClick={row.openDetails}
                    >
                      {t("loongport.accounts.detail")}
                      <ArrowRight className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                  {row.activity?.state === "busy" && (
                    <p
                      role="status"
                      className="mt-2 flex items-center gap-2 text-xs text-muted-foreground"
                    >
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      {t("loongport.accounts.importingTiers")}
                    </p>
                  )}
                  {row.activity?.state === "error" && (
                    <p
                      role="alert"
                      className="mt-2 flex items-start gap-2 text-xs text-destructive"
                    >
                      <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                      {t("loongport.accounts.importFailedLine", {
                        reason: row.activity.error ?? "",
                      })}
                    </p>
                  )}
                  <div className="mt-2">{renderActions(row.actionTarget)}</div>
                  {(row.apps.length > 0 || row.balance) && (
                    <div className="mt-2 flex flex-wrap items-center justify-between gap-2 border-t border-border/60 pt-2">
                      <div className="flex flex-wrap gap-1">
                        {row.apps.map((app) => (
                          <Button
                            key={app.id}
                            variant="ghost"
                            size="sm"
                            className="h-6 px-2 text-xs"
                            onClick={app.open}
                          >
                            {app.label}
                            <ArrowRight className="ml-1 h-3 w-3" />
                          </Button>
                        ))}
                      </div>
                      {row.balance && <RowBalance {...row.balance} />}
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

type Detail = NonNullable<ServiceAccountDetailsViewModel["detail"]>;
function SubscriptionWindowsCard({ windows }: { windows: Detail["windows"] }) {
  const { t } = useTranslation();
  if (!windows.length) return null;
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
              key={window.key}
              className="border-b border-border/40 last:border-0"
            >
              <td className="px-4 py-2.5">{window.label}</td>
              <td className="px-4 py-2.5 text-right">{window.usedLimit}</td>
              <td className="px-4 py-2.5 text-right">{window.resetLabel}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
function AccountUsageCard({ usage }: { usage: Detail["usage"] }) {
  const { t } = useTranslation();
  if (!usage) return null;
  return (
    <section className="rounded-xl border border-border bg-card">
      <div className="flex flex-wrap items-baseline justify-between gap-2 border-b border-border/60 px-4 py-3">
        <h3 className="text-sm font-medium">
          {t("loongport.accounts.usageTitle")}
        </h3>
        <p className="text-xs text-muted-foreground tabular-nums">
          {usage.headline}
        </p>
      </div>
      <ul className="divide-y divide-border/40">
        {usage.rows.map((row) => (
          <li
            key={row.providerId}
            className="flex items-center gap-3 px-4 py-2.5"
          >
            <span className="min-w-0 flex-1 truncate text-sm">
              {row.displayName}
            </span>
            <span
              className="h-1.5 rounded-full bg-primary/60"
              style={{ width: row.barWidth }}
              aria-hidden="true"
            />
            <span className="w-16 text-right text-sm tabular-nums">
              {row.costLabel}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}
