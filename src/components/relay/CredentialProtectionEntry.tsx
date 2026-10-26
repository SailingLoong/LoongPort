import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  credentialProtectionApi,
  credentialProtectionKey,
} from "@/lib/api/credentialProtection";
import { CredentialProtectionPanel } from "./CredentialProtectionPanel";

/** The native owner decides when a first-run/upgrade choice is due. Closing is not skipping. */
export function CredentialProtectionEntry({
  showInline = false,
}: {
  showInline?: boolean;
}) {
  const { t } = useTranslation();
  const [dismissed, setDismissed] = useState(false);
  const protection = useQuery({
    queryKey: credentialProtectionKey,
    queryFn: credentialProtectionApi.status,
    retry: false,
  });
  const open =
    !protection.isError && protection.data?.shouldPrompt === true && !dismissed;
  return (
    <>
      {showInline && !open && <CredentialProtectionPanel />}
      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (!next) setDismissed(true);
        }}
      >
        <DialogContent className="max-h-[90vh] max-w-xl overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{t("storageProtection.promptTitle")}</DialogTitle>
            <DialogDescription>
              {t("storageProtection.promptDescription")}
            </DialogDescription>
          </DialogHeader>
          {open && <CredentialProtectionPanel setupPrompt />}
        </DialogContent>
      </Dialog>
    </>
  );
}
