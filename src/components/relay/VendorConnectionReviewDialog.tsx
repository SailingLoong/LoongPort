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

/** Presentation only. The caller owns the captured native review proof and one-shot confirmation. */
export function VendorConnectionReviewDialog({
  targetName,
  disabled = false,
  onCancel,
  onConfirm,
}: {
  targetName: string | null;
  disabled?: boolean;
  onCancel: () => void;
  onConfirm: () => void | Promise<void>;
}) {
  const { t } = useTranslation();
  return (
    <Dialog
      open={targetName !== null}
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("applications.vendorRecovery.title")}</DialogTitle>
          <DialogDescription>
            {t("applications.vendorRecovery.description", {
              name: targetName ?? "",
            })}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button
            disabled={disabled}
            onClick={() => {
              void onConfirm();
            }}
          >
            {t("applications.vendorRecovery.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
