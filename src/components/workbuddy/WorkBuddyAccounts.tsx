import {
  AccountWorkspace,
  AccountDetailSections,
} from "@/components/relay/accounts/AccountWorkspace";
import { useCallback, useEffect, useRef, useState } from "react";
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
  type WorkBuddyClaimResult,
  type WorkBuddyLogin,
  type WorkBuddyInferenceLogin,
  type WorkBuddyInferenceAccount,
  type WorkBuddyInferenceModel,
} from "@/lib/api/workbuddy";
import { settingsApi } from "@/lib/api/settings";
const amount = (value: number | null) =>
  value == null
    ? "—"
    : value.toLocaleString(undefined, { maximumFractionDigits: 2 });
const date = (value: number | null) =>
  value == null ? "—" : new Date(value).toLocaleString();
// Order backend snapshots only. An older list/refresh cannot undo a confirmed
// policy mutation; identity generations always take precedence over revisions.
function latestAccount(
  previous: WorkBuddyAccount | undefined,
  incoming: WorkBuddyAccount,
) {
  if (
    previous &&
    (previous.generation > incoming.generation ||
      (previous.generation === incoming.generation &&
        previous.autoClaimRevision > incoming.autoClaimRevision))
  )
    return previous;
  return incoming;
}
function mergeAccounts(
  previous: WorkBuddyAccount[],
  incoming: WorkBuddyAccount[],
) {
  return incoming.map((row) =>
    latestAccount(
      previous.find((old) => old.id === row.id),
      row,
    ),
  );
}
export function WorkBuddyAccounts({
  unified = false,
}: { unified?: boolean } = {}) {
  const { t } = useTranslation();
  const [accounts, setAccounts] = useState<WorkBuddyAccount[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const active = useRef(false);
  const alive = useRef(true);
  const listGeneration = useRef(0);
  const policyLifecycle = useRef(0);
  const automaticActive = useRef(false);
  const [automaticBusy, setAutomaticBusy] = useState(false);
  const [automaticFailed, setAutomaticFailed] = useState(false);
  const [automaticUncertain, setAutomaticUncertain] = useState<
    Record<string, { generation: number; revision: number } | undefined>
  >({});
  const reconcileAutomatic = useCallback((rows: WorkBuddyAccount[]) => {
    setAutomaticUncertain((old) => {
      const next = { ...old };
      for (const row of rows) {
        const expected = next[row.id];
        if (
          expected &&
          (row.generation > expected.generation ||
            (row.generation === expected.generation &&
              row.autoClaimRevision > expected.revision))
        )
          delete next[row.id];
      }
      return next;
    });
  }, []);
  const setAutomatic = async (account: WorkBuddyAccount) => {
    if (
      automaticActive.current ||
      automaticUncertain[account.id] ||
      busy !== null ||
      account.generation === undefined ||
      account.autoClaimRevision === undefined ||
      account.autoClaimPolicyVersion === undefined ||
      (!account.canSetAutomatic && !account.autoClaimEnabled)
    )
      return;
    automaticActive.current = true;
    setAutomaticBusy(true);
    setAutomaticFailed(false);
    const lifecycle = policyLifecycle.current;
    const current = () =>
      alive.current && lifecycle === policyLifecycle.current;
    try {
      const updated = await workbuddyApi.setAutomatic(
        account,
        !account.autoClaimEnabled,
      );
      if (!current()) return;
      if (
        updated.id !== account.id ||
        updated.generation !== account.generation ||
        updated.autoClaimRevision <= account.autoClaimRevision
      )
        throw new Error("Unconfirmed policy response");
      setAccounts((rows) =>
        rows.map((row) =>
          row.id === updated.id ? latestAccount(row, updated) : row,
        ),
      );
    } catch {
      if (!current()) return;
      // IPC failure is not proof the setter failed. Never display the old off/on
      // value as authoritative until a later backend revision resolves it.
      setAutomaticFailed(true);
      setAutomaticUncertain((old) => ({
        ...old,
        [account.id]: {
          generation: account.generation,
          revision: account.autoClaimRevision,
        },
      }));
      try {
        const rows = await workbuddyApi.list();
        if (!current()) return;
        const confirmed = rows.find((row) => row.id === account.id);
        setAccounts((old) => mergeAccounts(old, rows));
        if (
          confirmed &&
          (confirmed.generation > account.generation ||
            (confirmed.generation === account.generation &&
              confirmed.autoClaimRevision > account.autoClaimRevision))
        ) {
          reconcileAutomatic(rows);
          setAutomaticFailed(false);
        }
      } catch {
        // Uncertain remains visible and disabled, not a false "off" confirmation.
      }
    } finally {
      automaticActive.current = false;
      if (current()) setAutomaticBusy(false);
    }
  };
  const [login, setLogin] = useState<WorkBuddyLogin | null>(null);
  const [waiting, setWaiting] = useState(false);
  const loginGeneration = useRef(0);
  const loginFlow = useRef<WorkBuddyLogin | null>(null);
  const loginOperation = useRef<string | null>(null);
  const closeLogin = () => {
    const generation = ++loginGeneration.current;
    const flow = loginFlow.current;
    loginFlow.current = null;
    const hadLoginOperation = loginOperation.current !== null;
    loginOperation.current = null;
    setLogin(null);
    setWaiting(false);
    if (hadLoginOperation) setBusy(null);
    if (flow)
      void workbuddyApi.cancelLogin(flow.flowId).catch(() => {
        if (alive.current && generation === loginGeneration.current)
          setFailed(true);
      });
  };
  const [inferenceAccounts, setInferenceAccounts] = useState<
    WorkBuddyInferenceAccount[]
  >([]);
  const [inferenceLogin, setInferenceLogin] =
    useState<WorkBuddyInferenceLogin | null>(null);
  const [inferenceTarget, setInferenceTarget] = useState<string | null>(null);
  const [inferenceBusy, setInferenceBusy] = useState<string | null>(null);
  const [inferenceWaiting, setInferenceWaiting] = useState(false);
  const [inferenceFailed, setInferenceFailed] = useState(false);
  const [models, setModels] = useState<
    Record<string, WorkBuddyInferenceModel[]>
  >({});
  const inferenceGeneration = useRef(0);
  const inferenceFlow = useRef<WorkBuddyInferenceLogin | null>(null);
  const inferenceOperation = useRef<string | null>(null);
  const inferenceStatusGeneration = useRef(0);
  const loadInferenceAccounts = useCallback(async () => {
    const generation = ++inferenceStatusGeneration.current;
    const operationGeneration = inferenceGeneration.current;
    try {
      const rows = await workbuddyApi.listInferenceAccounts();
      if (alive.current && generation === inferenceStatusGeneration.current) {
        setInferenceAccounts(rows);
      }
    } catch {
      if (alive.current && generation === inferenceStatusGeneration.current) {
        setInferenceAccounts([]);
        if (operationGeneration === inferenceGeneration.current)
          setInferenceFailed(true);
      }
    }
  }, []);
  const closeInference = () => {
    const generation = ++inferenceGeneration.current;
    const flow = inferenceFlow.current;
    inferenceFlow.current = null;
    inferenceOperation.current = null;
    setInferenceLogin(null);
    setInferenceTarget(null);
    setInferenceBusy(null);
    setInferenceWaiting(false);
    if (flow)
      void workbuddyApi.cancelInferenceLogin(flow.flowId).catch(() => {
        if (alive.current && generation === inferenceGeneration.current)
          setInferenceFailed(true);
      });
  };
  const beginInference = async (accountId: string) => {
    if (inferenceOperation.current === `begin:${accountId}`) return;
    const generation = ++inferenceGeneration.current;
    const current = () =>
      alive.current && generation === inferenceGeneration.current;
    const previous = inferenceFlow.current;
    inferenceFlow.current = null;
    inferenceOperation.current = `begin:${accountId}`;
    setInferenceLogin(null);
    setInferenceTarget(accountId);
    setInferenceBusy("begin");
    setInferenceWaiting(false);
    setInferenceFailed(false);
    setModels((old) => {
      const next = { ...old };
      delete next[accountId];
      return next;
    });
    try {
      if (previous) await workbuddyApi.cancelInferenceLogin(previous.flowId);
      if (!current()) return;
      const flow = await workbuddyApi.beginInferenceLogin(accountId);
      if (!current() || flow.accountId !== accountId) {
        await workbuddyApi.cancelInferenceLogin(flow.flowId);
        if (current()) setInferenceFailed(true);
        return;
      }
      inferenceFlow.current = flow;
      setInferenceLogin(flow);
    } catch {
      if (current()) setInferenceFailed(true);
    } finally {
      if (current()) {
        inferenceOperation.current = null;
        setInferenceBusy(null);
      }
    }
  };
  const pollInference = async () => {
    const flow = inferenceFlow.current;
    if (!flow || inferenceOperation.current) return;
    const generation = inferenceGeneration.current;
    const current = () =>
      alive.current && generation === inferenceGeneration.current;
    inferenceOperation.current = "poll";
    setInferenceBusy("poll");
    setInferenceFailed(false);
    try {
      const result = await workbuddyApi.pollInferenceLogin(flow.flowId);
      if (!current()) return;
      if (result.state === "saved") {
        inferenceFlow.current = null;
        setInferenceLogin(null);
        setInferenceTarget(null);
        setInferenceWaiting(false);
        await loadInferenceAccounts();
      } else setInferenceWaiting(true);
    } catch {
      if (current()) {
        inferenceFlow.current = null;
        setInferenceLogin(null);
        setInferenceFailed(true);
        // Also releases a backend flow when the command failed before polling.
        void workbuddyApi.cancelInferenceLogin(flow.flowId).catch(() => {});
      }
    } finally {
      if (current()) {
        inferenceOperation.current = null;
        setInferenceBusy(null);
      }
    }
  };
  const discoverModels = async (accountId: string) => {
    if (inferenceOperation.current || inferenceFlow.current) return;
    const generation = ++inferenceGeneration.current;
    const current = () =>
      alive.current && generation === inferenceGeneration.current;
    inferenceOperation.current = "models";
    setInferenceTarget(accountId);
    setInferenceBusy("models");
    setInferenceFailed(false);
    setModels((old) => {
      const next = { ...old };
      delete next[accountId];
      return next;
    });
    try {
      const discovered = await workbuddyApi.discoverModels(accountId);
      if (current()) setModels((old) => ({ ...old, [accountId]: discovered }));
    } catch {
      if (current()) setInferenceFailed(true);
    } finally {
      if (current()) {
        inferenceOperation.current = null;
        setInferenceBusy(null);
      }
    }
  };

  const [claimResults, setClaimResults] = useState<
    { id: string; error: WorkBuddyClaimResult["error"] }[] | null
  >(null);
  const loadAccounts = useCallback(async () => {
    const generation = ++listGeneration.current;
    const isCurrent = () =>
      alive.current && generation === listGeneration.current;
    setFailed(false);
    try {
      const rows = await workbuddyApi.list();
      if (isCurrent()) {
        setAccounts((old) => mergeAccounts(old, rows));
        reconcileAutomatic(rows);
        setClaimResults(null);
      }
    } catch {
      if (isCurrent()) setFailed(true);
    } finally {
      if (isCurrent()) setLoading(false);
    }
  }, [reconcileAutomatic]);
  useEffect(() => {
    alive.current = true;
    void loadAccounts();
    void loadInferenceAccounts();
    return () => {
      alive.current = false;
      loginGeneration.current += 1;
      const pendingLogin = loginFlow.current;
      loginFlow.current = null;
      loginOperation.current = null;
      if (pendingLogin)
        void workbuddyApi.cancelLogin(pendingLogin.flowId).catch(() => {});
      policyLifecycle.current += 1;
      listGeneration.current += 1;
      inferenceGeneration.current += 1;
      inferenceStatusGeneration.current += 1;
      const flow = inferenceFlow.current;
      inferenceFlow.current = null;
      if (flow)
        void workbuddyApi.cancelInferenceLogin(flow.flowId).catch(() => {});
    };
  }, [loadAccounts, loadInferenceAccounts]);
  const run = async (key: string, operation: () => Promise<void>) => {
    if (active.current || loginOperation.current) return;
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
  const beginLogin = async () => {
    if (active.current || loginOperation.current) return;
    const generation = ++loginGeneration.current;
    const current = () =>
      alive.current && generation === loginGeneration.current;
    const previous = loginFlow.current;
    loginFlow.current = null;
    loginOperation.current = "begin";
    setLogin(null);
    setWaiting(false);
    setBusy("login");
    setFailed(false);
    try {
      if (previous) await workbuddyApi.cancelLogin(previous.flowId);
      if (!current()) return;
      const flow = await workbuddyApi.beginLogin();
      if (!current()) {
        await workbuddyApi.cancelLogin(flow.flowId);
        return;
      }
      loginFlow.current = flow;
      setLogin(flow);
    } catch {
      if (current()) setFailed(true);
    } finally {
      if (current()) {
        loginOperation.current = null;
        setBusy(null);
      }
    }
  };
  const finishLogin = async () => {
    const flow = loginFlow.current;
    if (!flow || active.current || loginOperation.current) return;
    const generation = loginGeneration.current;
    const current = () =>
      alive.current && generation === loginGeneration.current;
    loginOperation.current = "finish";
    setBusy("finish");
    setFailed(false);
    try {
      const result = await workbuddyApi.finishLogin(flow.flowId);
      if (!current()) return;
      if (result.state === "saved") {
        loginFlow.current = null;
        setLogin(null);
        setWaiting(false);
        policyLifecycle.current += 1;
        setAutomaticBusy(false);
        setAutomaticFailed(false);
        setAutomaticUncertain({});
        // A legacy login may replace the row and clear its inference grant.
        closeInference();
        inferenceStatusGeneration.current += 1;
        setInferenceAccounts([]);
        setInferenceFailed(false);
        setModels({});
        await loadAccounts();
        if (current()) await loadInferenceAccounts();
      } else setWaiting(true);
    } catch {
      if (current()) setFailed(true);
    } finally {
      if (current()) {
        loginOperation.current = null;
        setBusy(null);
      }
    }
  };
  const update = (row: WorkBuddyAccount) => {
    if (!alive.current) return;
    reconcileAutomatic([row]);
    setAccounts((rows) =>
      rows.some((old) => old.id === row.id)
        ? rows.map((old) => (old.id === row.id ? latestAccount(old, row) : old))
        : [...rows, row],
    );
    setClaimResults(
      (results) =>
        results?.map((result) =>
          result.id === row.id ? { ...result, error: null } : result,
        ) ?? null,
    );
  };
  return (
    <section aria-label={t("workbuddy.title")} className="space-y-3">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-medium">{t("workbuddy.title")}</h2>
        <div className="flex gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={
              busy !== null || loading || !accounts.some((row) => row.canClaim)
            }
            onClick={() =>
              void run("claimAll", async () => {
                setClaimResults(null);
                const results = await workbuddyApi.claimAll();
                if (!alive.current) return;
                setAccounts((rows) => {
                  const updated = new Map(rows.map((row) => [row.id, row]));
                  for (const { account } of results)
                    updated.set(
                      account.id,
                      latestAccount(updated.get(account.id), account),
                    );
                  return [...updated.values()];
                });
                setClaimResults(
                  results.map(({ account, error }) => ({
                    id: account.id,
                    error,
                  })),
                );
              })
            }
          >
            {busy === "claimAll" && (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            )}
            {t(
              busy === "claimAll"
                ? "workbuddy.claimingAll"
                : "workbuddy.claimAll",
            )}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={busy !== null || loading || accounts.length === 0}
            onClick={() =>
              void run("all", async () => {
                const rows = await workbuddyApi.refreshAll();
                if (alive.current) {
                  setAccounts((old) => mergeAccounts(old, rows));
                  reconcileAutomatic(rows);
                  setClaimResults(null);
                }
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
            onClick={() => void beginLogin()}
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
      <p className="text-xs text-muted-foreground">
        {t("workbuddy.auto.notice")}
      </p>
      {automaticFailed && (
        <p role="alert" className="text-sm text-destructive">
          {t("workbuddy.auto.failed")}
        </p>
      )}
      {inferenceFailed && (
        <p role="alert" className="text-sm text-destructive">
          {t("workbuddy.inference.failed", {
            defaultValue:
              "Inference authorization or model discovery failed. Select the matching account and try again.",
          })}
        </p>
      )}
      {inferenceTarget && (inferenceLogin || inferenceBusy === "begin") && (
        <div
          role="region"
          aria-label={t("workbuddy.inference.authorization", {
            defaultValue: "Inference authorization",
          })}
          className="space-y-2 rounded-xl border p-3 text-sm"
        >
          <p>{accounts.find((row) => row.id === inferenceTarget)?.label}</p>
          <p>
            {t("workbuddy.inference.identityNotice", {
              defaultValue:
                "Authorize this same WorkBuddy account in the same region. The existing check-in login is retained.",
            })}
          </p>
          {inferenceLogin && (
            <>
              <a
                href={inferenceLogin.verificationUri}
                className="text-primary underline"
                onClick={(event) => {
                  event.preventDefault();
                  const generation = inferenceGeneration.current;
                  const flowId = inferenceLogin.flowId;
                  void settingsApi
                    .openExternal(inferenceLogin.verificationUri)
                    .catch(() => {
                      if (
                        alive.current &&
                        generation === inferenceGeneration.current &&
                        inferenceFlow.current?.flowId === flowId
                      )
                        setInferenceFailed(true);
                    });
                }}
              >
                {t("workbuddy.inference.open", {
                  defaultValue: "Open inference authorization",
                })}
              </a>
              <Button
                size="sm"
                variant="outline"
                disabled={inferenceBusy !== null}
                onClick={() => void pollInference()}
              >
                {t("workbuddy.inference.finish", {
                  defaultValue: "I have authorized this account",
                })}
              </Button>
            </>
          )}
          <Button size="sm" variant="ghost" onClick={closeInference}>
            {t("workbuddy.inference.cancel", {
              defaultValue: "Cancel authorization",
            })}
          </Button>
          {inferenceWaiting && (
            <p role="status">{t("workbuddy.authorizationWaiting")}</p>
          )}
        </div>
      )}
      {(login || busy === "login") && (
        <Button size="sm" variant="ghost" onClick={closeLogin}>
          {t("common.cancel")}
        </Button>
      )}
      {login && (
        <div className="flex flex-wrap items-center gap-3 rounded-xl border border-border p-3 text-sm">
          <a
            className="text-primary underline"
            href={login.verificationUri}
            onClick={(event) => {
              event.preventDefault();
              const generation = loginGeneration.current;
              void settingsApi.openExternal(login.verificationUri).catch(() => {
                if (alive.current && generation === loginGeneration.current)
                  setFailed(true);
              });
            }}
          >
            {t("workbuddy.authorize")}
          </a>
          <Button
            size="sm"
            variant="outline"
            disabled={busy !== null}
            onClick={() => void finishLogin()}
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
      {claimResults !== null && (
        <div
          role="region"
          aria-label={t("workbuddy.claimResults")}
          className="rounded-xl border border-border p-3 text-sm"
        >
          <h3 className="font-medium">{t("workbuddy.claimResults")}</h3>
          {claimResults.length === 0 ? (
            <p role="status" className="mt-2 text-muted-foreground">
              {t("workbuddy.claimNone")}
            </p>
          ) : (
            <ul className="mt-2 space-y-2">
              {claimResults.map((result) => {
                const account = accounts.find((row) => row.id === result.id);
                return account ? (
                  <li
                    key={result.id}
                    className="flex flex-wrap justify-between gap-2"
                  >
                    <span>{account.label}</span>
                    <span
                      role="status"
                      className={
                        result.error
                          ? "text-destructive"
                          : "text-muted-foreground"
                      }
                    >
                      {result.error
                        ? t("workbuddy.claimFailed")
                        : t(`workbuddy.state.${account.claimState}`)}
                    </span>
                  </li>
                ) : null;
              })}
            </ul>
          )}
        </div>
      )}
      <div className="grid gap-2">
        {
          <AccountWorkspace
            accounts={accounts}
            label={(account) => account.label}
            enabled={unified}
          >
            {(account) => {
              const actions = (
                <div className="flex flex-wrap items-center justify-between gap-3">
                  {!unified && (
                    <h3 className="text-sm font-medium">{account.label}</h3>
                  )}
                  <div className="flex flex-wrap gap-2">
                    <Button
                      role={
                        automaticUncertain[account.id] ? "button" : "switch"
                      }
                      aria-label={t(
                        automaticUncertain[account.id]
                          ? "workbuddy.auto.unknown"
                          : "workbuddy.auto.label",
                      )}
                      aria-checked={
                        automaticUncertain[account.id]
                          ? undefined
                          : account.autoClaimEnabled === true
                      }
                      size="sm"
                      variant="outline"
                      disabled={
                        automaticBusy ||
                        Boolean(automaticUncertain[account.id]) ||
                        busy !== null ||
                        account.generation === undefined ||
                        account.autoClaimRevision === undefined ||
                        account.autoClaimPolicyVersion === undefined ||
                        (!account.canSetAutomatic && !account.autoClaimEnabled)
                      }
                      onClick={() => void setAutomatic(account)}
                    >
                      {t(
                        automaticUncertain[account.id]
                          ? "workbuddy.auto.unknown"
                          : account.autoClaimEnabled
                            ? "workbuddy.auto.on"
                            : "workbuddy.auto.off",
                      )}
                    </Button>
                    {account.autoClaimBlocked && (
                      <span
                        role="status"
                        className="text-xs text-muted-foreground"
                      >
                        {t("workbuddy.auto.blocked")}
                      </span>
                    )}
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
              );
              const authorization = (
                <>
                  <p role="status">
                    {t(
                      `workbuddy.inference.state.${inferenceAccounts.find((row) => row.id === account.id)?.state ?? "needsAuthorization"}`,
                      {
                        defaultValue: (
                          {
                            needsAuthorization:
                              "New inference authorization required",
                            ready: "Inference authorization available",
                            refreshRequired:
                              "Inference authorization needs refresh",
                            expired: "Inference authorization expired",
                          } as const
                        )[
                          inferenceAccounts.find((row) => row.id === account.id)
                            ?.state ?? "needsAuthorization"
                        ],
                      },
                    )}
                  </p>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={
                      busy !== null ||
                      (inferenceTarget === account.id &&
                        inferenceBusy === "begin")
                    }
                    onClick={() => void beginInference(account.id)}
                  >
                    {t("workbuddy.inference.authorize", {
                      defaultValue: "Authorize inference",
                    })}
                  </Button>
                </>
              );
              const inference = (
                <>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={
                      busy !== null ||
                      inferenceBusy !== null ||
                      inferenceLogin !== null ||
                      !inferenceAccounts.some(
                        (row) =>
                          row.id === account.id &&
                          (row.state === "ready" ||
                            row.state === "refreshRequired"),
                      )
                    }
                    onClick={() => void discoverModels(account.id)}
                  >
                    {t("workbuddy.inference.models", {
                      defaultValue: "Discover inference models",
                    })}
                  </Button>
                  {models[account.id] && (
                    <ul
                      aria-label={t("workbuddy.inference.availableModels", {
                        defaultValue: "Available inference models",
                      })}
                    >
                      {models[account.id].map((model) => (
                        <li key={model.id}>
                          {model.name} · {model.id}
                        </li>
                      ))}
                    </ul>
                  )}
                </>
              );
              const credits = (
                <>
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
                      <dd className="mt-1">
                        {date(account.credits.nearestExpiry)}
                      </dd>
                    </div>
                    <div>
                      <dt>{t("workbuddy.updated")}</dt>
                      <dd className="mt-1">
                        {date(account.credits.updatedAt)}
                      </dd>
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
                                {t("workbuddy.remaining")}{" "}
                                {amount(pack.remaining)} ·{" "}
                                {t("workbuddy.expires")} {date(pack.expireAt)}
                              </span>
                            </li>
                          ))}
                        </ul>
                      </CollapsibleContent>
                    </Collapsible>
                  )}
                </>
              );
              if (unified)
                return (
                  <AccountDetailSections
                    authorization={authorization}
                    entitlements={
                      <>
                        {actions}
                        {credits}
                      </>
                    }
                    inference={
                      <>
                        {inference}
                        <p className="text-xs text-muted-foreground">
                          真实推理消费尚未验收。领取成功、授权就绪或模型发现均不等于积分已用于推理。
                        </p>
                      </>
                    }
                  />
                );
              return (
                <article className="rounded-xl border border-border bg-card p-3">
                  {actions}
                  <div className="mt-3 space-y-2 border-t pt-3 text-xs">
                    {authorization}
                    {inference}
                  </div>
                  {credits}
                </article>
              );
            }}
          </AccountWorkspace>
        }
      </div>
    </section>
  );
}
