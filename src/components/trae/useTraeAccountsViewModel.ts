import { confirmsTraeAutomaticPolicy } from "@/components/relay/accounts/automaticClaimSettings";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import {
  traeAuthHost,
  traeAccountsApi,
  type TraeAccountView,
  type TraeFlowView,
  type TraeRealm,
  type TraeInstallation,
} from "@/lib/api/traeAccounts";

import {
  traeAuthorizationFailure,
  type TraeAuthorizationFailure,
} from "@/lib/traeAuthorizationError";

function isSelectedFile(path: unknown, filename: string): path is string {
  return typeof path === "string" && path.split(/[\\/]/).pop() === filename;
}

/** Owns native authorization selection, consent and account command lifetimes. */
export function useTraeAccountsViewModel(edition: "cn" | "global" = "global") {
  const { t } = useTranslation();
  const [accounts, setAccounts] = useState<TraeAccountView[]>([]);
  const [loading, setLoading] = useState(true);
  const [listFailed, setListFailed] = useState(false);
  const [failed, setFailed] = useState<TraeAuthorizationFailure | null>(null);
  const [cancelUncertain, setCancelUncertain] = useState(false);
  const [saved, setSaved] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [realm, setRealm] = useState<TraeRealm | null>(null);
  const [flow, setFlow] = useState<TraeFlowView | null>(null);
  const [installations, setInstallations] = useState<TraeInstallation[]>([]);
  const [discoveryDone, setDiscoveryDone] = useState(false);
  const [consent, setConsent] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [busy, setBusy] = useState<"selecting" | "confirming" | null>(null);
  const [retiring, setRetiring] = useState<string | null>(null);
  const alive = useRef(false);
  const dialogActive = useRef(false);
  const generation = useRef(0);
  const listGeneration = useRef(0);
  const operation = useRef<number | null>(null);
  const activeFlow = useRef<TraeFlowView | null>(null);
  const consentGiven = useRef(false);
  const confirmationAttempted = useRef(false);
  const retireActive = useRef(false);

  const [checkinBusy, setCheckinBusy] = useState(false);
  const [checkinResults, setCheckinResults] = useState<Record<string, string>>(
    {},
  );
  const checkinActive = useRef(false);
  const checkinGeneration = useRef(0);

  const [automaticBusy, setAutomaticBusy] = useState(false);
  const automaticActive = useRef(false);
  const policyLifecycle = useRef(0);
  type UncertainPolicy = { generation: number; revision: number };
  const [uncertainPolicies, setUncertainPolicies] = useState<
    Record<string, UncertainPolicy>
  >({});
  const uncertainPoliciesRef = useRef<Record<string, UncertainPolicy>>({});
  const policyIsUnknown = (
    account: TraeAccountView,
    uncertain = uncertainPolicies,
  ) => {
    const pending = uncertain[account.id];
    return (
      pending !== undefined &&
      (account.generation === undefined ||
        account.generation < pending.generation ||
        (account.generation === pending.generation &&
          (account.autoCheckinRevision === undefined ||
            account.autoCheckinRevision <= pending.revision)))
    );
  };
  const reconcilePolicies = useCallback((rows: TraeAccountView[]) => {
    const next = { ...uncertainPoliciesRef.current };
    let changed = false;
    for (const row of rows) {
      const pending = next[row.id];
      if (
        pending &&
        row.generation !== undefined &&
        row.autoCheckinRevision !== undefined &&
        typeof row.autoCheckinEnabled === "boolean" &&
        (row.generation > pending.generation ||
          (row.generation === pending.generation &&
            row.autoCheckinRevision > pending.revision))
      ) {
        delete next[row.id];
        changed = true;
      }
    }
    if (changed) {
      uncertainPoliciesRef.current = next;
      setUncertainPolicies(next);
    }
  }, []);
  const setAutomatic = async (account: TraeAccountView) => {
    if (
      automaticActive.current ||
      policyIsUnknown(account, uncertainPoliciesRef.current) ||
      retireActive.current ||
      dialogActive.current ||
      account.generation === undefined ||
      account.autoCheckinRevision === undefined ||
      (!account.canSetAutomatic && !account.autoCheckinEnabled)
    )
      return;
    automaticActive.current = true;
    setAutomaticBusy(true);
    const request = policyLifecycle.current;
    const current = () => alive.current && request === policyLifecycle.current;
    try {
      const updated = await traeAccountsApi.setAutomatic(
        account,
        !account.autoCheckinEnabled,
      );
      if (!current()) return;
      if (
        !confirmsTraeAutomaticPolicy(
          account,
          updated,
          !account.autoCheckinEnabled,
        )
      )
        throw new Error("Unconfirmed policy identity");
      reconcilePolicies([updated]);
      setAccounts((rows) =>
        rows.map((row) =>
          row.id === account.id &&
          row.generation === account.generation &&
          row.autoCheckinRevision === account.autoCheckinRevision
            ? updated
            : row,
        ),
      );
    } catch {
      if (current()) {
        const next = {
          ...uncertainPoliciesRef.current,
          [account.id]: {
            generation: account.generation,
            revision: account.autoCheckinRevision,
          },
        };
        uncertainPoliciesRef.current = next;
        setUncertainPolicies(next);
        // A rejected IPC reply does not prove that durable consent was unchanged.
        // Read once, never resend the setter. This new list request also fences
        // every list that started before the uncertain mutation completed.
        await loadAccounts();
      }
    } finally {
      automaticActive.current = false;
      if (alive.current) setAutomaticBusy(false);
    }
  };

  const clearCheckin = useCallback(() => {
    ++checkinGeneration.current;
    setCheckinResults({});
  }, []);

  const checkin = async (account: TraeAccountView) => {
    if (
      checkinActive.current ||
      retireActive.current ||
      dialogActive.current ||
      edition !== "cn" ||
      account.realm !== "cn" ||
      account.status !== "stored" ||
      account.nativeSourceSupported !== true
    )
      return;
    const request = checkinGeneration.current;
    const current = () =>
      alive.current && request === checkinGeneration.current;
    checkinActive.current = true;
    setCheckinBusy(true);
    setCheckinResults((rows) => ({
      ...rows,
      [account.id]: "正在检查并尝试签到…",
    }));
    try {
      const result = await traeAccountsApi.checkin(account.id);
      if (!current()) return;
      const message =
        result.status === "done"
          ? "服务已确认此账号已签到；积分余额仍需另行核对。"
          : result.status === "accepted"
            ? "已受理，尚未确认签到或积分到账。可再次手动核对状态；不会重复领取。"
            : result.status === "inactive"
              ? "此账号当前未开放签到。"
              : result.status === "ineligible"
                ? "服务拒绝本次领取或账号不符合条件，未确认到账。"
                : result.status === "alreadyAttempted"
                  ? "已尝试领取或仍有未确认的领取；不会重复发送，可稍后手动核对状态。"
                  : "无法确认签到结果，不会自动重试。";
      setCheckinResults((rows) => ({ ...rows, [account.id]: message }));
    } catch {
      if (current())
        setCheckinResults((rows) => ({
          ...rows,
          [account.id]:
            "无法确认签到结果，不会自动重试。可稍后手动核对；已尝试的领取不会重复发送。",
        }));
    } finally {
      checkinActive.current = false;
      if (alive.current) setCheckinBusy(false);
    }
  };

  const loadAccounts = useCallback(async () => {
    clearCheckin();
    const request = ++listGeneration.current;
    const isCurrent = () => alive.current && request === listGeneration.current;
    setLoading(true);
    setListFailed(false);
    try {
      const rows = await traeAccountsApi.list();
      if (isCurrent()) {
        reconcilePolicies(rows);
        setAccounts((current) =>
          rows
            .filter((row) =>
              edition === "cn"
                ? row.realm === "cn"
                : row.realm === "sg" || row.realm === "us",
            )
            .map((row) => {
              const prior = current.find((a) => a.id === row.id);
              // Order backend snapshots only; never infer policy or eligibility.
              // A stale list may not reverse a confirmed policy/identity change.
              if (
                prior?.generation !== undefined &&
                row.generation !== undefined &&
                (prior.generation > row.generation ||
                  (prior.generation === row.generation &&
                    prior.autoCheckinRevision !== undefined &&
                    row.autoCheckinRevision !== undefined &&
                    prior.autoCheckinRevision > row.autoCheckinRevision))
              )
                return prior;
              return row;
            }),
        );
      }
    } catch {
      if (isCurrent()) setListFailed(true);
    } finally {
      if (isCurrent()) setLoading(false);
    }
  }, [edition, clearCheckin, reconcilePolicies]);

  const invalidateList = useCallback(() => {
    clearCheckin();
    ++listGeneration.current;
    if (alive.current) {
      setLoading(false);
      setListFailed(false);
    }
  }, [clearCheckin]);

  const cancelFlow = useCallback(
    async (flowId: string) => {
      try {
        await traeAccountsApi.cancel(flowId);
        if (alive.current) void loadAccounts();
      } catch {
        // Never claim cancellation succeeded, nor expose raw native errors.
        if (alive.current) setCancelUncertain(true);
      }
    },
    [loadAccounts],
  );

  useEffect(() => {
    alive.current = true;
    void loadAccounts();
    return () => {
      alive.current = false;
      ++policyLifecycle.current;
      ++checkinGeneration.current;
      dialogActive.current = false;
      ++generation.current;
      ++listGeneration.current;
      operation.current = null;
      const abandoned = activeFlow.current;
      activeFlow.current = null;
      if (abandoned) void cancelFlow(abandoned.flowId);
    };
  }, [cancelFlow, loadAccounts]);

  const beginDialog = () => {
    if (dialogActive.current || retireActive.current) return;
    ++generation.current;
    dialogActive.current = true;
    activeFlow.current = null;
    consentGiven.current = false;
    confirmationAttempted.current = false;
    operation.current = null;
    setRealm(edition === "cn" ? "cn" : null);
    setInstallations([]);
    setDiscoveryDone(false);
    setFlow(null);
    setConsent(false);
    setAttempted(false);
    setBusy(null);
    setFailed(null);
    setSaved(false);
    setDialogOpen(true);
  };

  const closeDialog = () => {
    if (!dialogActive.current) return;
    const review = confirmationAttempted.current && !activeFlow.current;
    dialogActive.current = false;
    ++generation.current;
    operation.current = null;
    consentGiven.current = false;
    const abandoned = activeFlow.current;
    activeFlow.current = null;
    setFlow(null);
    setConsent(false);
    setBusy(null);
    setDialogOpen(false);
    if (abandoned) {
      invalidateList();
      void cancelFlow(abandoned.flowId);
    } else if (review) {
      void loadAccounts();
    }
  };

  const selectFiles = async (installation?: TraeInstallation) => {
    if (
      !realm ||
      !dialogActive.current ||
      operation.current !== null ||
      activeFlow.current
    )
      return;
    const request = generation.current;
    const isCurrent = () =>
      alive.current && dialogActive.current && request === generation.current;
    operation.current = request;
    setBusy("selecting");
    setFailed(null);
    try {
      const storagePath =
        installation?.storagePath ??
        (await open({
          title: t("trae.selectStorage", { defaultValue: "选择 storage.json" }),
          directory: false,
          multiple: false,
          filters: [{ name: "JSON", extensions: ["json"] }],
        }));
      if (!isCurrent() || storagePath === null) return;
      if (!isSelectedFile(storagePath, "storage.json")) {
        setFailed("invalidSelection");
        return;
      }
      const productPath =
        installation?.productPath ??
        (await open({
          title: t("trae.selectProduct", {
            defaultValue: "选择同一安装的 product.json",
          }),
          directory: false,
          multiple: false,
          filters: [{ name: "JSON", extensions: ["json"] }],
        }));
      if (!isCurrent() || productPath === null) return;
      if (!isSelectedFile(productPath, "product.json")) {
        setFailed("invalidSelection");
        return;
      }
      const selected = await traeAccountsApi.begin({
        storagePath,
        productPath,
        realm,
      });
      if (!isCurrent()) {
        if (selected.flowId) void cancelFlow(selected.flowId);
        return;
      }
      // Validate the consent destination before displaying an actionable flow.
      if (
        !selected.flowId ||
        selected.realm !== realm ||
        selected.authHost !== traeAuthHost(realm)
      ) {
        if (selected.flowId) void cancelFlow(selected.flowId);
        setFailed("unknown");
        return;
      }
      activeFlow.current = selected;
      setFlow(selected);
    } catch (error) {
      if (isCurrent()) setFailed(traeAuthorizationFailure(error));
    } finally {
      if (operation.current === request) {
        operation.current = null;
        if (isCurrent()) setBusy(null);
      }
    }
  };

  const discoverFiles = async () => {
    if (
      !realm ||
      !dialogActive.current ||
      operation.current !== null ||
      activeFlow.current
    )
      return;
    const request = generation.current;
    const isCurrent = () =>
      alive.current && dialogActive.current && request === generation.current;
    operation.current = request;
    setBusy("selecting");
    setFailed(null);
    setInstallations([]);
    setDiscoveryDone(false);
    let single: TraeInstallation | undefined;
    try {
      const found = await traeAccountsApi.discover(realm);
      if (!isCurrent()) return;
      setDiscoveryDone(true);
      setInstallations(found);
      if (found.length === 1) single = found[0];
    } catch (error) {
      if (isCurrent()) setFailed(traeAuthorizationFailure(error));
    } finally {
      if (operation.current === request) {
        operation.current = null;
        if (isCurrent()) setBusy(null);
      }
    }
    if (single && isCurrent()) await selectFiles(single);
  };

  const confirm = async () => {
    const selected = activeFlow.current;
    if (
      !selected ||
      !consentGiven.current ||
      confirmationAttempted.current ||
      !dialogActive.current ||
      operation.current !== null
    )
      return;
    const request = generation.current;
    const isCurrent = () =>
      alive.current && dialogActive.current && request === generation.current;
    operation.current = request;
    confirmationAttempted.current = true;
    setAttempted(true);
    setBusy("confirming");
    setFailed(null);
    invalidateList();
    try {
      const account = await traeAccountsApi.confirm(selected.flowId);
      if (!isCurrent()) return;
      if (
        !account ||
        typeof account.id !== "string" ||
        !account.id.trim() ||
        account.realm !== selected.realm ||
        account.status !== "stored"
      ) {
        activeFlow.current = null;
        consentGiven.current = false;
        setFailed("receiptUnknown");
        return;
      }
      invalidateList();
      setAccounts((rows) =>
        rows.some((row) => row.id === account.id)
          ? rows.map((row) => (row.id === account.id ? account : row))
          : [...rows, account],
      );
      // A completed, accepted flow must not be cancelled on subsequent unmount.
      activeFlow.current = null;
      dialogActive.current = false;
      consentGiven.current = false;
      operation.current = null;
      ++generation.current;
      setFlow(null);
      setConsent(false);
      setBusy(null);
      setDialogOpen(false);
      setSaved(true);
    } catch (error) {
      if (isCurrent()) {
        // A settled IPC failure is not permission to revoke a possibly saved
        // grant. Review is read-only; running/unmounted flows still use cancel.
        activeFlow.current = null;
        consentGiven.current = false;
        setFailed(traeAuthorizationFailure(error));
      }
    } finally {
      if (operation.current === request) {
        operation.current = null;
        if (isCurrent()) setBusy(null);
      }
    }
  };

  const retire = async (id: string) => {
    if (retireActive.current || dialogActive.current) return;
    retireActive.current = true;
    setRetiring(id);
    setFailed(null);
    setSaved(false);
    invalidateList();
    try {
      await traeAccountsApi.retire(id);
      if (alive.current) await loadAccounts();
    } catch {
      if (alive.current) setFailed("unknown");
    } finally {
      retireActive.current = false;
      if (alive.current) setRetiring(null);
    }
  };

  const chooseRealm = (value: TraeRealm) => {
    if (
      !dialogActive.current ||
      operation.current !== null ||
      activeFlow.current
    )
      return;
    if (edition === "cn" ? value !== "cn" : value !== "sg" && value !== "us")
      return;
    setRealm(value);
    setInstallations([]);
    setDiscoveryDone(false);
  };
  const changeConsent = (checked: boolean) => {
    if (
      !dialogActive.current ||
      !activeFlow.current ||
      operation.current !== null ||
      confirmationAttempted.current
    )
      return;
    consentGiven.current = checked;
    setConsent(checked);
  };
  const automaticDisabled = (account: TraeAccountView) =>
    automaticBusy ||
    policyIsUnknown(account) ||
    retiring !== null ||
    dialogOpen ||
    account.generation === undefined ||
    account.autoCheckinRevision === undefined ||
    (!account.canSetAutomatic && !account.autoCheckinEnabled);
  const checkinDisabled = (account: TraeAccountView) =>
    checkinBusy ||
    retiring !== null ||
    dialogOpen ||
    account.status !== "stored" ||
    account.nativeSourceSupported !== true;
  const authHost = traeAuthHost(realm ?? (edition === "cn" ? "cn" : "sg"));
  return {
    accounts,
    loading,
    listFailed,
    failed,
    cancelUncertain,
    saved,
    dialogOpen,
    realm,
    flow,
    installations,
    discoveryDone,
    consent,
    attempted,
    busy,
    retiring,
    checkinBusy,
    checkinResults,
    automaticBusy,
    policyIsUnknown,
    setAutomatic,
    checkin,
    loadAccounts,
    beginDialog,
    closeDialog,
    selectFiles,
    discoverFiles,
    confirm,
    retire,
    chooseRealm,
    changeConsent,
    automaticDisabled,
    checkinDisabled,
    authHost,
  };
}
