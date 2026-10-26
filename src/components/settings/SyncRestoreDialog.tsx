import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  LiteralKeyInput,
  type LiteralKeyInputHandle,
} from "@/components/relay/LiteralKeyInput";
import { Label } from "@/components/ui/label";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

export function isSyncRestoreRequired(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return [
    "sync.vault_adoption_required",
    "sync.vault_revision_newer",
    "sync.vault_metadata_conflict",
  ].some((code) => message.includes(code));
}

export function SyncRestoreDialog({
  deviceName,
  sourceInputRequired = true,
  onRestore,
  onClose,
}: {
  deviceName: string;
  sourceInputRequired?: boolean;
  onRestore: (password: string) => Promise<void>;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const password = useRef<LiteralKeyInputHandle>(null);
  const [hasPassword, setHasPassword] = useState(false);
  const inFlight = useRef(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  // Only explicit native no-input authority permits the empty original wire value.
  const requiresInput = sourceInputRequired !== false;
  useEffect(() => {
    if (password.current) password.current.value = "";
    setHasPassword(false);
  }, [sourceInputRequired]);

  const clearPassword = () => {
    if (password.current) password.current.value = "";
    setHasPassword(false);
  };
  const close = () => {
    if (inFlight.current) return;
    clearPassword();
    onClose();
  };
  const restore = async () => {
    const sourceInput = requiresInput ? (password.current?.value ?? "") : "";
    if (inFlight.current || (requiresInput && sourceInput.length === 0)) return;
    inFlight.current = true;
    clearPassword();
    setPending(true);
    setFailure(null);
    try {
      await onRestore(sourceInput);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setFailure(
        message.includes("sync.conflict")
          ? "syncRestore.changed"
          : "syncRestore.failed",
      );
    } finally {
      inFlight.current = false;
      setPending(false);
    }
  };

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
    >
      <DialogContent>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void restore();
          }}
          className="space-y-4"
        >
          <DialogHeader>
            <DialogTitle>{t("syncRestore.title")}</DialogTitle>
            <DialogDescription>
              {t(
                requiresInput
                  ? "syncRestore.description"
                  : "syncRestore.descriptionPlain",
              )}
            </DialogDescription>
          </DialogHeader>
          <p className="text-sm text-muted-foreground">
            {t("syncRestore.source", { deviceName })}
          </p>
          <p className="text-sm text-muted-foreground">
            {t("syncRestore.protection")}
          </p>
          {requiresInput ? (
            <div className="space-y-2">
              <Label htmlFor="sync-restore-password">
                {t("syncRestore.password")}
              </Label>
              <LiteralKeyInput
                ref={password}
                id="sync-restore-password"
                disabled={pending}
                onValueChange={() =>
                  setHasPassword((password.current?.value.length ?? 0) > 0)
                }
              />
            </div>
          ) : (
            <p className="text-sm text-muted-foreground">
              {t("syncRestore.noSourceInput")}
            </p>
          )}
          {failure && (
            <p role="alert" className="text-sm text-destructive">
              {t(failure)}
            </p>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={pending}
              onClick={close}
            >
              {t("common.cancel")}
            </Button>
            <Button
              type="submit"
              variant="destructive"
              disabled={pending || (requiresInput && !hasPassword)}
            >
              {t(pending ? "syncRestore.restoring" : "syncRestore.confirm")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
