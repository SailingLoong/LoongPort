import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { FileDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

interface ProtectionStatus {
  automaticUnlock: boolean;
  passwordConfigured: boolean;
}

const protectionKey = ["secretProtection"] as const;

export function SecretProtectionSettings() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [password, setPassword] = useState("");
  const [rotate, setRotate] = useState(false);
  const [automatic, setAutomatic] = useState<boolean | null>(null);
  const [confirmExport, setConfirmExport] = useState(false);
  const exportPlain = useMutation({
    mutationFn: () => invoke<string | null>("export_plaintext_secrets"),
    onSuccess: () => setConfirmExport(false),
  });
  const status = useQuery({
    queryKey: protectionKey,
    queryFn: () => invoke<ProtectionStatus>("get_secret_protection"),
  });
  const save = useMutation({
    mutationFn: () =>
      invoke(rotate ? "rotate_secret_key" : "set_secret_password", {
        password,
        automaticUnlock: automatic ?? status.data?.automaticUnlock,
      }),
    onSuccess: async () => {
      setPassword("");
      await queryClient.invalidateQueries({ queryKey: protectionKey });
      setAutomatic(null);
      setRotate(false);
    },
  });

  return (
    <section className="space-y-4 rounded-lg border p-5">
      <div className="space-y-1">
        <h3 className="font-medium">{t("secrets.protectionTitle")}</h3>
        <p className="text-sm text-muted-foreground">
          {t("secrets.protectionDescription")}
        </p>
      </div>
      {status.isError ? (
        <p role="alert" className="text-sm text-destructive">
          {t("secrets.statusFailed")}
        </p>
      ) : status.data ? (
        <form
          className="space-y-4"
          onSubmit={(event) => {
            event.preventDefault();
            save.mutate();
          }}
        >
          <div className="space-y-2">
            <Label htmlFor="protection-password">
              {t(
                status.data.passwordConfigured
                  ? "secrets.newPassword"
                  : "secrets.password",
              )}
            </Label>
            <Input
              id="protection-password"
              type="password"
              autoComplete="new-password"
              value={password}
              disabled={save.isPending}
              onChange={(event) => setPassword(event.target.value)}
            />
          </div>
          <div className="flex items-center justify-between gap-4">
            <div className="space-y-1">
              <Label htmlFor="automatic-unlock">
                {t("secrets.automaticUnlock")}
              </Label>
              <p className="text-xs text-muted-foreground">
                {t("secrets.automaticUnlockDescription")}
              </p>
            </div>
            <Switch
              id="automatic-unlock"
              checked={automatic ?? status.data.automaticUnlock}
              onCheckedChange={setAutomatic}
              disabled={save.isPending}
            />
          </div>
          <p className="text-xs text-muted-foreground">
            {t("secrets.passwordRecoveryNotice")}
          </p>
          <div className="flex items-center justify-between gap-4">
            <div className="space-y-1">
              <Label htmlFor="rotate-secret-key">
                {t("secrets.rotateKey")}
              </Label>
              <p className="text-xs text-muted-foreground">
                {t("secrets.rotateDescription")}
              </p>
            </div>
            <Switch
              id="rotate-secret-key"
              checked={rotate}
              onCheckedChange={setRotate}
              disabled={save.isPending}
            />
          </div>
          {rotate && (
            <p className="text-sm text-muted-foreground">
              {t("secrets.rotateNotice")}
            </p>
          )}
          {save.isError && (
            <p role="alert" className="text-sm text-destructive">
              {t("secrets.saveFailed")}
            </p>
          )}
          {save.isSuccess && (
            <p role="status" className="text-sm text-muted-foreground">
              {t("secrets.saved")}
            </p>
          )}
          <Button type="submit" disabled={!password || save.isPending}>
            {t(rotate ? "secrets.rotateKey" : "common.save")}
          </Button>
        </form>
      ) : null}
      <div className="flex items-center justify-between gap-4 border-t pt-4">
        <div className="space-y-1">
          <Label>{t("secrets.exportPlainTitle")}</Label>
          <p className="text-xs text-muted-foreground">
            {t("secrets.exportPlainDescription")}
          </p>
          {exportPlain.isSuccess && (
            <p role="status" className="text-xs text-muted-foreground">
              {t("secrets.exportPlainDone")}
            </p>
          )}
          {exportPlain.isError && (
            <p role="alert" className="text-xs text-destructive">
              {t("secrets.exportPlainFailed")}
            </p>
          )}
        </div>
        <Button
          type="button"
          variant="outline"
          onClick={() => setConfirmExport(true)}
          disabled={exportPlain.isPending}
        >
          <FileDown className="h-4 w-4" />
          {t("secrets.exportPlainAction")}
        </Button>
      </div>
      <Dialog open={confirmExport} onOpenChange={setConfirmExport}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>{t("secrets.exportPlainConfirmTitle")}</DialogTitle>
            <DialogDescription>
              {t("secrets.exportPlainWarning")}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="gap-2">
            <Button variant="ghost" onClick={() => setConfirmExport(false)}>
              {t("common.cancel")}
            </Button>
            <Button
              disabled={exportPlain.isPending}
              onClick={() => exportPlain.mutate()}
            >
              {t("secrets.exportPlainConfirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
