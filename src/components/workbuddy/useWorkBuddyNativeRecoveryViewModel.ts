import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { usePreservedViewActive } from "@/components/ui/PreservedView";
import {
  workbuddyApi,
  type WorkBuddyNativeRecoveryInspection,
  type WorkBuddyNativeRecoveryResult,
} from "@/lib/api/workbuddy";

type RecoveryState = {
  busy: boolean;
  unresolvedRecovery: boolean;
  capability: boolean | null;
  inspection: WorkBuddyNativeRecoveryInspection | null;
  inspectionId: number;
  afterReceipt: boolean;
  receipt: WorkBuddyNativeRecoveryResult | null;
  error:
    | "checkFailed"
    | "unsupported"
    | "recoveryUnknown"
    | "afterCheckFailed"
    | null;
};
// The native worker can outlive a component or a detached waiter. This store
// retains its receipt and serializes this explicit recovery UI across remounts.
let state: RecoveryState = {
  busy: false,
  unresolvedRecovery: false,
  capability: null,
  inspection: null,
  inspectionId: 0,
  afterReceipt: false,
  receipt: null,
  error: null,
};
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
};
const snapshot = () => state;
function update(patch: Partial<RecoveryState>) {
  state = { ...state, ...patch };
  listeners.forEach((listener) => listener());
}

export function useWorkBuddyNativeRecoveryViewModel() {
  const shared = useSyncExternalStore(subscribe, snapshot, snapshot);
  const viewActive = usePreservedViewActive();
  const active = useRef(false);
  const lifetime = useRef(0);
  const [renderLifetime, setRenderLifetime] = useState(0);
  const [checked, setChecked] = useState<{
    id: number;
    lifetime: number;
  } | null>(null);
  const [confirmation, setConfirmation] = useState<number | null>(null);
  const token = useRef(0);
  useEffect(() => {
    active.current = true;
    setRenderLifetime(lifetime.current);
    return () => {
      active.current = false;
      lifetime.current += 1;
      token.current += 1;
    };
  }, []);
  const latest = useRef({ viewActive, renderLifetime });
  latest.current = { viewActive, renderLifetime };
  const isLive = () =>
    active.current &&
    latest.current.viewActive &&
    renderLifetime === lifetime.current &&
    latest.current.renderLifetime === renderLifetime;
  const check = async () => {
    if (!isLive() || state.busy) return;
    const issuedLifetime = lifetime.current;
    setChecked(null);
    token.current += 1;
    setConfirmation(null);
    update({
      busy: true,
      capability: null,
      inspection: null,
      afterReceipt: false,
      receipt: null,
      error: null,
    });
    try {
      const catalog = await workbuddyApi.listNativeCaptures();
      if (!catalog.activationAvailable) {
        update({ capability: false, error: "unsupported" });
        return;
      }
      const inspection = await workbuddyApi.inspectNativeRecovery();
      const inspectionId = state.inspectionId + 1;
      update({ capability: true, inspection, inspectionId });
      if (
        active.current &&
        lifetime.current === issuedLifetime &&
        latest.current.viewActive
      )
        setChecked({ id: inspectionId, lifetime: issuedLifetime });
    } catch {
      update({ error: "checkFailed" });
    } finally {
      update({ busy: false });
    }
  };
  const eligible = () =>
    isLive() &&
    !state.busy &&
    state.capability === true &&
    checked?.id === state.inspectionId &&
    checked.lifetime === lifetime.current &&
    state.inspection !== null &&
    state.inspection.phase !== "absent";
  const open = () => {
    if (!eligible()) return;
    const next = ++token.current;
    setConfirmation(next);
  };
  const cancel = () => {
    if (
      confirmation !== null &&
      confirmation === token.current &&
      !state.busy
    ) {
      token.current += 1;
      setConfirmation(null);
    }
  };
  const recover = async () => {
    if (confirmation === null || confirmation !== token.current || !eligible())
      return;
    token.current += 1;
    setConfirmation(null);
    setChecked(null);
    update({
      busy: true,
      receipt: null,
      inspection: null,
      error: null,
      afterReceipt: false,
    });
    try {
      // No AbortSignal: leaving the page must not pretend to cancel the worker.
      const receipt = await workbuddyApi.recoverNative();
      update({ receipt });
      try {
        const inspection = await workbuddyApi.inspectNativeRecovery();
        update({
          inspection,
          inspectionId: state.inspectionId + 1,
          afterReceipt: true,
        });
      } catch {
        update({ error: "afterCheckFailed" });
      }
    } catch {
      // Errors can arrive after partial work or a detached IPC; no inference,
      // automatic replay, or clearing a previous switch's unknown outcome.
      update({ error: "recoveryUnknown", unresolvedRecovery: true });
    } finally {
      update({ busy: false });
    }
  };
  return {
    ...shared,
    canRecover: eligible(),
    confirming:
      confirmation !== null && confirmation === token.current && isLive(),
    check,
    open,
    cancel,
    recover,
  };
}
