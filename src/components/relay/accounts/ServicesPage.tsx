import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  Loader2,
  Plus,
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
import type { AccountRoute } from "@/components/shell/navigation";
import { getAppDisplayName } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
import { RelaySection, type RelaySectionProps } from "../RelaySection";
import { RowBalance } from "../RowBalance";
import { useRowBusy } from "../useRowBusy";
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

/**
 * 账号卡的进行态/失败态（busy 状态源是模块级的，见 `useRowBusy`）：
 * 登录或导入任一在跑就算「正在导入」；都不跑但留有失败记录则显示失败行。
 * 返回 null = 没有任何值得占一行的状态。
 */
function useAccountActivity() {
  const { isBusy, errorOf } = useRowBusy();
  return (account: ServiceAccount) => {
    const keys = (["login", "provision"] as const).map((action) =>
      account.kind === "relay"
        ? `${action}:${account.id}`
        : `${action}:vendor:${account.id}`,
    );
    if (keys.some((key) => isBusy(key))) {
      return { state: "busy" as const, error: null };
    }
    const error =
      keys.map((key) => errorOf(key)).find((message) => message !== null) ??
      null;
    return error ? { state: "error" as const, error } : null;
  };
}

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
  const activityOf = useAccountActivity();
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
              const activity = activityOf(account);
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
                  {activity?.state === "busy" && (
                    <p
                      role="status"
                      className="mt-2 flex items-center gap-2 text-xs text-muted-foreground"
                    >
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      {t("loongport.accounts.importingTiers")}
                    </p>
                  )}
                  {activity?.state === "error" && (
                    <p
                      role="alert"
                      className="mt-2 flex items-start gap-2 text-xs text-destructive"
                    >
                      <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                      {t("loongport.accounts.importFailedLine", {
                        reason: activity.error ?? "",
                      })}
                    </p>
                  )}
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
