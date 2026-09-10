import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";

export function FirstVisitDomainDialog({
  open,
  onDismiss,
  onSubmit,
  onOfficial,
  onBrowse,
  domain: controlledDomain,
  onDomainChange,
}: {
  open: boolean;

  onDismiss: () => void;

  onSubmit: (domain: string) => void;
  onOfficial?: () => void;
  onBrowse?: () => void;
  domain?: string;
  onDomainChange?: (domain: string) => void;
}) {
  const { t } = useTranslation();
  const [localDomain, setLocalDomain] = useState("");
  const domain = controlledDomain ?? localDomain;
  const setDomain = onDomainChange ?? setLocalDomain;
  const submit = () => {
    if (domain.trim()) onSubmit(domain.trim());
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onDismiss();
      }}
    >
      <DialogContent className="max-w-[26rem] gap-0 p-6" zIndex="top">
        <DialogTitle className="text-base font-semibold">
          {t("loongport.firstSite.title")}
        </DialogTitle>

        <Input
          autoFocus
          className="mt-4"
          value={domain}
          onChange={(event) => setDomain(event.target.value)}
          placeholder={t("loongport.onboarding.domainPlaceholder")}
          aria-label={t("loongport.onboarding.domain")}
          onKeyDown={(event) => {
            if (event.key === "Enter") submit();
          }}
        />

        <div className="mt-3 flex gap-3">
          {onOfficial && (
            <Button variant="link" size="sm" onClick={onOfficial}>
              {t("loongport.sections.official")}
            </Button>
          )}
          {onBrowse && (
            <Button variant="link" size="sm" onClick={onBrowse}>
              {t("loongport.onboarding.browse")}
            </Button>
          )}
        </div>
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="ghost" size="sm" onClick={onDismiss}>
            {t("loongport.firstSite.cancel")}
          </Button>
          <Button
            size="sm"
            data-testid="first-site-confirm"
            disabled={!domain.trim()}
            onClick={submit}
          >
            {t("loongport.firstSite.confirm")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
