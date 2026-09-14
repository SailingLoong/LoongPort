import { useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";

interface ResetPreview {
  fingerprint: string;
  protectedValues: number;
}
export function SecretReset({
  onReset,
  onNeedsRestart,
}: {
  onReset: () => void;
  onNeedsRestart: () => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [preview, setPreview] = useState<ResetPreview | null>(null);
  const [password, setPassword] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const [archive, setArchive] = useState<string | null>(null);
  async function prepare() {
    setOpen(true);
    setBusy(true);
    setFailed(false);
    setPreview(null);
    setConfirmed(false);
    try {
      setPreview(await invoke<ResetPreview>("preview_secret_reset"));
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  }
  async function reset() {
    if (!preview || !confirmed) return;
    setBusy(true);
    setFailed(false);
    try {
      setArchive(
        await invoke<string>("reset_secret_vault", {
          fingerprint: preview.fingerprint,
          password,
        }),
      );
      setPassword("");
    } catch (error) {
      setFailed(true);
      if (
        typeof error === "object" &&
        error !== null &&
        "restartRequired" in error &&
        error.restartRequired === true
      ) {
        setOpen(false);
        setPassword("");
        onNeedsRestart();
      }
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <Button
        type="button"
        variant="link"
        className="w-full text-muted-foreground"
        onClick={() => void prepare()}
      >
        {t("secrets.resetTitle")}
      </Button>
      <Dialog
        open={open}
        onOpenChange={(value) => {
          if (!busy) {
            setOpen(value);
            setPassword("");
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("secrets.resetTitle")}</DialogTitle>
            <DialogDescription>
              {t("secrets.resetDescription")}
            </DialogDescription>
          </DialogHeader>
          {archive ? (
            <div className="space-y-4">
              <p>{t("secrets.resetCompleted")}</p>
              <p className="break-all text-sm text-muted-foreground">
                {archive}
              </p>
              <Button onClick={onReset}>{t("secrets.enterApp")}</Button>
            </div>
          ) : (
            <form
              className="space-y-4"
              onSubmit={(event) => {
                event.preventDefault();
                void reset();
              }}
            >
              <p className="text-sm">{t("secrets.resetArchiveNotice")}</p>
              {preview && (
                <p className="text-sm text-muted-foreground">
                  {t("secrets.resetImpact", { count: preview.protectedValues })}
                </p>
              )}
              <div className="space-y-2">
                <Label htmlFor="reset-password">
                  {t("secrets.newPassword")}
                </Label>
                <Input
                  id="reset-password"
                  type="password"
                  autoComplete="new-password"
                  value={password}
                  onChange={(event) => setPassword(event.target.value)}
                  disabled={busy}
                />
              </div>
              <label className="flex items-start gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={confirmed}
                  onChange={(event) => setConfirmed(event.target.checked)}
                  disabled={busy}
                />
                <span>{t("secrets.resetConfirmation")}</span>
              </label>
              {failed && (
                <p role="alert" className="text-sm text-destructive">
                  {t("secrets.resetFailed")}
                </p>
              )}
              <Button
                type="submit"
                variant="destructive"
                disabled={!preview || !confirmed || !password || busy}
              >
                {t("secrets.resetAction")}
              </Button>
            </form>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}
