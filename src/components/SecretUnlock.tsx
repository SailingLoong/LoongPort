import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { LockKeyhole } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SecretRestore } from "@/components/SecretRestore";
import { SecretReset } from "@/components/SecretReset";
import { Label } from "@/components/ui/label";

interface StartupRecovery {
  token: string;
  status: "pending" | "completed" | "verification_required";
  canRecover: boolean;
  restartRequired: boolean;
}

export function SecretUnlock({
  onUnlocked = () => window.location.reload(),
  requiresRestart = false,
  initialError,
}: {
  onUnlocked?: () => void;
  requiresRestart?: boolean;
  initialError?: string;
}) {
  const { t } = useTranslation();
  const settingUp = initialError === "secret.password_setup_required";
  const recoveryRequired = initialError === "secret.recovery_required";
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(initialError ?? null);
  const [restartRequired, setRestartRequired] = useState(requiresRestart);
  const [recovery, setRecovery] = useState<StartupRecovery | null>(null);

  useEffect(() => {
    if (!recoveryRequired) return;
    let active = true;
    void invoke<StartupRecovery>("get_startup_recovery").then(
      (view) => {
        if (!active) return;
        setRecovery(view);
        setRestartRequired(view.restartRequired);
      },
      () => active && setFailure("secret.operation_failed"),
    );
    return () => {
      active = false;
    };
  }, [recoveryRequired]);

  async function recheckRecovery() {
    setRecovery(null);
    try {
      const view = await invoke<StartupRecovery>("get_startup_recovery");
      setRecovery(view);
      setRestartRequired(view.restartRequired);
    } catch {
      setFailure("secret.operation_failed");
    }
  }

  async function unlock(usePassword: boolean) {
    setBusy(true);
    setFailure(null);
    try {
      if (recoveryRequired) {
        if (!recovery?.canRecover) return;
        const view = await invoke<StartupRecovery>(
          "recover_startup_operation",
          {
            token: recovery.token,
            password,
          },
        );
        setRecovery(view);
        setRestartRequired(view.restartRequired);
        return;
      }
      await invoke("unlock_secret_vault", {
        password: usePassword ? password : null,
      });
      setPassword("");
      onUnlocked();
    } catch (error) {
      setFailure(
        typeof error === "string"
          ? error
          : typeof error === "object" &&
              error !== null &&
              "code" in error &&
              typeof error.code === "string"
            ? error.code
            : "secret.operation_failed",
      );
      if (
        typeof error === "object" &&
        error !== null &&
        "restartRequired" in error &&
        error.restartRequired === true
      ) {
        setRestartRequired(true);
      }
      if (recoveryRequired) await recheckRecovery();
    } finally {
      if (recoveryRequired) setPassword("");
      setBusy(false);
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center p-6">
      <form
        className="w-full max-w-sm space-y-5 rounded-xl border bg-card p-6 shadow-sm"
        onSubmit={(event) => {
          event.preventDefault();
          void unlock(true);
        }}
      >
        <LockKeyhole className="h-7 w-7 text-muted-foreground" aria-hidden />
        <div className="space-y-2">
          <h1 className="text-xl font-semibold">
            {t(
              recoveryRequired
                ? "secrets.recoveryTitle"
                : settingUp
                  ? "secrets.setupTitle"
                  : "secrets.unlockTitle",
            )}
          </h1>
          <p className="text-sm text-muted-foreground">
            {t(
              recovery?.status === "completed"
                ? "secrets.recoveryCompleted"
                : recovery?.status === "verification_required"
                  ? "secrets.recoveryVerificationRequired"
                  : restartRequired
                    ? "secrets.restartDescription"
                    : recoveryRequired
                      ? "secrets.recoveryDescription"
                      : settingUp
                        ? "secrets.setupDescription"
                        : "secrets.unlockDescription",
            )}
          </p>
        </div>
        {!restartRequired && (
          <div className="space-y-2">
            <Label htmlFor="vault-password">{t("secrets.password")}</Label>
            <Input
              id="vault-password"
              type="password"
              autoComplete={settingUp ? "new-password" : "current-password"}
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              disabled={busy}
            />
          </div>
        )}
        {failure && !restartRequired && (
          <p role="alert" className="text-sm text-destructive">
            {t(unlockErrorKey(failure))}
          </p>
        )}
        {restartRequired ? (
          <Button
            className="w-full"
            type="button"
            onClick={() => void invoke("restart_app")}
          >
            {t("secrets.restart")}
          </Button>
        ) : (
          <>
            <Button
              className="w-full"
              type="submit"
              disabled={
                busy || !password || (recoveryRequired && !recovery?.canRecover)
              }
            >
              {t(
                busy
                  ? "secrets.unlocking"
                  : recoveryRequired
                    ? "secrets.resumeRecovery"
                    : "secrets.unlock",
              )}
            </Button>
            {recoveryRequired ? (
              <Button
                className="w-full"
                variant="outline"
                type="button"
                disabled={busy}
                onClick={() => void recheckRecovery()}
              >
                {t("secrets.recheckOperation")}
              </Button>
            ) : (
              <>
                <Button
                  className="w-full"
                  variant="outline"
                  type="button"
                  disabled={busy}
                  onClick={() => void unlock(false)}
                >
                  {t("secrets.retrySystemUnlock")}
                </Button>
                <SecretRestore
                  onRestored={onUnlocked}
                  onNeedsRestart={() => setRestartRequired(true)}
                />
                <SecretReset
                  onReset={() => void invoke("restart_app")}
                  onNeedsRestart={() => setRestartRequired(true)}
                />
              </>
            )}
          </>
        )}
      </form>
    </main>
  );
}

function unlockErrorKey(code: string): string {
  switch (code) {
    case "secret.password_rejected":
      return "secrets.passwordRejected";
    case "secret.password_unavailable":
      return "secrets.passwordUnavailable";
    case "secret.password_too_short":
      return "secrets.passwordTooShort";
    case "secret.password_setup_required":
      return "secrets.setupDescription";
    case "secret.store_unavailable":
      return "secrets.storeUnavailable";
    case "secret.key_missing":
      return "secrets.keyMissing";
    case "secret.unsupported_format":
    case "secret.resource_limit":
      return "secrets.unsupportedFormat";
    case "secret.authentication_failed":
    case "secret.invalid_envelope":
    case "secret.invalid_metadata":
    case "secret.invalid_file":
      return "secrets.corruptData";
    case "secret.identity_mismatch":
    case "secret.key_rejected":
      return "secrets.keyMismatch";
    case "secret.locked":
      return "secrets.unlockDescription";
    default:
      return "secrets.unlockFailed";
  }
}
