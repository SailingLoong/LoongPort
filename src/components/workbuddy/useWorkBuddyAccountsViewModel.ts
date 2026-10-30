import { confirmsWorkBuddyAutomaticPolicy } from "@/components/relay/accounts/automaticClaimSettings";
import { useCallback, useEffect, useRef, useState } from "react";
import { useAutomaticClaimStatus } from "@/hooks/useAutomaticClaimStatus";
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
/** Owns native account commands and their async lifetimes; DTOs remain backend-owned. */
export function useWorkBuddyAccountsViewModel() {
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
  const automaticUncertainRef = useRef(automaticUncertain);
  const reconcileAutomatic = useCallback((rows: WorkBuddyAccount[]) => {
    const next = { ...automaticUncertainRef.current };
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
    automaticUncertainRef.current = next;
    setAutomaticUncertain(next);
  }, []);
  const setAutomatic = async (account: WorkBuddyAccount) => {
    if (
      automaticActive.current ||
      automaticUncertainRef.current[account.id] ||
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
        !confirmsWorkBuddyAutomaticPolicy(
          account,
          updated,
          !account.autoClaimEnabled,
        )
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
      const uncertain = {
        ...automaticUncertainRef.current,
        [account.id]: {
          generation: account.generation,
          revision: account.autoClaimRevision,
        },
      };
      automaticUncertainRef.current = uncertain;
      setAutomaticUncertain(uncertain);
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
      const reused = await workbuddyApi.setInferenceConsent(accountId, true);
      if (!current()) return;
      if (reused) {
        setInferenceTarget(null);
        await loadAccounts();
        await loadInferenceAccounts();
        return;
      }
      // Only legacy/incomplete or expired credentials require a browser step.
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
  const disableInference = async (accountId: string) => {
    if (inferenceOperation.current) return;
    closeInference();
    const generation = inferenceGeneration.current;
    inferenceOperation.current = `disable:${accountId}`;
    setInferenceBusy("begin");
    try {
      await workbuddyApi.setInferenceConsent(accountId, false);
      if (alive.current && generation === inferenceGeneration.current)
        await loadInferenceAccounts();
    } catch {
      if (alive.current && generation === inferenceGeneration.current)
        setInferenceFailed(true);
    } finally {
      if (alive.current && generation === inferenceGeneration.current) {
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
  const loadAccounts = useCallback(
    async (passive = false) => {
      const generation = ++listGeneration.current;
      const isCurrent = () =>
        alive.current && generation === listGeneration.current;
      setFailed(false);
      try {
        const rows = await workbuddyApi.list();
        if (isCurrent()) {
          setAccounts((old) => mergeAccounts(old, rows));
          reconcileAutomatic(rows);
          if (!passive) setClaimResults(null);
        }
      } catch {
        if (isCurrent()) setFailed(true);
      } finally {
        if (isCurrent()) setLoading(false);
      }
    },
    [reconcileAutomatic],
  );
  const automaticStatus = useAutomaticClaimStatus("workbuddy", () =>
    loadAccounts(true),
  );
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
        automaticUncertainRef.current = {};
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
    // A pre-completion local read cannot replace this newer command receipt.
    listGeneration.current += 1;
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
  const canClaimAll = !loading && accounts.some((row) => row.canClaim);
  const claimAll = async () => {
    if (!canClaimAll) return;
    await run("claimAll", async () => {
      setClaimResults(null);
      const results = await workbuddyApi.claimAll();
      if (!alive.current) return;
      listGeneration.current += 1;
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
        results.map(({ account, error }) => ({ id: account.id, error })),
      );
    });
  };
  const refreshAll = () =>
    run("all", async () => {
      const rows = await workbuddyApi.refreshAll();
      if (alive.current) {
        listGeneration.current += 1;
        setAccounts((old) => mergeAccounts(old, rows));
        reconcileAutomatic(rows);
        setClaimResults(null);
      }
    });
  const refreshAccount = async (account: WorkBuddyAccount) => {
    if (!account.canRefresh || loading) return;
    await run(account.id, async () =>
      update(await workbuddyApi.refresh(account.id)),
    );
  };
  const claimAccount = async (account: WorkBuddyAccount) => {
    if (!account.canClaim || loading) return;
    await run(account.id, async () =>
      update(await workbuddyApi.claim(account.id)),
    );
  };
  const openLogin = async () => {
    const flow = loginFlow.current;
    if (!flow) return;
    const generation = loginGeneration.current;
    try {
      await settingsApi.openExternal(flow.verificationUri);
    } catch {
      if (alive.current && generation === loginGeneration.current)
        setFailed(true);
    }
  };
  const openInference = async () => {
    const flow = inferenceFlow.current;
    if (!flow) return;
    const generation = inferenceGeneration.current;
    try {
      await settingsApi.openExternal(flow.verificationUri);
    } catch {
      if (
        alive.current &&
        generation === inferenceGeneration.current &&
        inferenceFlow.current?.flowId === flow.flowId
      )
        setInferenceFailed(true);
    }
  };
  const automaticDisabled = (account: WorkBuddyAccount) =>
    automaticBusy ||
    Boolean(automaticUncertain[account.id]) ||
    busy !== null ||
    account.generation === undefined ||
    account.autoClaimRevision === undefined ||
    account.autoClaimPolicyVersion === undefined ||
    (!account.canSetAutomatic && !account.autoClaimEnabled);
  const inferenceState = (id: string) =>
    inferenceAccounts.find((row) => row.id === id)?.state ??
    "needsAuthorization";
  const canDisableInference = (id: string) =>
    ["ready", "refreshRequired"].includes(inferenceState(id));
  const canDiscoverModels = (id: string) =>
    busy === null &&
    inferenceBusy === null &&
    inferenceLogin === null &&
    canDisableInference(id);
  return {
    accounts,
    loading,
    failed,
    busy,
    automaticBusy,
    automaticFailed,
    automaticUncertain,
    login,
    waiting,
    inferenceAccounts,
    inferenceLogin,
    inferenceTarget,
    inferenceBusy,
    inferenceWaiting,
    inferenceFailed,
    models,
    claimResults,
    automaticStatus,
    loadAccounts,
    beginLogin,
    finishLogin,
    closeLogin,
    beginInference,
    closeInference,
    disableInference,
    pollInference,
    discoverModels,
    setAutomatic,
    claimAll,
    canClaimAll,
    refreshAll,
    refreshAccount,
    claimAccount,
    openLogin,
    openInference,
    automaticDisabled,
    inferenceState,
    canDisableInference,
    canDiscoverModels,
  };
}
