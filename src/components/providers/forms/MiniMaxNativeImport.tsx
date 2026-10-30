import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { type MiniMaxCodeRegion } from "@/lib/api/minimaxCode";
import { useMiniMaxNativeImportViewModel } from "./useMiniMaxNativeImportViewModel";

export function MiniMaxNativeImport({
  region,
  disabled = false,
  onSaved,
}: {
  region: MiniMaxCodeRegion;
  disabled?: boolean;
  onSaved?: () => void;
}) {
  const { t } = useTranslation();
  const copy = (key: string) => t(`minimaxCode.nativeImport.${key}`);
  const id = useId();
  const {
    visible,
    path,
    readConsent,
    verifyConsent,
    saveConsent,
    busy,
    failed,
    expired,
    preview,
    canVerify,
    canSave,
    changeOpen,
    choose,
    perform,
    changeReadConsent,
    changeVerifyConsent,
    changeSaveConsent,
  } = useMiniMaxNativeImportViewModel({ region, disabled, onSaved });
  return (
    <>
      <Button
        type="button"
        variant="outline"
        disabled={disabled}
        onClick={() => changeOpen(true)}
      >
        {copy("open")}
      </Button>
      <Dialog open={visible} onOpenChange={changeOpen}>
        <DialogContent className="max-h-[90vh] overflow-y-auto">
          <DialogTitle>{copy("title")}</DialogTitle>
          <DialogDescription>{copy("description")}</DialogDescription>
          <p className="text-sm text-muted-foreground">
            {t("minimaxCode.region")}: {t(`minimaxCode.regions.${region}`)}
          </p>
          <Button
            type="button"
            variant="outline"
            disabled={busy || disabled}
            onClick={() => void choose()}
          >
            {copy("choose")}
          </Button>
          <Input readOnly aria-label={copy("source")} value={path} />
          <div className="flex items-center gap-2">
            <Checkbox
              id={`${id}-read`}
              checked={readConsent}
              disabled={!path || busy || disabled}
              onCheckedChange={(value) => changeReadConsent(value === true)}
            />
            <Label htmlFor={`${id}-read`}>{copy("readConsent")}</Label>
          </div>
          <Button
            type="button"
            disabled={!path || !readConsent || busy || disabled}
            onClick={() => void perform("read")}
          >
            {copy("read")}
          </Button>
          {preview && (
            <div role="status" className="space-y-1 text-sm">
              {preview.maskedSubject !== null && (
                <p>
                  {copy("unverified")}: <span>{preview.maskedSubject}</span>
                </p>
              )}
              <p>
                {copy("expires")}:{" "}
                {new Date(preview.expiresAtMs).toLocaleString()}
              </p>
              {preview.stage === "verified" && (
                <>
                  <p>
                    {copy("verified")}: <span>{preview.verifiedUserId}</span>
                  </p>
                  {preview.label !== null && <p>{preview.label}</p>}
                </>
              )}
            </div>
          )}
          <div className="flex items-center gap-2">
            <Checkbox
              id={`${id}-verify`}
              checked={verifyConsent}
              disabled={!canVerify || busy || disabled}
              onCheckedChange={(value) => changeVerifyConsent(value === true)}
            />
            <Label htmlFor={`${id}-verify`}>{copy("verifyConsent")}</Label>
          </div>
          <Button
            type="button"
            disabled={!canVerify || !verifyConsent || busy || disabled}
            onClick={() => void perform("verify")}
          >
            {copy("verify")}
          </Button>
          <p className="text-sm text-muted-foreground">{copy("accessOnly")}</p>
          {expired && (
            <p role="alert" className="text-sm text-destructive">
              {copy("expired")}
            </p>
          )}
          <div className="flex items-center gap-2">
            <Checkbox
              id={`${id}-save`}
              checked={saveConsent}
              disabled={!canSave || busy || disabled}
              onCheckedChange={(value) => changeSaveConsent(value === true)}
            />
            <Label htmlFor={`${id}-save`}>{copy("saveConsent")}</Label>
          </div>
          <Button
            type="button"
            disabled={!canSave || !saveConsent || busy || disabled}
            onClick={() => void perform("save")}
          >
            {copy("save")}
          </Button>
          {failed && (
            <p role="alert" className="text-sm text-destructive">
              {copy("error")}
            </p>
          )}
          <Button
            type="button"
            variant="ghost"
            onClick={() => changeOpen(false)}
          >
            {copy("cancel")}
          </Button>
        </DialogContent>
      </Dialog>
    </>
  );
}
