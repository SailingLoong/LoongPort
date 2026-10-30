import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { zcodeClaimApi, type ClaimState } from "@/lib/api/zcodeClaim";
const emptyState: ClaimState = {
  consentRevision: "",
  enabled: false,
  participants: [],
  records: {},
  busy: false,
};

export function useZCodeClaimControls({
  dataRoot,
  profiles,
  disabled = false,
}: {
  dataRoot?: string;
  profiles: { id: string; label: string | null }[];
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const copy = (
    key: string,
    fallback: string,
    values?: Record<string, string>,
  ) => t(`zcode.claim.${key}`, { defaultValue: fallback, ...values });
  const [state, setState] = useState<ClaimState>(emptyState);
  const [draftRevision, setDraftRevision] = useState("");
  const resetDraft = useRef(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [auto, setAuto] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  const epoch = useRef(0);
  const operationInFlight = useRef(false);
  const operationVersion = useRef(0);
  const backendBusy = useRef(false);
  const refresh = useRef<(() => void) | null>(null);
  useEffect(() => {
    const generation = ++epoch.current;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let initialized = false;
    let reading = false;
    setState(emptyState);
    setSelected([]);
    setDraftRevision("");
    resetDraft.current = false;
    setAuto(false);
    setLoaded(false);
    setPending(false);
    setError(false);
    backendBusy.current = false;
    operationInFlight.current = false;
    const poll = async () => {
      if (disposed || reading || document.hidden) return;
      reading = true;
      const readVersion = operationVersion.current;
      try {
        const next = await zcodeClaimApi.state(dataRoot);
        if (
          !disposed &&
          generation === epoch.current &&
          readVersion === operationVersion.current
        ) {
          setState(next);
          backendBusy.current = next.busy;
          setLoaded(true);
          setError(false);
          if (!initialized || resetDraft.current) {
            setDraftRevision(next.consentRevision);
            resetDraft.current = false;
            setSelected(next.participants);
            setAuto(next.enabled);
            initialized = true;
          }
        }
      } catch {
        if (
          !disposed &&
          generation === epoch.current &&
          readVersion === operationVersion.current
        ) {
          setError(true);
        }
      } finally {
        reading = false;
        if (!disposed && !document.hidden)
          timer = setTimeout(poll, backendBusy.current ? 2000 : 10000);
      }
    };
    const visibility = () => {
      clearTimeout(timer);
      if (!document.hidden) void poll();
    };
    refresh.current = visibility;
    document.addEventListener("visibilitychange", visibility);
    void poll();
    return () => {
      disposed = true;
      ++epoch.current;
      clearTimeout(timer);
      refresh.current = null;
      document.removeEventListener("visibilitychange", visibility);
    };
  }, [dataRoot]);
  const ids = selected.filter((id) =>
    profiles.some((profile) => profile.id === id),
  );
  const claimable = ids.filter((id) => state.records[id]?.canClaim === true);
  const blocked = disabled || pending || state.busy || !loaded;
  async function perform(
    operation: () => Promise<ClaimState>,
    consent = false,
  ) {
    if (operationInFlight.current) return;
    const generation = epoch.current;
    operationInFlight.current = true;
    ++operationVersion.current;
    setPending(true);
    setError(false);
    try {
      const next = await operation();
      if (generation === epoch.current) {
        // Retire reads that started while the command was still pending.
        ++operationVersion.current;
        setState(next);
        if (consent) {
          setDraftRevision(next.consentRevision);
          setSelected(next.participants);
          setAuto(next.enabled);
        }
        setError(false);
        backendBusy.current = next.busy;
        refresh.current?.();
      }
    } catch {
      if (generation === epoch.current) {
        ++operationVersion.current;
        if (consent) {
          // A failed/uncertain write is never replayed with a newer revision.
          setDraftRevision("");
          resetDraft.current = true;
          refresh.current?.();
        }
        setError(true);
      }
    } finally {
      if (generation === epoch.current) {
        operationInFlight.current = false;
        setPending(false);
      }
    }
  }
  const date = (seconds: number | null) => {
    if (seconds === null) return copy("unknown", "Unknown");
    const value = new Date(seconds * 1000);
    return Number.isFinite(value.getTime())
      ? value.toLocaleString()
      : copy("unknown", "Unknown");
  };
  const selectAccount = (id: string, checked: boolean) => {
    if (blocked || !profiles.some((profile) => profile.id === id)) return;
    setSelected((previous) =>
      checked
        ? [...previous.filter((value) => value !== id), id]
        : previous.filter((value) => value !== id),
    );
  };
  const checkSelected = async () => {
    if (blocked || !ids.length) return;
    await perform(() => zcodeClaimApi.start(dataRoot, ids, true));
  };
  const claimSelected = async () => {
    if (blocked || !claimable.length) return;
    await perform(() => zcodeClaimApi.start(dataRoot, claimable, false));
  };
  const checkAccount = async (id: string) => {
    if (blocked || !profiles.some((profile) => profile.id === id)) return;
    await perform(() => zcodeClaimApi.start(dataRoot, [id], true));
  };
  const claimAccount = async (id: string) => {
    if (
      blocked ||
      state.records[id]?.canClaim !== true ||
      !profiles.some((profile) => profile.id === id)
    )
      return;
    await perform(() => zcodeClaimApi.start(dataRoot, [id], false));
  };
  const cancelClaims = async () => {
    if (disabled || pending || !state.busy) return;
    await perform(() => zcodeClaimApi.cancel(dataRoot));
  };
  const applyAutomatic = async () => {
    if (blocked || !draftRevision || (auto && !ids.length)) return;
    await perform(
      () => zcodeClaimApi.setAuto(dataRoot, auto, ids, draftRevision),
      true,
    );
  };
  return {
    checkSelected,
    claimSelected,
    checkAccount,
    claimAccount,
    cancelClaims,
    applyAutomatic,
    copy,
    state,
    draftRevision,
    selectAccount,
    auto,
    setAuto,
    loaded,
    pending,
    error,
    ids,
    claimable,
    blocked,
    date,
  };
}
