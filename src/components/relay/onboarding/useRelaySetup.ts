import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  relayApi,
  type RelaySetupReceipt,
  type RelayTierSelection,
} from "@/lib/api/relay";
import type { RelayConnectionReview } from "@/lib/api/relaySetup";
import type { ProviderCatalogDecision } from "@/lib/api/providers";
import type { AppId } from "@/lib/api/types";
import {
  isRelayPreparationRejection,
  type RelayPreparationRejection,
} from "@/lib/api/relaySetup";

export const relaySetupOperationKey = (relayId: number) =>
  `relay-setup-operation:${relayId}`;

type Phase = "selection" | "review" | "result";
type ReviewedSelection = {
  discoveryRevision: string;
  selections: RelayTierSelection[];
};
const needsReconciliation = (receipt: RelaySetupReceipt | null) =>
  receipt !== null &&
  (receipt.status !== "settled" ||
    receipt.groups.some((group) =>
      ["unknown", "requesting"].includes(group.state),
    ) ||
    receipt.targets.some(
      (target) =>
        target.configuration === "unknown" ||
        ["unknown", "applying"].includes(target.connection.state),
    ));

/** The receipt is authoritative; the editable draft never mutates its facts.
 * Discovery/recovery are read-only. No effect runs preparation on mount. */
export function useRelaySetup(
  relayId: number,
  finishPending?: { readonly current: boolean },
) {
  const scope = useRef<{
    relayId: number;
    recoveryId: string;
    pending: boolean;
    mounted: boolean;
    version: number;
    operationId: string | null;
    receipt: RelaySetupReceipt | null;
    needsReplay: boolean;
    deferredError: string | null;
  } | null>(null);
  if (!scope.current || scope.current.relayId !== relayId) {
    let remembered: string | null = null;
    try {
      remembered = localStorage.getItem(relaySetupOperationKey(relayId));
    } catch {
      // Recovery and dispatch below both fail closed if storage is unavailable.
    }
    scope.current = {
      relayId,
      recoveryId: crypto.randomUUID(),
      pending: false,
      mounted: true,
      version: 0,
      operationId: remembered,
      receipt: null,
      needsReplay: false,
      deferredError: null,
    };
  }
  const owner = scope.current;
  const isActive = () => scope.current === owner && owner.mounted;
  // Check synchronously before either setup or finish dispatches. React's
  // rendered busy flag may not have committed when another handler runs.
  const canStartOperation = () =>
    isActive() && !owner.pending && !finishPending?.current;
  const discovery = useQuery({
    queryKey: ["relay-discovery", relayId],
    queryFn: () => relayApi.discoverTiers(relayId),
    retry: false,
  });
  const recovery = useQuery({
    // A cached account receipt is evidence, not proof that this mounted owner
    // has completed recovery. Each owner must perform its own native read.
    queryKey: ["relay-setup-recovery", relayId, owner.recoveryId],
    // Read both the account journal and any previously dispatched UUID. An
    // account-only null can race a command that has not journaled yet.
    queryFn: async () => {
      const version = owner.version;
      const pointer = localStorage.getItem(relaySetupOperationKey(relayId));
      const [accountReceipt, pointerReceipt] = await Promise.all([
        relayApi.getSetupOperation(relayId),
        pointer ? relayApi.getSetupOperation(relayId, pointer) : null,
      ]);
      return { accountReceipt, pointerReceipt, pointer, version };
    },
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnMount: "always",
    staleTime: 0,
    gcTime: 0,
  });
  const [draft, setDraft] = useState<Partial<Record<AppId, string>>>({});
  const [receipt, setReceipt] = useState<RelaySetupReceipt | null>(null);
  const [phase, setPhase] = useState<Phase>(
    owner.operationId !== null ? "result" : "selection",
  );
  const [reviewed, setReviewed] = useState<ReviewedSelection | null>(null);
  const [connectionReview, setConnectionReview] = useState<{
    app: AppId;
    operationId: string;
    requestId: string;
    receiptRevision: number;
    confirmation: RelayConnectionReview["confirmation"];
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [unresolved, setUnresolved] = useState(owner.operationId !== null);
  useEffect(() => {
    owner.mounted = true;
    setBusy(owner.pending);
    if (owner.needsReplay && owner.receipt) {
      owner.needsReplay = false;
      accept(owner.receipt);
    }
    if (owner.deferredError) {
      setError(owner.deferredError);
      setUnresolved(true);
      setPhase("result");
      owner.deferredError = null;
    }
    return () => {
      owner.mounted = false;
    };
  }, [owner]);

  const stateOwner = useRef(owner);
  useEffect(() => {
    // Activity reconnects effects on reveal; only a different account owner
    // resets the in-memory draft and review.
    if (stateOwner.current === owner) return;
    stateOwner.current = owner;
    setDraft({});
    setConnectionReview(null);
    setReceipt(null);
    setReviewed(null);
    setPhase(owner.operationId !== null ? "result" : "selection");
    setUnresolved(owner.operationId !== null);
    setError(null);
    setBusy(false);
  }, [owner]);
  useEffect(() => {
    const data = recovery.data;
    if (!data || data.version !== owner.version || !isActive()) return;
    let currentPointer: string | null;
    try {
      currentPointer = localStorage.getItem(relaySetupOperationKey(relayId));
    } catch {
      setUnresolved(true);
      setError("recoveryFailed");
      setPhase("result");
      return;
    }
    if (currentPointer !== data.pointer) {
      owner.operationId = currentPointer ?? owner.operationId;
      setUnresolved(true);
      setError("recoveryConflict");
      setPhase("result");
      return;
    }
    if (data.pointer !== null) {
      owner.operationId = data.pointer;
      if (
        !data.pointerReceipt ||
        data.pointerReceipt.operationId !== data.pointer ||
        (data.accountReceipt &&
          data.accountReceipt.operationId !== data.pointer)
      ) {
        setUnresolved(true);
        setError(
          data.accountReceipt ? "recoveryConflict" : "recoveryUncertain",
        );
        setPhase("result");
        return;
      }
      const next =
        data.accountReceipt &&
        data.accountReceipt.receiptRevision >
          data.pointerReceipt.receiptRevision
          ? data.accountReceipt
          : data.pointerReceipt;
      accept(next, true);
    } else if (data.accountReceipt) accept(data.accountReceipt, true);
  }, [recovery.data, owner]);

  function accept(next: RelaySetupReceipt, fromRecovery = false) {
    if (scope.current !== owner || next.relayId !== relayId) return;
    const prior = owner.receipt;
    if (!owner.mounted) {
      if (owner.operationId && owner.operationId !== next.operationId) {
        owner.deferredError = "recoveryConflict";
        return;
      }
      if (
        prior?.operationId === next.operationId &&
        prior.receiptRevision > next.receiptRevision
      )
        return;
      // Preserve the actual receipt on its original owner. React state updates
      // are deferred until this same view is active again.
      owner.receipt = next;
      owner.operationId = next.operationId;
      owner.needsReplay = true;
      owner.deferredError = null;
      return;
    }
    // Reconnecting the same settled receipt is not a new result and must not
    // displace a subsequent draft/review. Unresolved work still reconciles.
    if (
      fromRecovery &&
      prior?.operationId === next.operationId &&
      prior.receiptRevision === next.receiptRevision &&
      !unresolved &&
      !needsReconciliation(prior) &&
      !needsReconciliation(next)
    )
      return;
    if (
      prior?.operationId === next.operationId &&
      prior.receiptRevision > next.receiptRevision
    )
      return;
    if (
      fromRecovery &&
      owner.operationId &&
      owner.operationId !== next.operationId
    ) {
      setError("recoveryConflict");
      setUnresolved(true);
      return;
    }
    setConnectionReview((review) =>
      review &&
      (review.operationId !== next.operationId ||
        review.receiptRevision !== next.receiptRevision ||
        next.targets.find((target) => target.app === review.app)?.connection
          .requestId !== review.requestId)
        ? null
        : review,
    );
    owner.receipt = next;
    setReceipt(next);
    owner.operationId = next.operationId;
    setUnresolved(false);
    setPhase("result");
  }
  // Only remove the operation we own; another window may have advanced it.
  function forget(operationId: string): boolean {
    try {
      const key = relaySetupOperationKey(relayId);
      const current = localStorage.getItem(key);
      if (current !== null && current !== operationId) return false;
      if (current !== null) localStorage.removeItem(key);
      return localStorage.getItem(key) === null;
    } catch {
      return false;
    }
  }
  function remember(operationId: string): boolean {
    try {
      const key = relaySetupOperationKey(relayId);
      const current = localStorage.getItem(key);
      if (current !== null) {
        owner.operationId = current;
        setUnresolved(true);
        setError("recoveryConflict");
        setPhase("result");
        return false;
      }
      localStorage.setItem(key, operationId);
      const stored = localStorage.getItem(key);
      if (stored !== operationId) {
        owner.operationId = stored;
        setUnresolved(true);
        setError("recoveryConflict");
        setPhase("result");
        return false;
      }
      owner.operationId = operationId;
      return true;
    } catch {
      setError("operationStorageUnavailable");
      return false;
    }
  }
  async function observeOriginal(
    operationId: string,
    rejection?: RelayPreparationRejection,
  ) {
    try {
      const found = await relayApi.getSetupOperation(relayId, operationId);
      if (found) accept(found);
      else if (isActive()) {
        if (rejection && forget(operationId)) {
          // The pointer change invalidates any already-started recovery read.
          owner.version += 1;
          // This invocation was rejected before operation creation. Preserve any
          // prior result; only the newly rejected draft can be reviewed again.
          owner.operationId = owner.receipt?.operationId ?? null;
          setUnresolved(false);
          setReviewed(null);
          setPhase("selection");
          setError(rejection.reasonCode);
        } else setUnresolved(true); // null alone is not retry authority.
      }
    } catch {
      if (isActive()) setUnresolved(true);
    }
  }
  function reviewSelection() {
    if (
      !isActive() ||
      owner.pending ||
      !recovery.isSuccess ||
      unresolved ||
      needsReconciliation(receipt)
    )
      return;
    const current = discovery.data;
    if (!current) return;
    const selections = Object.entries(draft)
      .filter((entry): entry is [AppId, string] => Boolean(entry[1]))
      .map(([app, groupId]) => ({ app, groupId }));
    if (!selections.length) return;
    if (
      selections.some(
        (selection) =>
          !current.choices.some(
            (choice) =>
              choice.app === selection.app &&
              choice.groupId === selection.groupId,
          ),
      )
    ) {
      setError("selectionChanged");
      return;
    }
    setReviewed({ discoveryRevision: current.revision, selections });
    setError(null);
    setPhase("review");
  }
  async function confirmSelection() {
    if (
      !canStartOperation() ||
      phase !== "review" ||
      !reviewed ||
      !recovery.isSuccess ||
      unresolved ||
      needsReconciliation(receipt)
    )
      return;
    if (reviewed.discoveryRevision !== discovery.data?.revision) {
      setError("discoveryChanged");
      return;
    }
    const operationId = crypto.randomUUID();
    if (!remember(operationId)) return;
    owner.pending = true;
    owner.version += 1;
    setBusy(true);
    setError(null);
    try {
      accept(
        await relayApi.prepareTiers({
          kind: "start",
          operationId,
          relayId,
          ...reviewed,
        }),
      );
    } catch (cause) {
      if (!isActive()) {
        if (scope.current === owner)
          owner.deferredError = "preparationOutcomeUncertain";
        return;
      }
      setError("preparationOutcomeUncertain");
      setUnresolved(true);
      setPhase("result");
      await observeOriginal(
        operationId,
        isRelayPreparationRejection(cause, operationId) ? cause : undefined,
      );
    } finally {
      owner.pending = false;
      if (isActive()) setBusy(false);
    }
  }
  async function reconcile() {
    const operationId = owner.operationId ?? receipt?.operationId;
    if (!canStartOperation() || !operationId) return;
    try {
      const pointer = localStorage.getItem(relaySetupOperationKey(relayId));
      if (pointer !== null && pointer !== operationId) {
        setUnresolved(true);
        setError("recoveryConflict");
        return;
      }
    } catch {
      setUnresolved(true);
      setError("recoveryFailed");
      return;
    }
    owner.pending = true;
    owner.version += 1;
    setBusy(true);
    setError(null);
    try {
      accept(await relayApi.reconcileSetupOperation(operationId));
    } catch {
      if (!isActive()) {
        if (scope.current === owner)
          owner.deferredError = "reconciliationFailed";
        return;
      }
      setError("reconciliationFailed");
      await observeOriginal(operationId);
    } finally {
      owner.pending = false;
      if (isActive()) setBusy(false);
    }
  }
  async function resumeFailed(apps: AppId[]) {
    if (
      !canStartOperation() ||
      !receipt ||
      unresolved ||
      needsReconciliation(receipt) ||
      !apps.length
    )
      return;
    if (
      new Set(apps).size !== apps.length ||
      apps.some(
        (app) =>
          !receipt.targets.some(
            (target) => target.app === app && target.configuration === "failed",
          ),
      )
    )
      return;
    owner.pending = true;
    owner.version += 1;
    setBusy(true);
    setError(null);
    try {
      accept(
        await relayApi.prepareTiers({
          kind: "resume",
          operationId: receipt.operationId,
          expectedReceiptRevision: receipt.receiptRevision,
          apps,
        }),
      );
    } catch {
      if (!isActive()) {
        if (scope.current === owner)
          owner.deferredError = "preparationOutcomeUncertain";
        return;
      }
      setError("preparationOutcomeUncertain");
      setUnresolved(true);
      await observeOriginal(receipt.operationId);
    } finally {
      owner.pending = false;
      if (isActive()) setBusy(false);
    }
  }
  async function reviewConnection(
    app: AppId,
    catalogDecision?: ProviderCatalogDecision,
  ) {
    const current = owner.receipt;
    const target = current?.targets.find((target) => target.app === app);
    if (
      !canStartOperation() ||
      !recovery.isSuccess ||
      !current ||
      !target ||
      !["prepared", "existing"].includes(target.configuration) ||
      ["applying", "unknown", "applied"].includes(target.connection.state)
    )
      return;
    owner.pending = true;
    owner.version += 1;
    setBusy(true);
    setError(null);
    setConnectionReview(null);
    try {
      const review = await relayApi.prepareApplicationConnection(
        current.operationId,
        app,
        current.receiptRevision,
        catalogDecision,
      );
      accept(review.receipt);
      const requestId = review.receipt.targets.find(
        (target) => target.app === app,
      )?.connection.requestId;
      if (
        isActive() &&
        requestId &&
        owner.receipt?.operationId === review.receipt.operationId &&
        owner.receipt.receiptRevision === review.receipt.receiptRevision
      )
        setConnectionReview({
          app,
          confirmation: review.confirmation,
          operationId: review.receipt.operationId,
          receiptRevision: review.receipt.receiptRevision,
          requestId,
        });
    } catch {
      if (isActive()) setError("connectionReviewFailed");
      else if (scope.current === owner)
        owner.deferredError = "connectionReviewFailed";
      await observeOriginal(current.operationId);
    } finally {
      owner.pending = false;
      if (isActive()) setBusy(false);
    }
  }
  async function confirmConnection(quitChatgpt: boolean) {
    const current = owner.receipt;
    const app = connectionReview?.app;
    if (
      !canStartOperation() ||
      !current ||
      !app ||
      !connectionReview ||
      current.operationId !== connectionReview.operationId ||
      current.receiptRevision !== connectionReview.receiptRevision ||
      current.targets.find((target) => target.app === app)?.connection
        .requestId !== connectionReview.requestId ||
      current.targets.find((target) => target.app === app)?.connection.state !==
        "confirmationRequired"
    )
      return;
    owner.pending = true;
    owner.version += 1;
    setBusy(true);
    setError(null);
    try {
      const outcome = await relayApi.applyPreparedTier(
        current.operationId,
        app,
        current.receiptRevision,
        quitChatgpt,
      );
      accept(outcome.receipt);
      const requestId = outcome.receipt.targets.find(
        (target) => target.app === app,
      )?.connection.requestId;
      if (isActive())
        setConnectionReview(
          outcome.result?.status === "confirmationRequired" && requestId
            ? {
                app,
                confirmation: outcome.result,
                operationId: outcome.receipt.operationId,
                receiptRevision: outcome.receipt.receiptRevision,
                requestId,
              }
            : null,
        );
    } catch {
      if (isActive()) {
        setError("applicationOutcomeUnknown");
        setConnectionReview(null);
      } else if (scope.current === owner)
        owner.deferredError = "applicationOutcomeUnknown";
      await observeOriginal(current.operationId);
    } finally {
      owner.pending = false;
      if (isActive()) setBusy(false);
    }
  }
  function cancelConnection() {
    if (isActive() && !owner.pending) setConnectionReview(null);
  }
  function newDraft() {
    if (
      !isActive() ||
      owner.pending ||
      unresolved ||
      needsReconciliation(receipt)
    )
      return;
    if (receipt && !forget(receipt.operationId)) {
      setUnresolved(true);
      setError("recoveryConflict");
      setPhase("result");
      return;
    }
    if (receipt) owner.version += 1;
    owner.operationId = null;
    setConnectionReview(null);
    setDraft({});
    setReviewed(null);
    setError(null);
    setPhase("selection");
  }
  function cancelDraft() {
    if (!isActive() || owner.pending) return;
    setReviewed(null);
    setPhase(receipt ? "result" : "selection");
  }
  return {
    discovery,
    recovery,
    canStartOperation,
    draft,
    setDraft,
    receipt,
    phase,
    reviewed,
    busy: busy || recovery.isPending,
    error: error ?? (recovery.isError ? "recoveryFailed" : null),
    needsReconciliation: unresolved || needsReconciliation(receipt),
    reviewSelection,
    confirmSelection,
    reconcile,
    newDraft,
    cancelDraft,
    resumeFailed,
    connectionReview,
    reviewConnection,
    confirmConnection,
    cancelConnection,
  };
}
