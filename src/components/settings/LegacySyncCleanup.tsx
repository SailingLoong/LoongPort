import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { settingsApi } from "@/lib/api";
import type { LegacyCleanupPreview } from "@/lib/api/settings";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

export function LegacySyncCleanup({
  transport,
  disabled,
}: {
  transport: "webdav" | "s3";
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [preview, setPreview] = useState<LegacyCleanupPreview | null>(null);
  const check = async () => {
    setBusy(true);
    try {
      const result = await (transport === "webdav"
        ? settingsApi.webdavSyncLegacyCleanupPreview()
        : settingsApi.s3SyncLegacyCleanupPreview());
      if (result.paths.length === 0) toast.info(t("legacySyncCleanup.empty"));
      else setPreview(result);
    } catch {
      toast.error(t("legacySyncCleanup.checkFailed"));
    } finally {
      setBusy(false);
    }
  };
  const clean = async () => {
    if (!preview?.canClean || !preview.receipt) return;
    setBusy(true);
    try {
      await (transport === "webdav"
        ? settingsApi.webdavSyncCleanupLegacy(preview.receipt)
        : settingsApi.s3SyncCleanupLegacy(preview.receipt));
      toast.success(t("legacySyncCleanup.success"));
    } catch (error) {
      const message =
        error instanceof Error ? error.message : String(error ?? "");
      toast.error(
        t(
          message.includes("sync.cleanup_conditions_unsupported")
            ? "legacySyncCleanup.unsupported"
            : "legacySyncCleanup.failed",
        ),
      );
    } finally {
      setBusy(false);
      setPreview(null);
    }
  };
  return (
    <>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={disabled || busy}
        onClick={check}
      >
        {t(busy ? "legacySyncCleanup.working" : "legacySyncCleanup.check")}
      </Button>
      <Dialog
        open={preview !== null}
        onOpenChange={(open) => {
          if (!open && !busy) setPreview(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("legacySyncCleanup.title")}</DialogTitle>
            <DialogDescription>
              {t("legacySyncCleanup.description")}
            </DialogDescription>
          </DialogHeader>
          <p className="text-sm text-muted-foreground">
            {t("legacySyncCleanup.versionHistory")}
          </p>
          {preview && !preview.canClean && (
            <p className="text-sm text-amber-600">
              {t("legacySyncCleanup.backupRequired")}
            </p>
          )}
          <DialogFooter>
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => setPreview(null)}
            >
              {t("common.cancel")}
            </Button>
            <Button
              variant="destructive"
              disabled={busy || !preview?.canClean || !preview.receipt}
              onClick={clean}
            >
              {t(
                busy
                  ? "legacySyncCleanup.working"
                  : "legacySyncCleanup.confirm",
              )}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
