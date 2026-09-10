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
import { getAppDisplayName } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
import { RelaySection, type RelaySectionProps } from "../RelaySection";
import { RowBalance } from "../RowBalance";
import {
  configuredApps,
  useServiceAccounts,
  type ServiceAccount,
} from "./useServiceAccounts";

export interface ServicesPageProps {
  appId: AppId;
  onOpenAddHub: RelaySectionProps["onOpenAddHub"];
  onOpenApp: (appId: AppId) => void;
}

const accountName = (account: ServiceAccount) =>
  account.kind === "relay" ? account.row.siteName : account.row.vendorName;

export function ServicesPage({
  appId,
  onOpenAddHub,
  onOpenApp,
}: ServicesPageProps) {
  const { t } = useTranslation();
  const { accounts, isPending, error, reload } = useServiceAccounts();
  const [selection, setSelection] = useState<{
    kind: ServiceAccount["kind"];
    id: number;
    appId: AppId;
  } | null>(null);
  const selected =
    selection &&
    accounts.find(
      (account) =>
        account.kind === selection.kind && account.id === selection.id,
    );

  if (selection) {
    return (
      <section className="page-content space-y-6">
        <Button
          variant="ghost"
          onClick={() => {
            setSelection(null);
            void reload();
          }}
        >
          <ArrowLeft className="h-4 w-4" />
          {t("loongport.accounts.back")}
        </Button>
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
          accountFilter={{ kind: selection.kind, id: selection.id }}
          onOpenAddHub={onOpenAddHub}
        />
      </section>
    );
  }

  return (
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
      <div className="grid gap-3">
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
              className="rounded-xl border border-border bg-card p-5"
            >
              <div className="flex flex-wrap items-start justify-between gap-4">
                <div className="flex min-w-0 items-start gap-3">
                  <div className="rounded-lg bg-muted p-2.5">
                    <Icon className="h-5 w-5 text-muted-foreground" />
                  </div>
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <h2 className="font-medium">{accountName(account)}</h2>
                      <span className="rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground">
                        {t(`loongport.accounts.${account.kind}`)}
                      </span>
                    </div>
                    {account.row.accountLabel && (
                      <p className="mt-1 break-all text-sm text-muted-foreground">
                        {account.row.accountLabel}
                      </p>
                    )}
                    <p className="mt-1 text-xs text-muted-foreground">
                      {getAppDisplayName(contextApp, t)} ·{" "}
                      {t(`loongport.accounts.status.${row.status}`)}
                    </p>
                  </div>
                </div>
                <Button
                  variant="outline"
                  onClick={() =>
                    setSelection({
                      kind: account.kind,
                      id: account.id,
                      appId: contextApp,
                    })
                  }
                >
                  {t("loongport.accounts.detail")}
                  <ArrowRight className="h-4 w-4" />
                </Button>
              </div>
              {(apps.length > 0 || account.row.canQueryBalance) && (
                <div className="mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-border/60 pt-3">
                  <div className="flex flex-wrap gap-1">
                    {apps.map((app) => (
                      <Button
                        key={app}
                        variant="ghost"
                        size="sm"
                        className="h-7 text-xs"
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
  );
}
