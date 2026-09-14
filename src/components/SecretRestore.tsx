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
interface RestorePreview {
  snapshotId: string;
  deviceName: string;
  createdAt: string;
}
type Transport = "webdav" | "s3";
const fields = {
  webdav: ["baseUrl", "username", "password", "remoteRoot", "profile"],
  s3: [
    "region",
    "bucket",
    "accessKeyId",
    "secretAccessKey",
    "endpoint",
    "remoteRoot",
    "profile",
  ],
} as const;
export function SecretRestore({
  onRestored,
  onNeedsRestart,
}: {
  onRestored: () => void;
  onNeedsRestart: () => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [transport, setTransport] = useState<Transport>("webdav");
  const [values, setValues] = useState<Record<string, string>>({});
  const [preview, setPreview] = useState<RestorePreview | null>(null);
  const [password, setPassword] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [automatic, setAutomatic] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  function source() {
    return {
      transport,
      settings: Object.fromEntries(
        fields[transport]
          .filter((field) => values[field]?.length)
          .map((field) => [field, values[field]]),
      ),
    };
  }
  async function inspect() {
    setBusy(true);
    setFailed(false);
    setPreview(null);
    setConfirmed(false);
    try {
      setPreview(
        await invoke<RestorePreview>("preview_startup_restore", {
          source: source(),
        }),
      );
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  }
  async function restore() {
    if (!preview || !confirmed) return;
    setBusy(true);
    setFailed(false);
    try {
      await invoke("restore_startup_vault", {
        source: source(),
        password,
        expectedSnapshotId: preview.snapshotId,
        automaticUnlock: automatic,
      });
      setPassword("");
      setValues({});
      onRestored();
    } catch (error) {
      setFailed(true);
      if (
        typeof error === "object" &&
        error !== null &&
        "restartRequired" in error &&
        error.restartRequired === true
      ) {
        setPassword("");
        setValues({});
        setOpen(false);
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
        variant="outline"
        className="w-full"
        onClick={() => setOpen(true)}
      >
        {t("secretRestore.title")}
      </Button>
      <Dialog
        open={open}
        onOpenChange={(value) => {
          if (!busy) {
            setOpen(value);
            if (!value) {
              setPassword("");
              setValues({});
              setPreview(null);
            }
          }
        }}
      >
        <DialogContent className="max-h-[90vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{t("secretRestore.title")}</DialogTitle>
            <DialogDescription>
              {t("secretRestore.description")}
            </DialogDescription>
          </DialogHeader>
          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (preview) void restore();
              else void inspect();
            }}
          >
            <div className="space-y-2">
              <Label htmlFor="restore-transport">
                {t("secretRestore.connection")}
              </Label>
              <select
                id="restore-transport"
                value={transport}
                disabled={busy}
                className="h-9 w-full rounded-md border bg-background px-3 text-sm"
                onChange={(event) => {
                  setTransport(event.target.value as Transport);
                  setPreview(null);
                  setConfirmed(false);
                  setValues({});
                }}
              >
                <option value="webdav">WebDAV</option>
                <option value="s3">S3</option>
              </select>
            </div>
            {fields[transport].map((field) => (
              <div className="space-y-2" key={field}>
                <Label htmlFor={`startup-restore-${field}`}>
                  {t(
                    `settings.${transport === "webdav" ? "webdavSync" : "s3Sync"}.${field}`,
                  )}
                </Label>
                <Input
                  id={`startup-restore-${field}`}
                  type={
                    field === "password" || field === "secretAccessKey"
                      ? "password"
                      : "text"
                  }
                  value={values[field] ?? ""}
                  autoComplete="off"
                  disabled={busy}
                  onChange={(event) => {
                    setValues({ ...values, [field]: event.target.value });
                    setPreview(null);
                    setConfirmed(false);
                  }}
                />
              </div>
            ))}
            <Button
              type="button"
              variant="outline"
              onClick={() => void inspect()}
              disabled={busy}
            >
              {t("secretRestore.inspect")}
            </Button>
            {preview && (
              <div className="space-y-4 rounded-md border p-4">
                <p className="text-sm">
                  {t("syncRestore.source", { deviceName: preview.deviceName })}
                </p>
                <div className="space-y-2">
                  <Label htmlFor="startup-recovery-password">
                    {t("syncRestore.password")}
                  </Label>
                  <Input
                    id="startup-recovery-password"
                    type="password"
                    autoComplete="current-password"
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                    disabled={busy}
                  />
                </div>
                <label className="flex items-start gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={automatic}
                    onChange={(event) => setAutomatic(event.target.checked)}
                    disabled={busy}
                  />
                  {t("secrets.automaticUnlock")}
                </label>
                <label className="flex items-start gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={confirmed}
                    onChange={(event) => setConfirmed(event.target.checked)}
                    disabled={busy}
                  />
                  {t("secretRestore.confirmation")}
                </label>
                <Button
                  type="submit"
                  disabled={!confirmed || !password || busy}
                >
                  {t("syncRestore.confirm")}
                </Button>
              </div>
            )}
            {failed && (
              <p role="alert" className="text-sm text-destructive">
                {t("secretRestore.failed")}
              </p>
            )}
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
