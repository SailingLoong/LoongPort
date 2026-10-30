import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";
import { serviceOnboardingKey } from "@/lib/query/serviceOnboarding";
import { PROVIDER_STORE_APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
import type { ProviderCatalogDecision } from "@/lib/api/providers";
import type { ConnectedService } from "./useServiceOnboarding";
import { useRelaySetup } from "./useRelaySetup";

/** Owns this page's transient intent and composes the single setup transaction owner. */
export function useRelaySetupConfigurationViewModel(
  account: ConnectedService,
  sourceAppId: AppId,
  onDone: () => void,
) {
  const finishing = useRef(false);
  const setup = useRelaySetup(account.rowId, finishing);
  const client = useQueryClient();
  const [leaving, setLeaving] = useState(false);
  const [commandRevision, setCommandRevision] = useState(0);
  const [finishError, setFinishError] = useState(false);
  const [retryApps, setRetryApps] = useState<AppId[]>([]);
  const [acknowledged, setAcknowledgedState] = useState(false);
  const acknowledgedRef = useRef(false);
  const updateAcknowledged = (value: boolean) => {
    acknowledgedRef.current = value;
    setAcknowledgedState(value);
  };
  const [connectionAcknowledged, setConnectionAcknowledgedState] =
    useState(false);
  const connectionAcknowledgedRef = useRef(false);
  const updateConnectionAcknowledged = (value: boolean) => {
    connectionAcknowledgedRef.current = value;
    setConnectionAcknowledgedState(value);
  };
  const [connectionDialog, setConnectionDialogState] = useState(false);
  const connectionDialogRef = useRef(false);
  const setConnectionDialog = (value: boolean) => {
    connectionDialogRef.current = value;
    setConnectionDialogState(value);
  };
  const mounted = useRef(true);
  const lifecycle = useRef(0);
  useEffect(() => {
    mounted.current = true;
    // Activity and StrictMode can reconnect effects without a new render.
    // Rebind command closures to the newly active lifecycle.
    setCommandRevision((value) => value + 1);
    setLeaving(finishing.current);
    return () => {
      mounted.current = false;
      lifecycle.current += 1;
    };
  }, []);
  const busy = setup.busy || leaving;
  const commandOwner = useRef<object | null>(null);
  // Stable through StrictMode's double render, replaced on meaningful surface
  // or acknowledgement changes and on Activity activation.
  const renderedCommand = useMemo(
    () => ({}),
    [
      commandRevision,
      sourceAppId,
      account.rowId,
      busy,
      setup.phase,
      setup.receipt,
      setup.draft,
      setup.discovery.data,
      setup.discovery.isSuccess,
      setup.discovery.isError,
      setup.recovery.isSuccess,
      setup.recovery.isError,
      setup.needsReconciliation,
      setup.connectionReview,
      retryApps,
      acknowledged,
      connectionAcknowledged,
      connectionDialog,
    ],
  );
  useLayoutEffect(() => {
    commandOwner.current = renderedCommand;
    return () => {
      if (commandOwner.current === renderedCommand) commandOwner.current = null;
    };
  }, [renderedCommand]);
  const surfaceRevision = useRef(0);
  useLayoutEffect(() => {
    ++surfaceRevision.current;
    return () => {
      ++surfaceRevision.current;
    };
  }, [
    setup.phase,
    setup.receipt?.operationId,
    setup.receipt?.receiptRevision,
    retryApps,
  ]);
  const renderedLifecycle = lifecycle.current;
  const isCurrentCommand = () =>
    mounted.current &&
    lifecycle.current === renderedLifecycle &&
    commandOwner.current === renderedCommand;
  useLayoutEffect(() => {
    acknowledgedRef.current = false;
    setAcknowledgedState(false);
    return () => {
      acknowledgedRef.current = false;
    };
  }, [
    setup.phase,
    setup.receipt?.operationId,
    setup.receipt?.receiptRevision,
    retryApps,
  ]);
  useLayoutEffect(() => {
    connectionAcknowledgedRef.current = false;
    connectionDialogRef.current = false;
    setConnectionAcknowledgedState(false);
    setConnectionDialogState(false);
    return () => {
      connectionAcknowledgedRef.current = false;
      connectionDialogRef.current = false;
    };
  }, [
    setup.connectionReview?.app,
    setup.connectionReview?.operationId,
    setup.connectionReview?.receiptRevision,
    setup.connectionReview?.requestId,
  ]);
  const finish = async () => {
    if (
      !mounted.current ||
      lifecycle.current !== renderedLifecycle ||
      commandOwner.current !== renderedCommand ||
      !setup.canStartOperation() ||
      busy ||
      finishing.current
    )
      return;
    const activeLifecycle = renderedLifecycle;
    const activeSurfaceRevision = surfaceRevision.current;
    const isActive = () =>
      mounted.current &&
      lifecycle.current === activeLifecycle &&
      surfaceRevision.current === activeSurfaceRevision;
    finishing.current = true;
    setLeaving(true);
    setFinishError(false);
    try {
      // Explicit exit is not telemetry consent; it does not change sharing.
      await serviceOnboardingApi.dismiss();
      await client.invalidateQueries({ queryKey: serviceOnboardingKey });
      if (isActive()) onDone();
    } catch {
      if (isActive()) setFinishError(true);
    } finally {
      finishing.current = false;
      if (mounted.current) setLeaving(false);
    }
  };
  const choices = setup.discovery.data?.choices ?? [];
  const apps = [
    ...new Set([
      sourceAppId,
      ...PROVIDER_STORE_APP_IDS.filter((app) =>
        choices.some((choice) => choice.app === app),
      ),
    ]),
  ];
  const reviewing = setup.phase === "review" || retryApps.length > 0;
  const selectionVisible =
    setup.phase === "selection" && !reviewing && setup.discovery.isSuccess;
  const resultVisible = setup.phase === "result" && !reviewing;
  const reviewEnabled =
    !busy &&
    setup.recovery.isSuccess &&
    Object.values(setup.draft).some(Boolean) &&
    !setup.needsReconciliation;
  const canAct = () => isCurrentCommand() && !busy && setup.canStartOperation();
  const setAcknowledged = (value: boolean) => {
    if (canAct() && reviewing) updateAcknowledged(value);
  };
  const setConnectionAcknowledged = (value: boolean) => {
    if (canAct() && resultVisible && setup.connectionReview)
      updateConnectionAcknowledged(value);
  };
  const selected = retryApps.length
    ? (setup.receipt?.selections.filter((selection) =>
        retryApps.includes(selection.app),
      ) ?? [])
    : (setup.reviewed?.selections ?? []);
  const appRows = apps.map((app) => {
    const options = choices.filter((choice) => choice.app === app);
    const selectedChoice = options.find(
      (choice) => choice.groupId === setup.draft[app],
    );
    const hint = !options.length
      ? {
          key: "appEmpty",
          fallback: "当前应用暂无可选分组，可暂不接入并继续选择其他应用。",
        }
      : selectedChoice?.localConfiguration === "present"
        ? {
            key: "existingHint",
            fallback:
              "已有本地配置，将保持原样复用；不代表已校验密钥或在线可用。",
          }
        : {
            key: "coldHint",
            fallback:
              "远端密钥状态未知。确认后才核对可认领的托管密钥，必要时按需创建。",
          };
    return {
      app,
      options,
      value: setup.draft[app] ?? "",
      hint,
      disabled: busy || !options.length,
    };
  });
  const reviewRows = selected.map((selection) => ({
    app: selection.app,
    groupName:
      choices.find(
        (choice) =>
          choice.app === selection.app && choice.groupId === selection.groupId,
      )?.groupName ??
      setup.receipt?.groups.find((group) => group.groupId === selection.groupId)
        ?.groupName ??
      selection.groupId,
  }));
  const resultRows = (setup.receipt?.targets ?? []).map((target) => {
    const group = setup.receipt?.groups.find(
      (item) => item.groupId === target.groupId,
    );
    const localReady =
      target.configuration === "prepared" ||
      target.configuration === "existing";
    const result =
      target.connection.state === "applied"
        ? { key: "applied", fallback: "已记录本次接入成功；当前状态可核对" }
        : target.connection.state === "failedBeforeWrite"
          ? {
              key: "connectionNotApplied",
              fallback: "本次未执行接入，请重新检查接入影响",
            }
          : target.connection.state === "changed"
            ? {
                key: "connectionChanged",
                fallback: "应用配置已变化，请重新检查接入影响",
              }
            : ["unknown", "applying"].includes(target.connection.state)
              ? target.connection.observedCurrent
                ? {
                    key: "currentlySelected",
                    fallback:
                      "上次核对时配置指向此项，但原接入操作结果仍待核对",
                  }
                : {
                    key: "connectionUnknown",
                    fallback: "接入结果待核对，不会自动重复应用",
                  }
              : localReady
                ? {
                    key: "prepared",
                    fallback: "本地配置已准备；本次尚未执行应用接入",
                  }
                : group?.state === "ready" && target.configuration === "failed"
                  ? { key: "keyOnly", fallback: "密钥已准备，本地配置尚未完成" }
                  : target.configuration === "failed"
                    ? { key: "failed", fallback: "本次未完成配置准备" }
                    : {
                        key: "uncertain",
                        fallback: "结果待核对，不会自动重复创建",
                      };
    const origin =
      group?.origin === "created"
        ? { key: "keyCreated", fallback: "已记录本次创建的密钥事实" }
        : group?.origin === "reused"
          ? { key: "keyReused", fallback: "复用符合托管条件的已有密钥" }
          : group?.origin === "observed"
            ? {
                key: "keyObserved",
                fallback: "核对时发现托管密钥，未归因为本次创建",
              }
            : { key: "keyUnknown", fallback: "本次未确认远端密钥状态" };
    return {
      app: target.app,
      groupName: group?.groupName ?? target.groupId,
      result,
      origin,
      canConnect:
        localReady &&
        !["applied", "applying", "unknown"].includes(target.connection.state) &&
        target.compatibility !== "unsupported",
      unsupported: target.compatibility === "unsupported",
    };
  });
  const refresh = async (includeRecovery: boolean) => {
    if (!isCurrentCommand() || (!includeRecovery && !selectionVisible)) return;
    ++surfaceRevision.current;
    // Retire callbacks from the pre-read commit even if the read resolves
    // before React commits its new query state.
    commandOwner.current = null;
    setCommandRevision((value) => value + 1);
    try {
      await Promise.all([
        setup.discovery.refetch(),
        ...(includeRecovery ? [setup.recovery.refetch()] : []),
      ]);
    } catch {
      // The query owns its error state; it does not navigate the page.
    }
  };
  const refreshAll = () => {
    void refresh(true);
  };
  const refreshDiscovery = () => {
    void refresh(false);
  };
  const selectApp = (app: AppId, groupId: string) => {
    if (
      !canAct() ||
      !selectionVisible ||
      (groupId !== "" &&
        !choices.some(
          (choice) => choice.app === app && choice.groupId === groupId,
        ))
    )
      return;
    ++surfaceRevision.current;
    setup.setDraft({ ...setup.draft, [app]: groupId });
  };
  const reviewSelection = () => {
    if (!canAct() || !selectionVisible || !reviewEnabled) return;
    ++surfaceRevision.current;
    updateAcknowledged(false);
    setup.reviewSelection();
  };
  const cancelReview = () => {
    if (!canAct() || !reviewing) return;
    ++surfaceRevision.current;
    commandOwner.current = null;
    updateAcknowledged(false);
    setRetryApps([]);
    setup.cancelDraft();
  };
  const confirmPreparation = async () => {
    if (!canAct() || !acknowledgedRef.current || !reviewing) return;
    commandOwner.current = null;
    updateAcknowledged(false);
    if (retryApps.length) {
      await setup.resumeFailed(retryApps);
      setRetryApps([]);
    } else await setup.confirmSelection();
  };
  const startRetry = () => {
    // A same-turn retry intent may supersede an already-dispatched finish, but
    // it never dispatches setup work until the shared admission is free.
    if (
      !isCurrentCommand() ||
      setup.busy ||
      (!setup.canStartOperation() && !finishing.current) ||
      !resultVisible ||
      setup.needsReconciliation ||
      !setup.receipt
    )
      return;
    const failed = setup.receipt.targets.filter(
      (target) => target.configuration === "failed",
    );
    if (!failed.length) return;
    ++surfaceRevision.current;
    setRetryApps(failed.map((target) => target.app));
    updateAcknowledged(false);
  };
  const newDraft = () => {
    if (!canAct() || !resultVisible || setup.needsReconciliation) return;
    ++surfaceRevision.current;
    updateAcknowledged(false);
    setup.newDraft();
  };
  const openConnectionReview = (app: AppId) => {
    if (
      !canAct() ||
      !resultVisible ||
      !setup.recovery.isSuccess ||
      !resultRows.some((row) => row.app === app && row.canConnect)
    )
      return;
    updateConnectionAcknowledged(false);
    setConnectionDialog(false);
    void setup.reviewConnection(app);
  };
  const cancelConnection = () => {
    if (!canAct() || !resultVisible || !setup.connectionReview) return;
    commandOwner.current = null;
    updateConnectionAcknowledged(false);
    setup.cancelConnection();
    setConnectionDialog(false);
  };
  const beginConnection = () => {
    if (
      !canAct() ||
      !connectionAcknowledgedRef.current ||
      !setup.connectionReview
    )
      return;
    if (setup.connectionReview.confirmation) setConnectionDialog(true);
    else {
      commandOwner.current = null;
      updateConnectionAcknowledged(false);
      void setup.confirmConnection(false);
    }
  };
  const previewCatalog = (decision: ProviderCatalogDecision) => {
    if (
      canAct() &&
      connectionDialogRef.current &&
      connectionAcknowledgedRef.current &&
      setup.connectionReview
    )
      void setup.reviewConnection(setup.connectionReview.app, decision);
  };
  const switchConnection = (
    quit: boolean,
    decision?: ProviderCatalogDecision,
  ) => {
    const app = setup.connectionReview?.app;
    if (
      !canAct() ||
      !app ||
      !connectionDialogRef.current ||
      !connectionAcknowledgedRef.current
    )
      return;
    const reviewed = setup.receipt?.targets.find((target) => target.app === app)
      ?.connection.intent?.change.catalogDecision;
    if (decision && JSON.stringify(decision) !== JSON.stringify(reviewed)) {
      void setup.reviewConnection(app, decision);
      return;
    }
    commandOwner.current = null;
    setConnectionDialog(false);
    updateConnectionAcknowledged(false);
    void setup.confirmConnection(quit);
  };
  return {
    busy,
    finishError,
    accountLabel: setup.discovery.data?.account.accountLabel ?? account.name,
    siteOrigin: setup.discovery.data?.account.siteOrigin,
    loading: setup.discovery.isPending || setup.recovery.isPending,
    readError: setup.discovery.isError || setup.recovery.isError,
    error: setup.recovery.isError ? null : setup.error,
    selectionVisible,
    noChoices: !choices.length,
    appRows,
    reviewRows,
    reviewEnabled,
    reviewing,
    acknowledged,
    setAcknowledged,
    prepareDisabled: !acknowledged || busy,
    resultVisible,
    operationId: setup.receipt?.operationId,
    resultRows,
    connectionReviewApp: setup.connectionReview?.app,
    dialogTargetName: connectionDialog
      ? setup.connectionReview?.confirmation?.targetName
      : undefined,
    dialogTargetApp: connectionDialog ? setup.connectionReview?.app : undefined,
    dialogCatalog: setup.connectionReview?.confirmation?.catalog,
    dialogQuitRequired:
      setup.connectionReview?.confirmation?.quitRequired ?? false,
    connectionAcknowledged,
    setConnectionAcknowledged,
    connectionDisabled: busy || !connectionAcknowledged,
    connectionDialog,
    needsReconciliation: setup.needsReconciliation,
    newDraftDisabled: busy || setup.needsReconciliation,
    retryVisible:
      !setup.needsReconciliation &&
      !!setup.receipt?.targets.some(
        (target) => target.configuration === "failed",
      ),
    refreshAll,
    refreshDiscovery,
    selectApp,
    reviewSelection,
    cancelReview,
    confirmPreparation,
    startRetry,
    newDraft,
    openConnectionReview,
    cancelConnection,
    beginConnection,
    previewCatalog,
    switchConnection,
    cancelDialog: () => {
      if (!isCurrentCommand()) return;
      commandOwner.current = null;
      setConnectionDialog(false);
    },
    reconcile: () => {
      if (canAct() && resultVisible) void setup.reconcile();
    },
    finish,
  };
}
