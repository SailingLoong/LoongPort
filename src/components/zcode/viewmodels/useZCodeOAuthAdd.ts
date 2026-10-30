import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";

import { safeAccountError, type AccountError } from "@/lib/api/zcodeAccounts";
import {
  zcodeLoginApi,
  type LoginFamily,
  type LoginProgress,
} from "@/lib/api/zcodeLogin";
import { settingsApi } from "@/lib/api/settings";
import { zcodeDeviceApi } from "@/lib/api/zcodeDevices";
type Write = "begin" | "key" | "decline" | "save";
export function useZCodeOAuthAdd({
  open,
  onClose,
  onSaved,
  onImportLocal,
  libraryDataRoot,
  savedAccount,
  boundDeviceMids = [],
}: {
  open: boolean;
  onClose: () => void;
  onSaved: () => Promise<void>;
  onImportLocal?: () => void;
  libraryDataRoot?: string;
  savedAccount?: { id: string; catalogRevision: string };
  boundDeviceMids?: string[];
}) {
  const { t } = useTranslation();
  const copy = (
    key: string,
    defaultValue: string,
    values?: Record<string, string | number>,
  ) => t(`zcode.accounts.login.${key}`, { defaultValue, ...values });
  const [family, setFamily] = useState<LoginFamily>("bigmodel");
  const [devices, setDevices] = useState<string[]>([]);
  const [effectiveDeviceMid, setEffectiveDeviceMid] = useState<string | null>(
    null,
  );
  const [deviceMid, setDeviceMid] = useState("current");
  const [deviceLoading, setDeviceLoading] = useState(open && !savedAccount);
  const [deviceLoadFailed, setDeviceLoadFailed] = useState(false);
  const [deviceReload, setDeviceReload] = useState(0);
  const [progress, setProgress] = useState<LoginProgress | null>(null);
  const [busy, setBusy] = useState<Write | null>(null);
  const [reading, setReading] = useState(false);
  const [paused, setPaused] = useState(false);
  const [error, setError] = useState<AccountError | null>(null);
  const [uncertain, setUncertain] = useState<Write | null>(null);
  const [updateDuplicate, setUpdateDuplicate] = useState(false);
  const [listRefreshFailed, setListRefreshFailed] = useState(false);
  const [previousReadFailed, setPreviousReadFailed] = useState(false);
  const flow = useRef<string | null>(null);
  const generation = useRef(0);
  const readGeneration = useRef(0);
  const alive = useRef(false);
  const inFlight = useRef<Write | null>(null);
  const readInFlight = useRef(false);
  const notified = useRef(new Set<string>());
  const savedCallback = useRef(onSaved);
  savedCallback.current = onSaved;
  const savedId = savedAccount?.id;
  const savedRevision = savedAccount?.catalogRevision;
  const abandon = useCallback(() => {
    generation.current += 1;
    readGeneration.current += 1;
    inFlight.current = null;
    readInFlight.current = false;
    const previous = flow.current;
    flow.current = null;
    if (previous) void zcodeLoginApi.cancel(previous).catch(() => {});
  }, []);
  const reset = useCallback(() => {
    setProgress(null);
    setBusy(null);
    setReading(false);
    setPaused(false);
    setError(null);
    setUncertain(null);
    setUpdateDuplicate(false);
    setListRefreshFailed(false);
    setPreviousReadFailed(false);
  }, []);
  const current = useCallback(
    (version: number, flowId?: string) =>
      alive.current &&
      generation.current === version &&
      (!flowId || flow.current === flowId),
    [],
  );
  const accept = useCallback(
    (next: LoginProgress, version: number) => {
      if (!current(version, next.flowId)) return;
      setProgress(next);
      setError(next.error ? safeAccountError(next.error) : null);
      setUncertain((previous) =>
        next.phase === "saved" ||
        next.phase === "cancelled" ||
        next.phase === "expired" ||
        next.phase === "failed" ||
        (previous === "save" && next.error?.remedy === "retrySave") ||
        ((previous === "key" || previous === "decline") &&
          next.error?.remedy === "retryKeyConsent") ||
        ((previous === "key" || previous === "decline") &&
          next.phase === "review")
          ? null
          : previous,
      );
      if (
        next.phase === "saved" &&
        next.saved &&
        !notified.current.has(next.flowId)
      ) {
        // Record before awaiting the list refresh. A saved receipt is never a Save retry.
        notified.current.add(next.flowId);
        void (async () => {
          try {
            await savedCallback.current();
          } catch {
            if (current(version, next.flowId)) setListRefreshFailed(true);
          }
        })();
      }
    },
    [current],
  );
  const restore = useCallback(async () => {
    if (!alive.current || readInFlight.current || inFlight.current) return;
    const version = generation.current;
    const readVersion = ++readGeneration.current;
    readInFlight.current = true;
    setReading(true);
    setError(null);
    setPreviousReadFailed(false);
    try {
      let next = await zcodeLoginApi.lastProgress(libraryDataRoot);
      if (!current(version) || readGeneration.current !== readVersion) return;
      if (
        savedId &&
        savedRevision &&
        !(
          next?.purpose === "completeCoding" &&
          next.account?.id === savedId &&
          next.sourceCatalogRevision === savedRevision &&
          !["failed", "expired", "cancelled"].includes(next.phase)
        )
      ) {
        next = await zcodeLoginApi.beginSavedCoding(
          savedId,
          savedRevision,
          libraryDataRoot,
        );
        if (!current(version) || readGeneration.current !== readVersion) {
          void zcodeLoginApi.cancel(next.flowId).catch(() => {});
          return;
        }
      }
      if (next) {
        flow.current = next.flowId;
        accept(next, version);
      }
    } catch (cause) {
      if (current(version) && readGeneration.current === readVersion) {
        setError(safeAccountError(cause));
        setPreviousReadFailed(true);
      }
    } finally {
      if (current(version) && readGeneration.current === readVersion) {
        readInFlight.current = false;
        setReading(false);
      }
    }
  }, [libraryDataRoot, savedId, savedRevision, current, accept]);
  useLayoutEffect(() => {
    alive.current = open;
    if (open) {
      reset();
      setFamily("bigmodel");
      setDeviceLoading(!savedId);
      void restore();
    }
    return () => {
      alive.current = false;
      abandon();
    };
  }, [open, abandon, reset, restore, savedId]);
  useEffect(() => {
    if (!open || savedId) return;
    let active = true;
    setDevices([]);
    setEffectiveDeviceMid(null);
    setDeviceMid("current");
    setDeviceLoading(true);
    setDeviceLoadFailed(false);
    void zcodeDeviceApi.state(libraryDataRoot).then(
      (state) => {
        if (active) {
          setDevices(state.devices);
          setEffectiveDeviceMid(state.effectiveDeviceMid);
          setDeviceLoading(false);
        }
      },
      () => {
        if (active) {
          setDeviceLoadFailed(true);
          setDeviceLoading(false);
        }
      },
    );
    return () => {
      active = false;
    };
  }, [open, libraryDataRoot, savedId, deviceReload]);
  const read = useCallback(async () => {
    const flowId = flow.current;
    if (!alive.current || !flowId || inFlight.current || readInFlight.current)
      return;
    const version = generation.current;
    const readVersion = ++readGeneration.current;
    readInFlight.current = true;
    setReading(true);
    try {
      const next = await zcodeLoginApi.progress(flowId);
      if (
        current(version, flowId) &&
        readGeneration.current === readVersion &&
        next.flowId === flowId
      ) {
        accept(next, version);
      }
    } catch (cause) {
      if (current(version, flowId) && readGeneration.current === readVersion) {
        setError(safeAccountError(cause));
        setPaused(true);
      }
    } finally {
      if (current(version, flowId) && readGeneration.current === readVersion) {
        readInFlight.current = false;
        setReading(false);
      }
    }
  }, [accept, current]);
  useEffect(() => {
    if (
      !open ||
      paused ||
      reading ||
      busy ||
      !progress ||
      (progress.phase !== "waiting" && progress.phase !== "preparing")
    )
      return;
    const seconds = progress.authorization?.pollIntervalSec;
    // This polls a read-only snapshot; the backend owns official HTTP polling.
    const timer = setTimeout(
      () => void read(),
      Math.max(1, seconds ?? 2) * 1000,
    );
    return () => clearTimeout(timer);
  }, [open, progress, reading, busy, paused, read]);
  const openExternal = async (url: string) => {
    const version = generation.current;
    try {
      await settingsApi.openExternal(url);
    } catch (cause) {
      if (current(version)) setError(safeAccountError(cause));
    }
  };
  const begin = async () => {
    if (deviceLoading || deviceLoadFailed || deviceTaken || savedAccount)
      return;
    if (
      !alive.current ||
      inFlight.current ||
      readInFlight.current ||
      flow.current
    )
      return;
    const version = generation.current;
    inFlight.current = "begin";
    setBusy("begin");
    setError(null);
    try {
      const next = await (deviceMid !== "current"
        ? zcodeLoginApi.begin(family, libraryDataRoot, deviceMid)
        : libraryDataRoot === undefined
          ? zcodeLoginApi.begin(family)
          : zcodeLoginApi.begin(family, libraryDataRoot));
      if (!current(version)) {
        void zcodeLoginApi.cancel(next.flowId).catch(() => {});
        return;
      }
      flow.current = next.flowId;
      accept(next, version);
      if (next.authorization && next.phase === "waiting")
        await openExternal(next.authorization.url);
    } catch (cause) {
      if (current(version)) setError(safeAccountError(cause));
    } finally {
      if (current(version)) {
        inFlight.current = null;
        setBusy(null);
      }
    }
  };
  const write = async (action: Exclude<Write, "begin">) => {
    const flowId = flow.current;
    if (
      !alive.current ||
      !flowId ||
      !progress ||
      inFlight.current ||
      readInFlight.current
    )
      return;
    if (
      (action === "key" || action === "decline") &&
      (progress.phase !== "keyRequired" || keyUncertain)
    )
      return;
    if (action === "key" && (!progress.account || !progress.project)) return;
    if (action === "save" && (progress.phase !== "review" || saveUncertain))
      return;
    const version = generation.current;
    inFlight.current = action;
    setBusy(action);
    setError(null);
    try {
      const next =
        action === "key"
          ? await zcodeLoginApi.confirmKey(
              flowId,
              progress.project!.organizationId,
              progress.project!.projectId,
            )
          : action === "decline"
            ? await zcodeLoginApi.declineKey(flowId)
            : await zcodeLoginApi.save(
                flowId,
                progress.purpose === "completeCoding" || updateDuplicate,
              );
      if (next.flowId === flowId) accept(next, version);
    } catch (cause) {
      if (current(version, flowId)) {
        setError(safeAccountError(cause));
        setUncertain(action);
      }
    } finally {
      if (current(version, flowId)) {
        inFlight.current = null;
        setBusy(null);
      }
    }
  };
  const close = () => {
    alive.current = false;
    abandon();
    onClose();
  };
  const startAnother = () => {
    abandon();
    reset();
    setDeviceMid("current");
  };
  const cancelRead = () => {
    readGeneration.current += 1;
    readInFlight.current = false;
    setReading(false);
    setPaused(true);
  };
  const phase = progress?.phase;
  const selectedDeviceMid =
    deviceMid === "current" ? effectiveDeviceMid : deviceMid;
  const isBoundDevice = (id: string) =>
    boundDeviceMids.some((bound) => bound.toLowerCase() === id.toLowerCase());
  const deviceTaken =
    selectedDeviceMid !== null && isBoundDevice(selectedDeviceMid);
  const currentDeviceTaken =
    effectiveDeviceMid !== null && isBoundDevice(effectiveDeviceMid);
  const completeCoding = progress?.purpose === "completeCoding";
  const blocked = busy !== null || reading;
  const keyUncertain =
    progress?.error?.remedy !== "retryKeyConsent" &&
    (uncertain === "key" ||
      uncertain === "decline" ||
      !!progress?.keyMayExist ||
      !!progress?.error);
  const saveUncertain =
    progress?.error?.remedy !== "retrySave" &&
    (uncertain === "save" || !!progress?.error);
  const title = !phase
    ? savedAccount
      ? copy("completeSavedTitle", "Complete the saved Coding connection")
      : copy("title", "Add a sign-in account")
    : phase === "waiting"
      ? copy("waiting", "Waiting for official authorization")
      : phase === "preparing"
        ? completeCoding
          ? copy("preparingSaved", "Preparing the saved account connection")
          : copy("preparing", "Identity verified; preparing connections")
        : phase === "keyRequired"
          ? copy("keyTitle", "Complete the Coding Plan connection")
          : phase === "review"
            ? completeCoding
              ? copy(
                  "reviewSavedCoding",
                  "Review the Coding connection before saving",
                )
              : copy("review", "Confirm this account before saving")
            : phase === "saved"
              ? copy("saved", "Account save result")
              : phase === "expired"
                ? copy("expired", "Official authorization expired")
                : phase === "cancelled"
                  ? copy("cancelled", "Official sign-in cancelled")
                  : copy("failed", "Official sign-in could not complete");
  const capability = (value: "ready" | "unknown" | "unavailable") =>
    value === "ready"
      ? copy("ready", "Ready")
      : value === "unknown"
        ? copy("unknown", "Unknown")
        : copy("unavailable", "Unavailable");
  const failureKey = error?.code.slice("zcode.account.".length);
  const reloadDevices = () => setDeviceReload((v) => v + 1);
  const importLocal = () => {
    close();
    onImportLocal?.();
  };
  const queryOriginal = () => {
    setPaused(false);
    return read();
  };
  const restart = () => {
    startAnother();
    if (savedAccount) void restore();
  };
  return {
    reloadDevices,
    importLocal,
    queryOriginal,
    restart,
    t,
    copy,
    family,
    setFamily,
    devices,
    deviceMid,
    setDeviceMid,
    deviceLoading,
    deviceLoadFailed,
    progress,
    reading,
    error,
    uncertain,
    updateDuplicate,
    setUpdateDuplicate,
    listRefreshFailed,
    previousReadFailed,
    restore,
    openExternal,
    begin,
    write,
    close,
    startAnother,
    cancelRead,
    phase,
    isBoundDevice,
    deviceTaken,
    currentDeviceTaken,
    completeCoding,
    blocked,
    keyUncertain,
    saveUncertain,
    title,
    capability,
    failureKey,
  };
}
