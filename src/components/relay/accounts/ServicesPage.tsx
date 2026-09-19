import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, ArrowRight, Plus, Server, ShieldCheck } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { AccountRoute } from "@/components/shell/navigation";
import { getAppDisplayName } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
import { RelaySection, type RelaySectionProps } from "../RelaySection";
import type { SubscriptionWindow } from "@/lib/api/applicationRouting";
import { formatResetAt } from "@/components/applications/tierMetrics";
import { RowBalance } from "../RowBalance";
import {
  configuredApps,
  useServiceAccounts,
  type ServiceAccount,
} from "./useServiceAccounts";

export interface ServicesPageProps {
  appId: AppId;
  account?: AccountRoute;
  onBack?: () => void;
  onSelectAccount?: (account: AccountRoute | undefined, app: AppId) => void;
  onOpenAddHub: RelaySectionProps["onOpenAddHub"];
  onOpenApp: (appId: AppId) => void;
}

const accountName = (account: ServiceAccount) =>
  account.kind === "relay" ? account.row.siteName : account.row.vendorName;

export function ServicesPage({
  appId,
  account,
  onBack,
  onSelectAccount,
  onOpenAddHub,
  onOpenApp,
}: ServicesPageProps) {
  const { t } = useTranslation();
  const { accounts, isPending, error, reload } = useServiceAccounts();
  const [localSelection, setLocalSelection] = useState<{
    kind: ServiceAccount["kind"];
    id: number;
    appId: AppId;
  } | null>(null);
  const selection = onSelectAccount
    ? account
      ? { ...account, appId }
      : null
    : localSelection;
  const setSelection = (
    next: { kind: AccountRoute["kind"]; id: number; appId: AppId } | null,
  ) => {
    if (onSelectAccount)
      onSelectAccount(next ?? undefined, next?.appId ?? appId);
    else setLocalSelection(next);
  };
  const selected =
    selection &&
    accounts.find(
      (account) =>
        account.kind === selection.kind && account.id === selection.id,
    );

  if (selection) {
    return (
      <section className="page-content space-y-6">
        {!onSelectAccount && (
          <Button
            variant="ghost"
            onClick={() => {
              if (onBack) onBack();
              else setSelection(null);
              void reload();
            }}
          >
            <ArrowLeft className="h-4 w-4" />
            {t(onBack ? "common.back" : "loongport.accounts.back")}
          </Button>
        )}
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <p className="text-xs text-muted-foreground">
              {t(`loongport.accounts.${selection.kind}`)}
            </p>
            <h2 className="mt-1 text-xl font-semibold">
              {selected
                ? accountName(selected)
                : t("loongport.accounts.detail")}
            </h2>
            {selected?.row.accountLabel && (
              <p className="text-sm text-muted-foreground">
                {selected.row.accountLabel}
              </p>
            )}
          </div>
          {selected && (
            <div className="flex items-center gap-2">
              <Select
                value={selection.appId}
                onValueChange={(value) =>
                  setSelection({ ...selection, appId: value as AppId })
                }
              >
                <SelectTrigger
                  className="w-44"
                  aria-label={t("loongport.accounts.application")}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {[...selected.apps.keys()].map((app) => (
                    <SelectItem key={app} value={app}>
                      {getAppDisplayName(app, t)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button
                variant="outline"
                onClick={() => onOpenApp(selection.appId)}
              >
                {t("loongport.accounts.openApp")}
                <ArrowRight className="h-4 w-4" />
              </Button>
            </div>
          )}
        </div>
        {selected?.kind === "relay" && (
          <SubscriptionWindowsCard tiers={[...selected.apps.values()]} />
        )}
        <RelaySection
          key={`${selection.kind}:${selection.id}:${selection.appId}`}
          appId={selection.appId}
          accountActionsOnly
          accountFilter={{ kind: selection.kind, id: selection.id }}
          accountSnapshot={
            selected && selected.apps.has(selection.appId)
              ? selected.kind === "relay"
                ? {
                    kind: "relay",
                    row: selected.apps.get(selection.appId)!,
                    appId: selection.appId,
                  }
                : {
                    kind: "vendor",
                    row: selected.apps.get(selection.appId)!,
                    appId: selection.appId,
                  }
              : null
          }
          onOpenAddHub={onOpenAddHub}
          onAccountChanged={() => void reload()}
        />
      </section>
    );
  }

  return (
    <RelaySection
      appId={appId}
      accountSnapshot={null}
      onOpenAddHub={onOpenAddHub}
      onAccountChanged={() => void reload()}
      renderAccounts={(renderActions, renderDelete) => (
        <section className="page-content space-y-6">
          <header className="flex flex-wrap items-center justify-between gap-4">
            <div>
              <p className="max-w-xl text-sm leading-6 text-muted-foreground">
                {t("loongport.accounts.description")}
              </p>
            </div>
            <Button onClick={() => onOpenAddHub("directory")}>
              <Plus className="h-4 w-4" />
              {t("loongport.accounts.add")}
            </Button>
          </header>
          {isPending && (
            <p role="status" className="text-sm text-muted-foreground">
              {t("common.loading")}
            </p>
          )}
          {error && (
            <div
              role="alert"
              className="rounded-xl border border-destructive/30 p-4 text-sm"
            >
              <p>{t("loongport.accounts.loadFailed")}</p>
              <Button
                variant="outline"
                className="mt-3"
                onClick={() => void reload()}
              >
                {t("common.refresh")}
              </Button>
            </div>
          )}
          {!isPending && !error && accounts.length === 0 && (
            <div className="rounded-xl border border-dashed p-10 text-center text-sm text-muted-foreground">
              {t("loongport.accounts.empty")}
            </div>
          )}
          <div className="grid gap-2">
            {accounts.map((account) => {
              const Icon = account.kind === "relay" ? Server : ShieldCheck;
              const apps = configuredApps(account);
              const contextApp = account.apps.has(appId)
                ? appId
                : account.apps.keys().next().value!;
              const row = account.apps.get(contextApp)!;
              return (
                <article
                  key={`${account.kind}:${account.id}`}
                  className="group relative rounded-xl border border-border bg-card p-3 pr-12"
                >
                  {account.kind === "relay"
                    ? renderDelete({
                        kind: "relay",
                        row: account.apps.get(contextApp)!,
                        appId: contextApp,
                      })
                    : renderDelete({
                        kind: "vendor",
                        row: account.apps.get(contextApp)!,
                        appId: contextApp,
                      })}
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex min-w-0 items-center gap-2.5">
                      <div className="rounded-lg bg-muted p-2">
                        <Icon className="h-4 w-4 text-muted-foreground" />
                      </div>
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-2">
                          <h2 className="text-sm font-medium">
                            {accountName(account)}
                          </h2>
                          <span className="rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground">
                            {t(`loongport.accounts.${account.kind}`)}
                          </span>
                        </div>
                        <p className="mt-0.5 break-all text-xs text-muted-foreground">
                          {[
                            account.row.accountLabel,
                            getAppDisplayName(contextApp, t),
                            t(`loongport.accounts.status.${row.status}`),
                          ]
                            .filter(Boolean)
                            .join(" · ")}
                        </p>
                      </div>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-7"
                      onClick={() =>
                        setSelection({
                          kind: account.kind,
                          id: account.id,
                          appId: contextApp,
                        })
                      }
                    >
                      {t("loongport.accounts.detail")}
                      <ArrowRight className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                  <div className="mt-2">
                    {account.kind === "relay"
                      ? renderActions({
                          kind: "relay",
                          row: account.apps.get(contextApp)!,
                          appId: contextApp,
                        })
                      : renderActions({
                          kind: "vendor",
                          row: account.apps.get(contextApp)!,
                          appId: contextApp,
                        })}
                  </div>
                  {(apps.length > 0 || account.row.canQueryBalance) && (
                    <div className="mt-2 flex flex-wrap items-center justify-between gap-2 border-t border-border/60 pt-2">
                      <div className="flex flex-wrap gap-1">
                        {apps.map((app) => (
                          <Button
                            key={app}
                            variant="ghost"
                            size="sm"
                            className="h-6 px-2 text-xs"
                            onClick={() => onOpenApp(app)}
                          >
                            {getAppDisplayName(app, t)}
                            <ArrowRight className="ml-1 h-3 w-3" />
                          </Button>
                        ))}
                      </div>
                      {account.row.canQueryBalance && (
                        <RowBalance
                          rowKind={account.kind}
                          rowId={account.id}
                          enabled={account.row.canQueryBalance}
                        />
                      )}
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

/**
 * 账号详情的「订阅限额与重置」区：聚合该账号全部档位的窗口。
 *
 * composite 拆档的多个档共享同一把 key ⇒ 窗口数组逐字节相同，按内容去重；
 * 非订阅账号（全部档位窗口为空）整区不渲染，不留空壳。
 */
function SubscriptionWindowsCard({
  tiers,
}: {
  tiers: { tiers: { subscriptionWindows: SubscriptionWindow[] }[] }[];
}) {
  const { t } = useTranslation();
  const windows = (() => {
    const seen = new Set<string>();
    const result: SubscriptionWindow[] = [];
    for (const row of tiers) {
      for (const tier of row.tiers) {
        for (const window of tier.subscriptionWindows) {
          const key = JSON.stringify(window);
          if (!seen.has(key)) {
            seen.add(key);
            result.push(window);
          }
        }
      }
    }
    return result.sort(
      (a, b) =>
        (a.resetAt ?? Number.MAX_SAFE_INTEGER) -
        (b.resetAt ?? Number.MAX_SAFE_INTEGER),
    );
  })();
  if (windows.length === 0) return null;
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
              key={window.kind}
              className="border-b border-border/40 last:border-0"
            >
              <td className="px-4 py-2.5">
                {t(`loongport.accounts.windowKind.${window.kind}`)}
              </td>
              <td className="px-4 py-2.5 text-right">
                {window.usedUsd == null
                  ? `— / ${window.limitUsd.toFixed(2)}`
                  : `${window.usedUsd.toFixed(2)} / ${window.limitUsd.toFixed(2)}`}
              </td>
              <td className="px-4 py-2.5 text-right">
                {window.resetAt == null ? "—" : formatResetAt(window.resetAt)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
