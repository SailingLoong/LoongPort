import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import {
  workbuddyApi,
  WorkBuddyNativeSwitchError,
  type WorkBuddyNativeSwitchReport,
} from "@/lib/api/workbuddy";
import type { AccountRowVM } from "./accountOverviewProjection";

// The backend worker survives a detached UI waiter. Keep uncertain and pending
// targets fenced across row remounts; only a known receipt clears pending.
const pendingTargets = new Set<string>();
const uncertainTargets = new Set<string>();
let recoveryRequired = false;
let sharedRevision = 0;
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
};
const snapshot = () => sharedRevision;
const notifySharedChange = () => {
  sharedRevision += 1;
  listeners.forEach((listener) => listener());
};

export function useWorkBuddyNativeSwitch(
  row: AccountRowVM,
  disabled?: boolean,
) {
  useSyncExternalStore(subscribe, snapshot, snapshot);
  const identity = row.nativeIdentity;
  const key =
    identity.status === "ready" ? JSON.stringify(identity.selection) : "";
  const lifetime = useRef({ key, epoch: 0 });
  if (lifetime.current.key !== key)
    lifetime.current = { key, epoch: lifetime.current.epoch + 1 };
  const current = useRef({ row, key, disabled, epoch: lifetime.current.epoch });
  current.current = { row, key, disabled, epoch: lifetime.current.epoch };
  const confirmationToken = useRef(0);
  const alive = useRef(true);
  const activityEpoch = useRef(0);
  const [renderActivityEpoch, setRenderActivityEpoch] = useState(0);
  useEffect(() => {
    alive.current = true;
    setRenderActivityEpoch(activityEpoch.current);
    return () => {
      alive.current = false;
      activityEpoch.current += 1;
      confirmationToken.current += 1;
    };
  }, []);
  const inFlight = useRef(false);
  const [confirmation, setConfirmation] = useState<{
    key: string;
    epoch: number;
    token: number;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<{
    key: string;
    epoch: number;
    value: WorkBuddyNativeSwitchReport;
  } | null>(null);
  const [error, setError] = useState<{
    key: string;
    epoch: number;
    stage: string;
    reason?: string;
  } | null>(null);
  const canSwitch =
    row.switchAction.enabled &&
    identity.status === "ready" &&
    !disabled &&
    !busy &&
    !recoveryRequired &&
    pendingTargets.size === 0 &&
    !uncertainTargets.has(key);
  const renderEpoch = lifetime.current.epoch;
  const open = () => {
    const state = current.current;
    if (
      canSwitch &&
      alive.current &&
      renderActivityEpoch === activityEpoch.current &&
      renderEpoch === state.epoch &&
      key === state.key &&
      state.row.switchAction.enabled &&
      !state.disabled &&
      !inFlight.current &&
      !recoveryRequired &&
      pendingTargets.size === 0 &&
      !uncertainTargets.has(key)
    ) {
      const token = ++confirmationToken.current;
      setConfirmation({ key, epoch: current.current.epoch, token });
    }
  };
  const cancel = () => {
    if (
      confirmation &&
      confirmation.token === confirmationToken.current &&
      !inFlight.current
    ) {
      confirmationToken.current += 1;
      setConfirmation(null);
    }
  };
  const confirm = async () => {
    const state = current.current;
    if (
      !confirmation ||
      !alive.current ||
      confirmation.token !== confirmationToken.current ||
      confirmation.key !== state.key ||
      confirmation.epoch !== state.epoch ||
      !state.row.switchAction.enabled ||
      state.disabled ||
      state.row.nativeIdentity.status !== "ready" ||
      inFlight.current ||
      recoveryRequired ||
      pendingTargets.size > 0 ||
      uncertainTargets.has(confirmation.key)
    ) {
      if (confirmation && confirmation.token === confirmationToken.current) {
        confirmationToken.current += 1;
        setConfirmation(null);
      }
      return;
    }
    const target = { ...state.row.nativeIdentity.selection };
    const submitted = confirmation.key;
    confirmationToken.current += 1;
    const epoch = state.epoch;
    const submittedActivityEpoch = activityEpoch.current;
    inFlight.current = true;
    pendingTargets.add(submitted);
    notifySharedChange();
    setBusy(true);
    setReport(null);
    setError(null);
    try {
      const value = await workbuddyApi.activateNativeWithRestart(target);
      if (
        alive.current &&
        activityEpoch.current === submittedActivityEpoch &&
        current.current.key === submitted &&
        current.current.epoch === epoch
      )
        setReport({ key: submitted, epoch, value });
    } catch (cause) {
      const stage =
        cause instanceof WorkBuddyNativeSwitchError
          ? cause.stage
          : "outcomeUnknown";
      if (stage === "outcomeUnknown") uncertainTargets.add(submitted);
      if (
        cause instanceof WorkBuddyNativeSwitchError &&
        cause.stage === "transaction" &&
        cause.reason === "recoveryRequired"
      )
        recoveryRequired = true;
      if (
        alive.current &&
        activityEpoch.current === submittedActivityEpoch &&
        current.current.key === submitted &&
        current.current.epoch === epoch
      )
        setError({
          key: submitted,
          epoch,
          stage,
          reason:
            cause instanceof WorkBuddyNativeSwitchError
              ? cause.reason
              : undefined,
        });
    } finally {
      pendingTargets.delete(submitted);
      notifySharedChange();
      inFlight.current = false;
      if (alive.current) {
        setBusy(false);
        setConfirmation(null);
      }
    }
  };
  return {
    canSwitch,
    busy,
    confirming:
      confirmation !== null &&
      (confirmation.token === confirmationToken.current || inFlight.current) &&
      confirmation.key === key &&
      confirmation.epoch === lifetime.current.epoch,
    report:
      report?.key === key && report.epoch === lifetime.current.epoch
        ? report.value
        : null,
    error:
      error?.key === key && error.epoch === lifetime.current.epoch
        ? error
        : recoveryRequired && row.groupId === "workbuddy"
          ? { stage: "transaction", reason: "recoveryRequired" }
          : uncertainTargets.has(key)
            ? { stage: "outcomeUnknown", reason: undefined }
            : null,
    open,
    cancel,
    confirm,
  };
}
