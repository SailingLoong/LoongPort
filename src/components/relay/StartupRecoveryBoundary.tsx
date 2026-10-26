import { useId, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { LiteralKeyInput, type LiteralKeyInputHandle } from "./LiteralKeyInput";
import { CredentialKeyRecoveryHelp } from "./CredentialKeyRecoveryHelp";
import {
  startupRecoveryApi,
  startupRecoveryKey,
} from "@/lib/api/startupRecovery";

/** A fault-only inline consumer of the original recovery owner, never a shell gate. */
export function StartupRecoveryBoundary({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const headingId = useId();
  const client = useQueryClient();
  const recovery = useQuery({
    queryKey: startupRecoveryKey,
    queryFn: startupRecoveryApi.status,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const operationInFlight = useRef(false);
  const [busy, setBusy] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [restartFromFailure, setRestartFromFailure] = useState(false);
  const [issue, setIssue] = useState<string | null>(null);
  const [inputRevision, setInputRevision] = useState(0);
  const view = recovery.isError ? null : recovery.data;
  const restartRequired = view?.restartRequired === true || restartFromFailure;
  const completed =
    view?.status === "completed" && view.restartRequired === true;
  const canRecover =
    view?.status === "pending" &&
    view.canRecover === true &&
    !restartRequired &&
    !recovery.isFetching;

  const recheck = async () => {
    if (operationInFlight.current) return;
    setInputRevision((value) => value + 1);
    await recovery.refetch();
  };
  const recover = async (sourceText: string) => {
    if (
      operationInFlight.current ||
      !canRecover ||
      !view ||
      sourceText.length === 0
    )
      return;
    operationInFlight.current = true;
    setBusy(true);
    setAttempted(true);
    setIssue(null);
    try {
      // Deliberately no useMutation: the source secret is transient invoke input.
      const result = await startupRecoveryApi.recover(view.token, sourceText);
      client.setQueryData(startupRecoveryKey, result);
      if (result.status !== "completed" || !result.restartRequired) {
        setIssue("startupRecovery.notCompleted");
      }
    } catch (error) {
      const code =
        typeof error === "object" && error !== null && "code" in error
          ? error.code
          : null;
      setIssue(
        code === "secret.password_rejected" || code === "secret.key_rejected"
          ? "startupRecovery.keyRejected"
          : code === "upgrade.source_changed"
            ? "startupRecovery.sourceChanged"
            : "startupRecovery.unknownResult",
      );
      if (
        typeof error === "object" &&
        error !== null &&
        "restartRequired" in error &&
        error.restartRequired === true
      ) {
        setRestartFromFailure(true);
      }
      // A lost reply may have completed. Inspect once; never resend the operation.
      await recovery.refetch();
    } finally {
      setBusy(false);
      operationInFlight.current = false;
    }
  };
  const restart = async () => {
    if (operationInFlight.current || !restartRequired) return;
    operationInFlight.current = true;
    setBusy(true);
    try {
      await startupRecoveryApi.restart();
    } catch {
      setIssue("startupRecovery.restartFailed");
    } finally {
      setBusy(false);
      operationInFlight.current = false;
    }
  };

  if (recovery.isSuccess && recovery.data === null && !attempted)
    return children;
  return (
    <section
      className="space-y-4 rounded-lg border p-5"
      aria-labelledby={headingId}
    >
      <h3 id={headingId} className="font-medium">
        {t(view ? "startupRecovery.title" : "startupRecovery.checkTitle")}
      </h3>
      <p className="text-sm text-muted-foreground">
        {t(
          completed
            ? "startupRecovery.completed"
            : view?.status === "verification_required"
              ? "startupRecovery.verificationRequired"
              : view?.status === "pending"
                ? "startupRecovery.description"
                : recovery.isPending
                  ? "common.loading"
                  : attempted
                    ? "startupRecovery.unknownResult"
                    : "startupRecovery.statusFailed",
        )}
      </p>
      {canRecover && (
        <RecoveryInput
          key={`${view.token}:${inputRevision}`}
          busy={busy}
          recover={recover}
        />
      )}
      {issue &&
        (!completed || issue === "startupRecovery.restartFailed") &&
        !(
          attempted &&
          !view &&
          !recovery.isPending &&
          issue === "startupRecovery.unknownResult"
        ) && (
          <p role="alert" className="text-sm text-destructive">
            {t(issue)}
          </p>
        )}
      {restartRequired && (
        <div className="space-y-3">
          <p className="text-sm text-muted-foreground">
            {t("startupRecovery.restartDescription")}
          </p>
          <Button disabled={busy} onClick={() => void restart()}>
            {t("secrets.restart")}
          </Button>
        </div>
      )}
      <Button
        type="button"
        variant="outline"
        disabled={busy || recovery.isFetching}
        onClick={() => void recheck()}
      >
        {t("startupRecovery.recheck")}
      </Button>
    </section>
  );
}

function RecoveryInput({
  busy,
  recover,
}: {
  busy: boolean;
  recover: (sourceText: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const id = useId();
  const input = useRef<LiteralKeyInputHandle>(null);
  const submitting = useRef(false);
  const [hasInput, setHasInput] = useState(false);
  return (
    <form
      className="space-y-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (busy || submitting.current) return;
        const sourceText = input.current?.value ?? "";
        if (sourceText.length === 0) return;
        submitting.current = true;
        if (input.current) input.current.value = "";
        setHasInput(false);
        void recover(sourceText).finally(() => {
          submitting.current = false;
        });
      }}
    >
      <Label htmlFor={id}>{t("startupRecovery.sourceKey")}</Label>
      <LiteralKeyInput
        id={id}
        ref={input}
        disabled={busy}
        autoComplete="off"
        onValueChange={() =>
          setHasInput((input.current?.value.length ?? 0) > 0)
        }
      />
      <p className="text-xs text-muted-foreground">
        {t("startupRecovery.sourceHelp")}
      </p>
      <CredentialKeyRecoveryHelp />
      <Button type="submit" disabled={busy || !hasInput}>
        {t("startupRecovery.resume")}
      </Button>
    </form>
  );
}
