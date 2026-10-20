import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ExternalLink, Loader2, Plus, RefreshCw } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  minimaxCodeApi,
  type MiniMaxCodeAccount,
  type MiniMaxCodeFailure,
  type MiniMaxCodeLogin,
  type MiniMaxCodeRegion,
} from "@/lib/api/minimaxCode";
import { settingsApi } from "@/lib/api/settings";

const failures: MiniMaxCodeFailure[] = [
  "storageUnavailable",
  "busy",
  "needsLogin",
  "unsupportedContext",
  "staleIdentity",
  "invalidResponse",
  "networkUnavailable",
  "expiredAuthorization",
  "authorizationCompleted",
  "notClaimable",
];
function failureCode(error: unknown): MiniMaxCodeFailure {
  const code =
    typeof error === "string"
      ? error
      : error instanceof Error
        ? error.message
        : typeof error === "object" && error !== null && "code" in error
          ? error.code
          : null;
  return failures.includes(code as MiniMaxCodeFailure)
    ? (code as MiniMaxCodeFailure)
    : "invalidResponse";
}
const date = (value: number) => new Date(value).toLocaleString();
type AccountAction = "refresh" | "claim" | "recheck" | "retire";

/** UI state only. Identities, capabilities and uncertain operations belong to Rust. */
export function MiniMaxCodeSection() {
  const { t } = useTranslation();
  const [accounts, setAccounts] = useState<MiniMaxCodeAccount[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<MiniMaxCodeFailure | null>(null);
  const [busy, setBusy] = useState<Set<string>>(new Set());
  const [region, setRegion] = useState<MiniMaxCodeRegion>("cn");
  const [login, setLogin] = useState<MiniMaxCodeLogin | null>(null);
  const [loginBusy, setLoginBusy] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const alive = useRef(true);
  const listGeneration = useRef(0);
  const activeAccounts = useRef(new Set<string>());
  const loginGeneration = useRef(0);
  const activeLogin = useRef<MiniMaxCodeLogin | null>(null);
  const loginRequest = useRef(false);

  const loadAccounts = useCallback(async (refreshed?: MiniMaxCodeAccount) => {
    const generation = ++listGeneration.current;
    const isCurrent = () =>
      alive.current && generation === listGeneration.current;
    setLoading(true);
    try {
      const rows = await minimaxCodeApi.list();
      if (!isCurrent()) return;
      setAccounts(
        rows.map((row) =>
          refreshed?.entitlement &&
          row.authorized &&
          row.id === refreshed.id &&
          row.authGeneration === refreshed.authGeneration &&
          row.credentialVersion === refreshed.credentialVersion &&
          row.realUserId === refreshed.realUserId &&
          row.region === refreshed.region
            ? { ...row, entitlement: refreshed.entitlement }
            : row,
        ),
      );
      setLoaded(true);
    } catch (cause) {
      if (!isCurrent()) return;
      setAccounts([]);
      setLoaded(false);
      setError(failureCode(cause));
      // A failed catalog read is not an unlocked, empty account store. Do not
      // retain an authorization session after losing access to that store.
      loginGeneration.current += 1;
      loginRequest.current = false;
      const flow = activeLogin.current;
      activeLogin.current = null;
      setLogin(null);
      setLoginBusy(false);
      setWaiting(false);
      if (flow) void minimaxCodeApi.cancelLogin(flow.flowId).catch(() => {});
    } finally {
      if (isCurrent()) setLoading(false);
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void loadAccounts();
    return () => {
      alive.current = false;
      listGeneration.current += 1;
      loginGeneration.current += 1;
      if (activeLogin.current) {
        void minimaxCodeApi
          .cancelLogin(activeLogin.current.flowId)
          .catch(() => {});
        activeLogin.current = null;
      }
    };
  }, [loadAccounts]);

  const runAccount = async (
    account: MiniMaxCodeAccount,
    action: AccountAction,
  ) => {
    if (!loaded || loading || activeAccounts.current.has(account.id)) return;
    activeAccounts.current.add(account.id);
    setBusy(new Set(activeAccounts.current));
    setError(null);
    // A list requested before this action must not replace its newer projection.
    listGeneration.current += 1;
    try {
      const refreshed = await minimaxCodeApi[action](account.id);
      // Re-read the sole catalog instead of publishing a possibly late command
      // response. This also retains pending operations on every other account.
      if (alive.current)
        await loadAccounts(
          action === "refresh" && refreshed ? refreshed : undefined,
        );
    } catch (cause) {
      if (!alive.current) return;
      setError(failureCode(cause));
      // The POST may have reached the server. Read the durable catalog before
      // enabling any further action; never retry a claim or fabricate success.
      await loadAccounts();
    } finally {
      activeAccounts.current.delete(account.id);
      if (alive.current) setBusy(new Set(activeAccounts.current));
    }
  };

  const beginLogin = async () => {
    if (!loaded || loading || loginRequest.current || activeLogin.current)
      return;
    loginRequest.current = true;
    const generation = ++loginGeneration.current;
    setLoginBusy(true);
    setError(null);
    setWaiting(false);
    try {
      const flow = await minimaxCodeApi.beginLogin(region);
      if (!alive.current || generation !== loginGeneration.current) {
        await minimaxCodeApi.cancelLogin(flow.flowId);
        return;
      }
      activeLogin.current = flow;
      setLogin(flow);
    } catch (cause) {
      if (alive.current && generation === loginGeneration.current) {
        setError(failureCode(cause));
        if (failureCode(cause) === "storageUnavailable") await loadAccounts();
      }
    } finally {
      if (generation === loginGeneration.current) {
        loginRequest.current = false;
        if (alive.current) setLoginBusy(false);
      }
    }
  };

  const cancelLogin = async () => {
    const generation = ++loginGeneration.current;
    const flow = activeLogin.current;
    activeLogin.current = null;
    loginRequest.current = false;
    setLogin(null);
    setLoginBusy(false);
    setWaiting(false);
    if (!flow) return;
    try {
      await minimaxCodeApi.cancelLogin(flow.flowId);
    } catch (cause) {
      if (!alive.current) return;
      const code = failureCode(cause);
      // Cancellation may lose to a durably saved authorization. Only the
      // backend's explicit receipt proves that outcome; expiry does not.
      // A late receipt may refresh accounts, but must not describe a new flow
      // (or newly selected region) as completed or replace its device code.
      if (generation === loginGeneration.current) setError(code);
      if (code === "authorizationCompleted") await loadAccounts();
    }
  };

  const pollLogin = async () => {
    const flow = activeLogin.current;
    if (!flow || loginRequest.current) return;
    const generation = loginGeneration.current;
    loginRequest.current = true;
    setLoginBusy(true);
    setError(null);
    try {
      const result = await minimaxCodeApi.pollLogin(flow.flowId);
      if (!alive.current || generation !== loginGeneration.current) return;
      if (result.state === "saved") {
        activeLogin.current = null;
        setLogin(null);
        setWaiting(false);
        await loadAccounts();
      } else setWaiting(true);
    } catch (cause) {
      if (alive.current && generation === loginGeneration.current) {
        setError(failureCode(cause));
        if (failureCode(cause) === "storageUnavailable") await loadAccounts();
      }
    } finally {
      if (generation === loginGeneration.current) {
        loginRequest.current = false;
        if (alive.current) setLoginBusy(false);
      }
    }
  };

  return (
    <div className="space-y-4">
      <p className="text-sm text-muted-foreground">
        {t("minimaxCode.description")}
      </p>
      <div className="flex flex-wrap items-end gap-2">
        <div className="min-w-40 space-y-2">
          <Label htmlFor="minimax-code-region">{t("minimaxCode.region")}</Label>
          <Select
            value={region}
            onValueChange={(value) => {
              loginGeneration.current += 1;
              setRegion(value as MiniMaxCodeRegion);
            }}
            disabled={loginBusy || login !== null}
          >
            <SelectTrigger id="minimax-code-region">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="cn">{t("minimaxCode.regions.cn")}</SelectItem>
              <SelectItem value="global">
                {t("minimaxCode.regions.global")}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
        <Button
          type="button"
          variant="outline"
          disabled={!loaded || loading || loginBusy || login !== null}
          onClick={() => void beginLogin()}
        >
          {loginBusy && !login ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Plus className="mr-2 h-4 w-4" />
          )}
          {t("minimaxCode.add")}
        </Button>
        <Button
          type="button"
          variant="ghost"
          disabled={loading}
          onClick={() => {
            setError(null);
            void loadAccounts();
          }}
        >
          <RefreshCw className="mr-2 h-4 w-4" />
          {t("minimaxCode.reload")}
        </Button>
      </div>
      {loading && (
        <p role="status" className="text-sm text-muted-foreground">
          {t("common.loading")}
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {t(`minimaxCode.errors.${error}`)}
        </p>
      )}
      {(login || loginBusy) && (
        <div className="space-y-3 rounded-lg border bg-muted/50 p-4">
          {login && (
            <>
              <p className="text-sm">{t("minimaxCode.enterCode")}</p>
              <code className="block font-mono text-2xl font-bold tracking-wider">
                {login.userCode}
              </code>
              <a
                href={login.verificationUri}
                className="inline-flex items-center gap-1 text-sm text-primary underline"
                onClick={(event) => {
                  event.preventDefault();
                  void settingsApi
                    .openExternal(login.verificationUri)
                    .catch(() => {
                      if (alive.current) setError("networkUnavailable");
                    });
                }}
              >
                {t("minimaxCode.authorize")}
                <ExternalLink className="h-3 w-3" />
              </a>
              <p className="text-xs text-muted-foreground">
                {t("minimaxCode.expiresAt", { time: date(login.expiresAt) })}
              </p>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={loginBusy || !loaded || loading}
                onClick={() => void pollLogin()}
              >
                {t("minimaxCode.finishAuthorization")}
              </Button>
            </>
          )}
          <Button
            type="button"
            size="sm"
            variant="ghost"
            onClick={() => void cancelLogin()}
          >
            {t("common.cancel", "取消")}
          </Button>
          {waiting && (
            <p role="status" className="text-sm text-muted-foreground">
              {t("minimaxCode.authorizationWaiting")}
            </p>
          )}
        </div>
      )}
      {loaded && !loading && accounts.length === 0 && (
        <p className="rounded-md border border-dashed p-4 text-sm text-muted-foreground">
          {t("minimaxCode.empty")}
        </p>
      )}
      <div className="space-y-2">
        {accounts.map((account) => (
          <article
            key={account.id}
            aria-label={account.label}
            className="space-y-3 rounded-md border bg-muted/30 p-3"
          >
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h5 className="text-sm font-medium">{account.label}</h5>
              <Badge variant="secondary">
                {t(`minimaxCode.status.${account.status}`)}
              </Badge>
            </div>
            <p className="break-all text-xs text-muted-foreground">
              {t(`minimaxCode.regions.${account.region}`)} ·{" "}
              {account.realUserId}
            </p>
            <div className="text-xs text-muted-foreground">
              <p>
                {t("minimaxCode.entitlement.remaining", {
                  amount:
                    account.entitlement?.remainingCredits ??
                    t("minimaxCode.entitlement.unknown"),
                })}
              </p>
              <p>
                {t("minimaxCode.entitlement.plan", {
                  status: t(
                    account.entitlement?.hasTokenPlan === true
                      ? "minimaxCode.entitlement.active"
                      : account.entitlement?.hasTokenPlan === false
                        ? "minimaxCode.entitlement.inactive"
                        : "minimaxCode.entitlement.unknown",
                  ),
                })}
              </p>
              {account.entitlement?.quotaState === "not-subscribed" ? (
                <p>{t("minimaxCode.entitlement.quotaNotSubscribed")}</p>
              ) : (
                (["fiveHour", "weekly"] as const).map((kind) => {
                  const window =
                    account.entitlement?.quotaState === "available"
                      ? account.entitlement.quota?.[kind]
                      : undefined;
                  const remaining = window?.unlimited
                    ? t("minimaxCode.entitlement.quotaUnlimited")
                    : window?.remainingPercent != null
                      ? t("minimaxCode.entitlement.quotaPercent", {
                          percent: window.remainingPercent,
                        })
                      : t("minimaxCode.entitlement.unknown");
                  return (
                    <div key={kind}>
                      <p>
                        {t(
                          kind === "fiveHour"
                            ? "minimaxCode.entitlement.quotaFiveHour"
                            : "minimaxCode.entitlement.quotaWeekly",
                          { remaining },
                        )}
                      </p>
                      {window?.resetAtMs != null && (
                        <p>
                          {t("minimaxCode.entitlement.quotaReset", {
                            time: date(window.resetAtMs),
                          })}
                        </p>
                      )}
                    </div>
                  );
                })
              )}
              {account.entitlement?.tokenPlanExpiresAt != null && (
                <p>
                  {t("minimaxCode.entitlement.expires", {
                    time: date(account.entitlement.tokenPlanExpiresAt),
                  })}
                </p>
              )}
            </div>
            {account.creditAmount !== null && (
              <p className="text-sm">
                {t("minimaxCode.creditAmount", {
                  amount: account.creditAmount,
                })}
              </p>
            )}
            {account.operation && (
              <div className="space-y-1 rounded-md border p-2 text-sm">
                <p>{t(`minimaxCode.operation.${account.operation.status}`)}</p>
                <p>
                  {t("minimaxCode.startedAt", {
                    time: date(account.operation.startedAt),
                  })}
                </p>
                <p className="break-all text-xs text-muted-foreground">
                  {t("minimaxCode.attempt", {
                    id: account.operation.attemptId,
                  })}
                </p>
              </div>
            )}
            {account.status === "unknown" && (
              <p className="text-sm text-amber-600">
                {t("minimaxCode.unknownHint")}
              </p>
            )}
            {account.lastCheckedAt !== null && (
              <p className="text-xs text-muted-foreground">
                {t("minimaxCode.lastCheckedAt", {
                  time: date(account.lastCheckedAt),
                })}
              </p>
            )}
            {account.lastObservedStatus !== null && (
              <p className="text-xs text-muted-foreground">
                {t(`minimaxCode.observed.${account.lastObservedStatus}`)}
              </p>
            )}
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={
                  loading || busy.has(account.id) || !account.canRefresh
                }
                onClick={() => void runAccount(account, "refresh")}
              >
                {t("minimaxCode.refresh")}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={loading || busy.has(account.id) || !account.canClaim}
                onClick={() => void runAccount(account, "claim")}
              >
                {t("minimaxCode.claim")}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={
                  loading || busy.has(account.id) || !account.canRecheck
                }
                onClick={() => void runAccount(account, "recheck")}
              >
                {t("minimaxCode.recheck")}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="ghost"
                disabled={loading || busy.has(account.id) || !account.canRetire}
                onClick={() => void runAccount(account, "retire")}
              >
                {t("minimaxCode.retire")}
              </Button>
            </div>
          </article>
        ))}
      </div>
    </div>
  );
}
