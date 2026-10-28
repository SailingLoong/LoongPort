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
export function useRelaySetup(relayId: number) {
  const scope = useRef<{
    relayId: number;
    recoveryId: string;
    pending: boolean;
    mounted: boolean;
    version: number;
    operationId: string | null;
    receipt: RelaySetupReceipt | null;
  } | null>(null);
  if (!scope.current || scope.current.relayId !== relayId) {
    scope.current = {
      relayId,
      recoveryId: crypto.randomUUID(),
      pending: false,
      mounted: true,
      version: 0,
      operationId: null,
      receipt: null,
    };
  }
  const owner = scope.current;
  const isActive = () => scope.current === owner && owner.mounted;
  const discovery = useQuery({
    queryKey: ["relay-discovery", relayId],
    queryFn: () => relayApi.discoverTiers(relayId),
    retry: false,
  });
  const recovery = useQuery({
    // A cached account receipt is evidence, not proof that this mounted owner
    // has completed recovery. Each owner must perform its own native read.
    queryKey: ["relay-setup-recovery", relayId, owner.recoveryId],
    // The native account lookup also works after storage/UUID loss. A cached
    // client UUID must not hide a newer unresolved operation on this account.
    queryFn: async () => {
      const version = owner.version;
      return { receipt: await relayApi.getSetupOperation(relayId), version };
    },
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnMount: "always",
    staleTime: 0,
    gcTime: 0,
  });
  const [draft, setDraft] = useState<Partial<Record<AppId, string>>>({});
  const [receipt, setReceipt] = useState<RelaySetupReceipt | null>(null);
  const [phase, setPhase] = useState<Phase>("selection");
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
  const [unresolved, setUnresolved] = useState(false);
  useEffect(() => {
    owner.mounted = true;
    return () => {
      owner.mounted = false;
    };
  }, [owner]);

  useEffect(() => {
    setDraft({});
    setConnectionReview(null);
    setReceipt(null);
    setReviewed(null);
    setPhase("selection");
    setUnresolved(false);
    setError(null);
    setBusy(false);
  }, [relayId]);
  useEffect(() => {
    if (recovery.data?.receipt && recovery.data.version === owner.version) {
      accept(recovery.data.receipt, true);
    }
  }, [recovery.data, owner]);

  function accept(next: RelaySetupReceipt, fromRecovery = false) {
    if (!isActive() || next.relayId !== relayId) return;
    const prior = owner.receipt;
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
  function remember(operationId: string) {
    owner.operationId = operationId;
    // A convenience pointer only; native account recovery is always available.
    try {
      localStorage.setItem(relaySetupOperationKey(relayId), operationId);
    } catch {
      /* unavailable client storage cannot erase native journal facts */
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
        if (rejection) {
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
      !isActive() ||
      owner.pending ||
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
    owner.pending = true;
    owner.version += 1;
    setBusy(true);
    setError(null);
    const operationId = crypto.randomUUID();
    remember(operationId);
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
      if (!isActive()) return;
      setError("preparationOutcomeUncertain");
      setUnresolved(true);
      setPhase("result");
      await observeOriginal(
        operationId,
        isRelayPreparationRejection(cause, operationId) ? cause : undefined,
      );
    } finally {
      if (isActive()) {
        owner.pending = false;
        setBusy(false);
      }
    }
  }
  async function reconcile() {
    const operationId = receipt?.operationId ?? owner.operationId;
    if (!isActive() || owner.pending || !operationId) return;
    owner.pending = true;
    owner.version += 1;
    setBusy(true);
    setError(null);
    try {
      accept(await relayApi.reconcileSetupOperation(operationId));
    } catch {
      if (!isActive()) return;
      setError("reconciliationFailed");
      await observeOriginal(operationId);
    } finally {
      if (isActive()) {
        owner.pending = false;
        setBusy(false);
      }
    }
  }
  async function resumeFailed(apps: AppId[]) {
    if (
      !isActive() ||
      owner.pending ||
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
      if (!isActive()) return;
      setError("preparationOutcomeUncertain");
      setUnresolved(true);
      await observeOriginal(receipt.operationId);
    } finally {
      if (isActive()) {
        owner.pending = false;
        setBusy(false);
      }
    }
  }
  async function reviewConnection(
    app: AppId,
    catalogDecision?: ProviderCatalogDecision,
  ) {
    const current = owner.receipt;
    const target = current?.targets.find((target) => target.app === app);
    if (
      !isActive() ||
      owner.pending ||
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
      !isActive() ||
      owner.pending ||
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
      }
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
