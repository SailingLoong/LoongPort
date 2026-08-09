import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { modelVerificationApi } from "@/lib/api/modelVerification";

import { VerificationEvidenceList } from "./VerificationEvidenceList";
import { useModelVerificationRun } from "./useModelVerificationRun";

export interface ModelVerificationDialogProps {
  providerId: string;
  appType: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

type ModelsState = "idle" | "loading" | "ready" | "error";

export function ModelVerificationDialog({
  providerId,
  appType,
  open,
  onOpenChange,
}: ModelVerificationDialogProps) {
  const { t } = useTranslation();
  const [models, setModels] = useState<string[]>([]);
  const [modelsState, setModelsState] = useState<ModelsState>("idle");
  const [selectedModel, setSelectedModel] = useState<string | undefined>();
  const requestRef = useRef(0);
  const run = useModelVerificationRun({ open, providerId, appType });

  const loadModels = useCallback(() => {
    const request = ++requestRef.current;
    setModelsState("loading");
    setModels([]);
    setSelectedModel(undefined);
    void modelVerificationApi
      .listModels(providerId, appType)
      .then((nextModels) => {
        if (request !== requestRef.current) return;
        setModels(nextModels);
        setModelsState("ready");
      })
      .catch(() => {
        if (request !== requestRef.current) return;
        setModelsState("error");
      });
  }, [appType, providerId]);

  useEffect(() => {
    if (!open) {
      requestRef.current += 1;
      setModels([]);
      setModelsState("idle");
      setSelectedModel(undefined);
      return;
    }
    loadModels();
    // Reload from the live backend on every open; the model catalog is not cached UI state.
  }, [loadModels, open]);

  const showStart = !run.isRunning;
  const canStart =
    modelsState === "ready" && selectedModel !== undefined && !run.stopping;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[30rem] gap-0 p-0">
        <DialogHeader>
          <DialogTitle>{t("loongport.modelVerification.title")}</DialogTitle>
          <DialogDescription>
            {t("loongport.modelVerification.description")}
          </DialogDescription>
        </DialogHeader>

        <div className="min-h-40 space-y-4 overflow-y-auto px-6 py-5">
          {modelsState === "loading" && (
            <p className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="size-4 animate-spin" />
              {t("loongport.modelVerification.model.loading")}
            </p>
          )}

          {modelsState === "error" && (
            <div className="space-y-3" role="alert">
              <p className="text-sm text-destructive">
                {t("loongport.modelVerification.model.error")}
              </p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={loadModels}
              >
                {t("loongport.modelVerification.actions.retry")}
              </Button>
            </div>
          )}

          {modelsState === "ready" && models.length === 0 && (
            <p className="text-sm text-muted-foreground">
              {t("loongport.modelVerification.model.empty")}
            </p>
          )}

          {modelsState === "ready" && models.length > 0 && (
            <div className="space-y-2">
              <label
                className="text-sm font-medium"
                htmlFor="model-verification-model"
              >
                {t("loongport.modelVerification.model.label")}
              </label>
              <Select
                value={selectedModel}
                onValueChange={setSelectedModel}
                disabled={run.isRunning}
              >
                <SelectTrigger id="model-verification-model">
                  <SelectValue
                    placeholder={t(
                      "loongport.modelVerification.model.placeholder",
                    )}
                  />
                </SelectTrigger>
                <SelectContent>
                  {models.map((model) => (
                    <SelectItem key={model} value={model}>
                      {model}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}

          {run.isRunning && (
            <div className="space-y-2" aria-live="polite">
              <p className="text-sm text-muted-foreground">
                {t("loongport.modelVerification.status.running", {
                  completed: run.progress?.completedChecks ?? 0,
                  total: run.progress?.totalChecks ?? 0,
                })}
              </p>
              <div
                aria-valuemax={run.progress?.totalChecks ?? 0}
                aria-valuemin={0}
                aria-valuenow={run.progress?.completedChecks ?? 0}
                className="h-2 overflow-hidden rounded-full bg-muted"
                role="progressbar"
              >
                <div
                  className="h-full bg-primary transition-[width]"
                  style={{
                    width:
                      run.progress && run.progress.totalChecks > 0
                        ? `${(run.progress.completedChecks / run.progress.totalChecks) * 100}%`
                        : "0%",
                  }}
                />
              </div>
            </div>
          )}

          {run.failure && (
            <p className="text-sm text-destructive" role="alert">
              {t(`loongport.modelVerification.failure.${run.failure}`)}
            </p>
          )}

          {run.report && (
            <div className="space-y-3 border-t border-border-default pt-4">
              <p className="text-sm font-medium">
                {t(`loongport.modelVerification.verdict.${run.report.verdict}`)}
              </p>
              <VerificationEvidenceList report={run.report} />
            </div>
          )}
        </div>

        <DialogFooter>
          {showStart ? (
            <Button
              type="button"
              disabled={!canStart}
              onClick={() => selectedModel && void run.start(selectedModel)}
            >
              {t(
                run.failure
                  ? "loongport.modelVerification.actions.retry"
                  : "loongport.modelVerification.actions.start",
              )}
            </Button>
          ) : (
            <Button
              type="button"
              variant="outline"
              disabled={run.stopping}
              onClick={() => void run.stop()}
            >
              {run.stopping && <Loader2 className="size-4 animate-spin" />}
              {t("loongport.modelVerification.actions.stop")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
