import { useCallback, useEffect, useRef, useState } from "react";
import { useAutomaticClaimStatus } from "@/hooks/useAutomaticClaimStatus";
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
type AccountAction = "refresh" | "claim" | "recheck" | "retire";

/** Owns catalog ordering and native command lifetimes; backend owns eligibility. */
export function useMiniMaxCodeViewModel(edition?: MiniMaxCodeRegion) {
  const [accounts, setAccounts] = useState<MiniMaxCodeAccount[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<MiniMaxCodeFailure | null>(null);
  const [busy, setBusy] = useState<Set<string>>(new Set());
  const [region, setRegion] = useState<MiniMaxCodeRegion>(edition ?? "cn");
  const [login, setLogin] = useState<MiniMaxCodeLogin | null>(null);
  const [loginBusy, setLoginBusy] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const alive = useRef(true);
  const listGeneration = useRef(0);
  const activeAccounts = useRef(new Set<string>());
  const loginGeneration = useRef(0);
  const activeLogin = useRef<MiniMaxCodeLogin | null>(null);
  const loginRequest = useRef(false);
  // Only refresh responses waiting to be published, not a second account catalog.
  const pendingEntitlements = useRef(new Map<string, MiniMaxCodeAccount>());

  const loadAccounts = useCallback(
    async (refreshed?: MiniMaxCodeAccount, passive = false) => {
      if (refreshed?.entitlement)
        pendingEntitlements.current.set(refreshed.id, refreshed);
      const generation = ++listGeneration.current;
      const isCurrent = () =>
        alive.current && generation === listGeneration.current;
      setLoading(true);
      try {
        const rows = await minimaxCodeApi.list();
        if (!isCurrent()) return;
        const pending = new Map(pendingEntitlements.current);
        pendingEntitlements.current.clear();
        setAccounts((previous) =>
          rows.map((row) => {
            // Catalog reads do not fetch entitlements. Keep an explicit refresh only
            // for exactly the same identity, never across relogin or credential changes.
            const cached =
              pending.get(row.id) ??
              refreshed ??
              (passive ? previous.find((old) => old.id === row.id) : undefined);
            return !row.entitlement &&
              cached?.entitlement &&
              row.authorized &&
              row.id === cached.id &&
              row.authGeneration === cached.authGeneration &&
              row.credentialVersion === cached.credentialVersion &&
              row.realUserId === cached.realUserId &&
              row.region === cached.region
              ? { ...row, entitlement: cached.entitlement }
              : row;
          }),
        );
        setLoaded(true);
      } catch (cause) {
        if (!isCurrent()) return;
        pendingEntitlements.current.clear();
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
    },
    [],
  );
  const automaticStatus = useAutomaticClaimStatus("minimaxCode", () =>
    loadAccounts(undefined, true),
  );

  useEffect(() => {
    alive.current = true;
    void loadAccounts();
    return () => {
      alive.current = false;
      pendingEntitlements.current.clear();
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
    action: AccountAction | "enableAuto" | "disableAuto",
  ) => {
    if (!loaded || loading || activeAccounts.current.has(account.id)) return;
    if (
      (action === "claim" && !account.canClaim) ||
      (action === "refresh" && !account.canRefresh) ||
      (action === "recheck" && !account.canRecheck) ||
      (action === "retire" && !account.canRetire) ||
      ((action === "enableAuto" || action === "disableAuto") &&
        !account.authorized &&
        !account.autoClaimEnabled)
    )
      return;
    activeAccounts.current.add(account.id);
    if (action === "retire") pendingEntitlements.current.delete(account.id);
    setBusy(new Set(activeAccounts.current));
    setError(null);
    // A list requested before this action must not replace its newer projection.
    listGeneration.current += 1;
    try {
      const refreshed =
        action === "enableAuto" || action === "disableAuto"
          ? await minimaxCodeApi.setAutomatic(account, action === "enableAuto")
          : await minimaxCodeApi[action](account.id);
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

  const visibleAccounts = edition
    ? accounts.filter((account) => account.region === edition)
    : accounts;
  const chooseRegion = (value: MiniMaxCodeRegion) => {
    if (loginRequest.current || activeLogin.current) return;
    loginGeneration.current += 1;
    setRegion(value);
  };
  const reload = () => {
    setError(null);
    return loadAccounts();
  };
  const openLogin = async () => {
    const flow = activeLogin.current;
    if (!flow) return;
    const generation = loginGeneration.current;
    try {
      await settingsApi.openExternal(flow.verificationUri);
    } catch {
      if (alive.current && generation === loginGeneration.current)
        setError("networkUnavailable");
    }
  };
  const toggleAutomatic = (account: MiniMaxCodeAccount) =>
    runAccount(
      account,
      account.autoClaimEnabled ? "disableAuto" : "enableAuto",
    );
  return {
    visibleAccounts,
    loaded,
    loading,
    error,
    busy,
    region,
    login,
    loginBusy,
    waiting,
    automaticStatus,
    loadAccounts,
    runAccount,
    beginLogin,
    cancelLogin,
    pollLogin,
    chooseRegion,
    reload,
    openLogin,
    toggleAutomatic,
  };
}
