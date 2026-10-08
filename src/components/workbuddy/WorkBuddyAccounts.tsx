import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, Loader2, Plus, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  workbuddyApi,
  type WorkBuddyAccount,
  type WorkBuddyLogin,
} from "@/lib/api/workbuddy";
import { settingsApi } from "@/lib/api/settings";
const amount = (value: number | null) =>
  value == null
    ? "—"
    : value.toLocaleString(undefined, { maximumFractionDigits: 2 });
const date = (value: number | null) =>
  value == null ? "—" : new Date(value).toLocaleString();
export function WorkBuddyAccounts() {
  const { t } = useTranslation();
  const [accounts, setAccounts] = useState<WorkBuddyAccount[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const active = useRef(false);
  const alive = useRef(true);
  const [login, setLogin] = useState<WorkBuddyLogin | null>(null);
  const [waiting, setWaiting] = useState(false);
  useEffect(() => {
    alive.current = true;
    void workbuddyApi
      .list()
      .then((rows) => {
        if (alive.current) setAccounts(rows);
      })
      .catch(() => {
        if (alive.current) setFailed(true);
      })
      .finally(() => {
        if (alive.current) setLoading(false);
      });
    return () => {
      alive.current = false;
    };
  }, []);
  const run = async (key: string, operation: () => Promise<void>) => {
    if (active.current) return;
    active.current = true;
    setBusy(key);
    setFailed(false);
    try {
      await operation();
    } catch {
      if (alive.current) setFailed(true);
    } finally {
      active.current = false;
      if (alive.current) setBusy(null);
    }
  };
  const update = (row: WorkBuddyAccount) => {
    if (alive.current)
      setAccounts((rows) => rows.map((old) => (old.id === row.id ? row : old)));
  };
  return (
    <section aria-label={t("workbuddy.title")} className="space-y-3">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-medium">{t("workbuddy.title")}</h2>
        <div className="flex gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={busy !== null || loading || accounts.length === 0}
            onClick={() =>
              void run("all", async () => {
                const rows = await workbuddyApi.refreshAll();
                if (alive.current) setAccounts(rows);
              })
            }
          >
            <RefreshCw className="h-3.5 w-3.5" />
            {t("workbuddy.refreshAll")}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={busy !== null}
            onClick={() =>
              void run("login", async () => {
                const flow = await workbuddyApi.beginLogin();
                if (alive.current) {
                  setLogin(flow);
                  setWaiting(false);
                }
              })
            }
          >
            <Plus className="h-3.5 w-3.5" />
            {t("workbuddy.add")}
          </Button>
        </div>
      </header>
      {loading && (
        <p role="status" className="text-sm text-muted-foreground">
          {t("common.loading")}
        </p>
      )}
      {failed && (
        <p role="alert" className="text-sm text-destructive">
          {t("workbuddy.failed")}
        </p>
      )}
      {login && (
        <div className="flex flex-wrap items-center gap-3 rounded-xl border border-border p-3 text-sm">
          <a
            className="text-primary underline"
            href={login.verificationUri}
            onClick={(event) => {
              event.preventDefault();
              void settingsApi.openExternal(login.verificationUri).catch(() => {
                if (alive.current) setFailed(true);
              });
            }}
          >
            {t("workbuddy.authorize")}
          </a>
          <Button
            size="sm"
            variant="outline"
            disabled={busy !== null}
            onClick={() =>
              void run("finish", async () => {
                const result = await workbuddyApi.finishLogin(login.flowId);
                if (!alive.current) return;
                if (result.state === "saved") {
                  setLogin(null);
                  setAccounts(await workbuddyApi.list());
                } else setWaiting(true);
              })
            }
          >
            {t("workbuddy.finishAuthorization")}
          </Button>
          {waiting && (
            <span role="status">{t("workbuddy.authorizationWaiting")}</span>
          )}
        </div>
      )}
      {!loading && !failed && accounts.length === 0 && (
        <p className="rounded-xl border border-dashed p-4 text-sm text-muted-foreground">
          {t("workbuddy.empty")}
        </p>
      )}
      <div className="grid gap-2">
        {accounts.map((account) => (
          <article
            key={account.id}
            className="rounded-xl border border-border bg-card p-3"
          >
            <div className="flex flex-wrap items-center justify-between gap-3">
              <h3 className="text-sm font-medium">{account.label}</h3>
              <div className="flex gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy !== null || !account.canRefresh}
                  onClick={() =>
                    void run(account.id, async () =>
                      update(await workbuddyApi.refresh(account.id)),
                    )
                  }
                >
                  {t("common.refresh")}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy !== null || !account.canClaim}
                  onClick={() =>
                    void run(account.id, async () =>
                      update(await workbuddyApi.claim(account.id)),
                    )
                  }
                >
                  {t("workbuddy.claim")}
                </Button>
              </div>
            </div>
            <dl className="mt-3 flex flex-wrap gap-x-6 gap-y-2 text-xs text-muted-foreground">
              <div>
                <dt>{t("workbuddy.total")}</dt>
                <dd
                  data-testid="workbuddy-total"
                  className="mt-1 tabular-nums text-foreground"
                >
                  {amount(account.credits.totalRemaining)}
                </dd>
              </div>
              <div>
                <dt>{t("workbuddy.expiry")}</dt>
                <dd className="mt-1">{date(account.credits.nearestExpiry)}</dd>
              </div>
              <div>
                <dt>{t("workbuddy.updated")}</dt>
                <dd className="mt-1">{date(account.credits.updatedAt)}</dd>
              </div>
            </dl>
            <p
              role="status"
              className="mt-3 flex items-center gap-2 text-xs text-muted-foreground"
            >
              {busy === account.id && (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              )}
              {t(`workbuddy.state.${account.claimState}`)}
              {account.credited != null && (
                <span>
                  {t("workbuddy.credited", {
                    amount: amount(account.credited),
                  })}
                </span>
              )}
            </p>
            {account.credits.packages.length > 0 && (
              <Collapsible className="mt-2">
                <CollapsibleTrigger asChild>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-7 px-0 text-xs"
                  >
                    {t("workbuddy.packages")}
                    <ChevronDown className="h-3 w-3" />
                  </Button>
                </CollapsibleTrigger>
                <CollapsibleContent>
                  <ul className="divide-y divide-border/40">
                    {account.credits.packages.map((pack, index) => (
                      <li
                        key={`${pack.id}:${index}`}
                        className="flex flex-wrap justify-between gap-2 py-2 text-xs"
                      >
                        <span>{pack.name || t("workbuddy.package")}</span>
                        <span className="tabular-nums">
                          {t("workbuddy.remaining")} {amount(pack.remaining)} ·{" "}
                          {t("workbuddy.expires")} {date(pack.expireAt)}
                        </span>
                      </li>
                    ))}
                  </ul>
                </CollapsibleContent>
              </Collapsible>
            )}
          </article>
        ))}
      </div>
    </section>
  );
}
