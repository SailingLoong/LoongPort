import { useCallback, useEffect, useRef, useState } from "react";

import {
  modelVerificationApi,
  type RunFailureKind,
  type VerificationProgressEvent,
  type VerificationReport,
  type VerificationTarget,
} from "@/lib/api/modelVerification";

export interface UseModelVerificationRunOptions {
  open: boolean;
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
  open,
  providerId,
  appType,
}: UseModelVerificationRunOptions) {
  const [runId, setRunId] = useState<string | null>(null);
  const [progress, setProgress] = useState<VerificationProgressEvent | null>(
    null,
  );
  const [failure, setFailure] = useState<RunFailureKind | null>(null);
  const [report, setReport] = useState<VerificationReport | null>(null);
  const [stopping, setStopping] = useState(false);
  const generationRef = useRef(0);
  const openRef = useRef(open);
  const activeRunRef = useRef<{
    runId: string;
    target: VerificationTarget;
  } | null>(null);
  const pendingTargetRef = useRef<VerificationTarget | null>(null);
  const pendingRunIdRef = useRef<string | null>(null);
  const pendingChangedRef = useRef(false);
  openRef.current = open;

  const loadReport = useCallback(
    async (target: VerificationTarget, generation: number) => {
      try {
        const reports = await modelVerificationApi.listResults([providerId]);
        const activeRun = activeRunRef.current;
        if (
          !openRef.current ||
          generation !== generationRef.current ||
          !activeRun ||
          !targetsMatch(activeRun.target, target)
        ) {
          return;
        }
        setReport(
          reports.find((candidate) => targetsMatch(candidate.target, target)) ??
            null,
        );
      } catch {
        // Progress remains usable. A later changed event retries the persisted read.
      }
    },
    [providerId],
  );

  useEffect(() => {
    if (open) return;
    generationRef.current += 1;
    activeRunRef.current = null;
    pendingTargetRef.current = null;
    pendingRunIdRef.current = null;
    pendingChangedRef.current = false;
    setRunId(null);
    setProgress(null);
    setFailure(null);
    setReport(null);
    setStopping(false);
  }, [open]);

  useEffect(() => {
    if (!open) return;

    let active = true;
    const generation = generationRef.current;
    let stopProgress: (() => void) | undefined;
    let stopChanged: (() => void) | undefined;

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
        if (event.failure) setFailure(event.failure);
      })
      .then((unlisten) => {
        if (isCurrent()) stopProgress = unlisten;
        else unlisten();
      });

    void modelVerificationApi
      .onChanged((scope) => {
        if (
          !isCurrent() ||
          scope.providerId !== providerId ||
          scope.appType !== appType
        ) {
          return;
        }

        const activeRun = activeRunRef.current;
        if (activeRun) {
          void loadReport(activeRun.target, generation);
          return;
        }

        if (pendingTargetRef.current) pendingChangedRef.current = true;
      })
      .then((unlisten) => {
        if (isCurrent()) stopChanged = unlisten;
        else unlisten();
      });

    return () => {
      active = false;
      stopProgress?.();
      stopChanged?.();
    };
  }, [appType, loadReport, open, providerId]);

  const start = useCallback(
    async (nextModel: string) => {
      const generation = generationRef.current;
      const target = { providerId, appType, model: nextModel };
      activeRunRef.current = null;
      pendingTargetRef.current = target;
      pendingRunIdRef.current = null;
      pendingChangedRef.current = false;
      setRunId(null);
      setProgress(null);
      setFailure(null);
      setReport(null);
      setStopping(false);

      let response;
      try {
        response = await modelVerificationApi.start(target);
      } catch (error) {
        if (
          openRef.current &&
          generation === generationRef.current &&
          targetsMatch(pendingTargetRef.current ?? target, target)
        ) {
          pendingTargetRef.current = null;
          pendingRunIdRef.current = null;
          pendingChangedRef.current = false;
          setFailure(asRunFailure(error));
        }
        return;
      }
      if (!openRef.current || generation !== generationRef.current) return;

      if (!targetsMatch(pendingTargetRef.current ?? target, target)) return;

      const acceptedEarlyEvents =
        pendingRunIdRef.current === null ||
        pendingRunIdRef.current === response.runId;
      const changedBeforeStartResolved = pendingChangedRef.current;
      activeRunRef.current = { runId: response.runId, target };
      pendingTargetRef.current = null;
      pendingRunIdRef.current = null;
      pendingChangedRef.current = false;
      setRunId(response.runId);
      if (!acceptedEarlyEvents) setProgress(null);
      setFailure(null);
      setReport(null);
      setStopping(false);
      if (acceptedEarlyEvents && changedBeforeStartResolved) {
        void loadReport(target, generation);
      }
    },
    [appType, loadReport, providerId],
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
    report,
    start,
    stop,
    stopping,
  };
}
