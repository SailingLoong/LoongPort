import { useCallback, useEffect, useRef, useState } from "react";

import {
  modelVerificationApi,
  type RunFailureKind,
  type VerificationProgressEvent,
  type VerificationReport,
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
  event: VerificationProgressEvent,
  providerId: string,
  appType: string,
  model: string,
): boolean {
  return (
    event.providerId === providerId &&
    event.appType === appType &&
    event.model === model
  );
}

export function useModelVerificationRun({
  open,
  providerId,
  appType,
}: UseModelVerificationRunOptions) {
  const [runId, setRunId] = useState<string | null>(null);
  const [model, setModel] = useState<string | null>(null);
  const [progress, setProgress] = useState<VerificationProgressEvent | null>(
    null,
  );
  const [failure, setFailure] = useState<RunFailureKind | null>(null);
  const [report, setReport] = useState<VerificationReport | null>(null);
  const [stopping, setStopping] = useState(false);
  const generationRef = useRef(0);
  const openRef = useRef(open);
  openRef.current = open;

  useEffect(() => {
    if (open) return;
    generationRef.current += 1;
    setRunId(null);
    setModel(null);
    setProgress(null);
    setFailure(null);
    setReport(null);
    setStopping(false);
  }, [open]);

  useEffect(() => {
    if (!open || !runId || !model) return;

    let active = true;
    const generation = generationRef.current;
    let stopProgress: (() => void) | undefined;
    let stopChanged: (() => void) | undefined;

    const isCurrent = () => active && generation === generationRef.current;

    void modelVerificationApi
      .onProgress((event) => {
        if (
          !isCurrent() ||
          event.runId !== runId ||
          !targetsMatch(event, providerId, appType, model)
        ) {
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
      .onChanged(async (scope) => {
        if (
          !isCurrent() ||
          scope.providerId !== providerId ||
          scope.appType !== appType
        ) {
          return;
        }

        try {
          const reports = await modelVerificationApi.listResults([providerId]);
          if (!isCurrent()) return;
          setReport(
            reports.find(
              (candidate) =>
                candidate.target.providerId === providerId &&
                candidate.target.appType === appType &&
                candidate.target.model === model,
            ) ?? null,
          );
        } catch {
          // Progress remains usable. A later changed event retries the persisted read.
        }
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
  }, [appType, model, open, providerId, runId]);

  const start = useCallback(
    async (nextModel: string) => {
      const generation = generationRef.current;
      const response = await modelVerificationApi
        .start({
          providerId,
          appType,
          model: nextModel,
        })
        .catch((error: unknown) => {
          const nextFailure = asRunFailure(error);
          setFailure(nextFailure);
          return null;
        });
      if (!response) return;
      if (!openRef.current || generation !== generationRef.current) return;

      generationRef.current += 1;
      setModel(nextModel);
      setRunId(response.runId);
      setProgress(null);
      setFailure(null);
      setReport(null);
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
    report,
    start,
    stop,
    stopping,
  };
}
