import { useCallback, useEffect, useRef, useState } from "react";
import {
  automaticClaimApi,
  type AutomaticClaimProvider,
  type AutomaticClaimSnapshot,
} from "@/lib/api/automaticClaim";
import { AUTOMATIC_CLAIM_STATUS_UPDATED } from "@/lib/api/events";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Reads local projections only. Opening a page never starts scheduler work. */
export function useAutomaticClaimStatus(
  provider: AutomaticClaimProvider,
  onUpdated: () => void | Promise<void>,
) {
  const [snapshot, setSnapshot] = useState<AutomaticClaimSnapshot | null>(null);
  const [failed, setFailed] = useState(false);
  const alive = useRef(true);
  const sequence = useRef(0);
  const updated = useRef(onUpdated);
  updated.current = onUpdated;
  const [subscriptionFailed, setSubscriptionFailed] = useState(false);
  const [subscriptionAttempt, setSubscriptionAttempt] = useState(0);
  const reload = useCallback(async () => {
    if (!alive.current) return;
    const request = ++sequence.current;
    try {
      const value = await automaticClaimApi.status(provider);
      if (!alive.current || sequence.current !== request) return;
      if (
        value?.provider !== provider ||
        !["waiting", "checking"].includes(value.phase)
      )
        throw new Error("Invalid scheduler snapshot");
      setSnapshot(value);
      setFailed(false);
    } catch {
      if (!alive.current || sequence.current !== request) return;
      setSnapshot(null);
      setFailed(true);
    }
  }, [provider]);
  useEffect(() => {
    alive.current = true;
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    setSnapshot(null);
    setFailed(false);
    setSubscriptionFailed(false);
    void reload();
    // useTauriEvent has no ready callback. Keep this thin lifecycle local: the
    // second read closes the gap between initial snapshots and native subscription.
    void (async () => {
      try {
        const off = await listen<AutomaticClaimSnapshot>(
          AUTOMATIC_CLAIM_STATUS_UPDATED,
          (event) => {
            if (disposed || event.payload?.provider !== provider) return;
            void reload();
            void updated.current();
          },
        );
        if (disposed) {
          off();
          return;
        }
        unlisten = off;
        void reload();
        void updated.current();
      } catch {
        if (!disposed) setSubscriptionFailed(true);
      }
    })();
    return () => {
      disposed = true;
      alive.current = false;
      sequence.current += 1;
      unlisten?.();
    };
  }, [provider, reload, subscriptionAttempt]);
  return {
    snapshot:
      !subscriptionFailed && snapshot?.provider === provider ? snapshot : null,
    failed: failed || subscriptionFailed,
    reload: () =>
      subscriptionFailed
        ? setSubscriptionAttempt((value) => value + 1)
        : reload(),
  };
}
