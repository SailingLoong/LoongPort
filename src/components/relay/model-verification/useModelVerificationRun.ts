import { useCallback, useEffect, useRef, useState } from "react";

import {
  modelVerificationApi,
  type RunFailureKind,
  type VerificationProgressEvent,
  type VerificationTarget,
} from "@/lib/api/modelVerification";

export interface UseModelVerificationRunOptions {
  providerId: string;
  appType: string;
}

const runFailures = new Set<RunFailureKind>([
  "authentication",
  "rateLimited",
  "insufficientBalance",
  "network",
  "upstream",
  "timeout",
  "modelUnavailable",
  "cancelled",
  "invalidResponse",
]);

function asRunFailure(error: unknown): RunFailureKind {
  return typeof error === "string" && runFailures.has(error as RunFailureKind)
    ? (error as RunFailureKind)
    : "upstream";
}

function targetsMatch(
  target: VerificationTarget,
  expected: VerificationTarget,
): boolean {
  return (
    target.providerId === expected.providerId &&
    target.appType === expected.appType &&
    target.model === expected.model
  );
}

export function useModelVerificationRun({
  providerId,
  appType,
}: UseModelVerificationRunOptions) {
  const [runId, setRunId] = useState<string | null>(null);
  const [progress, setProgress] = useState<VerificationProgressEvent | null>(
    null,
  );
  const [failure, setFailure] = useState<RunFailureKind | null>(null);
  const [stopping, setStopping] = useState(false);
  const generationRef = useRef(0);
  const activeRunRef = useRef<{
    runId: string;
    target: VerificationTarget;
  } | null>(null);
  const pendingTargetRef = useRef<VerificationTarget | null>(null);
  const pendingRunIdRef = useRef<string | null>(null);
  const pendingTerminalRef = useRef(false);

  useEffect(() => {
    let active = true;
    const generation = generationRef.current;
    let stopProgress: (() => void) | undefined;

    const isCurrent = () => active && generation === generationRef.current;

    void modelVerificationApi
      .onProgress((event) => {
        if (!isCurrent()) return;

        const activeRun = activeRunRef.current;
        const pendingTarget = pendingTargetRef.current;
        if (activeRun) {
          if (
            event.runId !== activeRun.runId ||
            !targetsMatch(event, activeRun.target)
          ) {
            return;
          }
        } else if (pendingTarget) {
          if (!targetsMatch(event, pendingTarget)) return;
          if (
            pendingRunIdRef.current !== null &&
            event.runId !== pendingRunIdRef.current
          ) {
            return;
          }
          pendingRunIdRef.current = event.runId;
        } else {
          return;
        }

        setProgress(event);
        setStopping(false);
        pendingTerminalRef.current =
          event.state === "completed" ||
          event.state === "cancelled" ||
          event.state === "failed";
        if (event.failure) setFailure(event.failure);
      })
      .then((unlisten) => {
        if (isCurrent()) stopProgress = unlisten;
        else unlisten();
      });

    return () => {
      active = false;
      generationRef.current += 1;
      stopProgress?.();
    };
  }, [appType, providerId]);

  const start = useCallback(
    async (nextModel: string) => {
      const generation = generationRef.current;
      const target = { providerId, appType, model: nextModel };
      activeRunRef.current = null;
      pendingTargetRef.current = target;
      pendingRunIdRef.current = null;
      pendingTerminalRef.current = false;
      setRunId(null);
      setProgress(null);
      setFailure(null);
      setStopping(false);

      let response;
      try {
        response = await modelVerificationApi.start(target);
      } catch (error) {
        if (
          generation === generationRef.current &&
          targetsMatch(pendingTargetRef.current ?? target, target)
        ) {
          pendingTargetRef.current = null;
          pendingRunIdRef.current = null;
          pendingTerminalRef.current = false;
          setFailure(asRunFailure(error));
        }
        return;
      }
      if (generation !== generationRef.current) return;

      if (!targetsMatch(pendingTargetRef.current ?? target, target)) return;

      const acceptedEarlyEvents =
        pendingRunIdRef.current === null ||
        pendingRunIdRef.current === response.runId;
      const terminalBeforeStartResolved = pendingTerminalRef.current;
      activeRunRef.current = { runId: response.runId, target };
      pendingTargetRef.current = null;
      pendingRunIdRef.current = null;
      pendingTerminalRef.current = false;
      setRunId(response.runId);
      if (!acceptedEarlyEvents) setProgress(null);
      if (!acceptedEarlyEvents || !terminalBeforeStartResolved) {
        setFailure(null);
      }
      setStopping(false);
    },
    [appType, providerId],
  );

  const stop = useCallback(async () => {
    if (!runId || stopping) return;
    setStopping(true);
    try {
      await modelVerificationApi.cancel(runId);
    } catch (error) {
      setStopping(false);
      setFailure(asRunFailure(error));
    }
  }, [runId, stopping]);

  const isRunning =
    runId !== null &&
    progress?.state !== "completed" &&
    progress?.state !== "cancelled" &&
    progress?.state !== "failed";

  return {
    failure,
    isRunning,
    progress,
    start,
    stop,
    stopping,
  };
}
