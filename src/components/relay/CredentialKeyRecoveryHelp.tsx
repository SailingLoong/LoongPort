import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { isLinux, isMac, isWindows } from "@/lib/platform";

/** Guidance only: never reads, displays or guesses the system-stored secret. */
export function CredentialKeyRecoveryHelp() {
  const { t } = useTranslation();
  const platform = isMac()
    ? "mac"
    : isWindows()
      ? "windows"
      : isLinux()
        ? "linux"
        : "unknown";
  return (
    <Collapsible className="space-y-3">
      <CollapsibleTrigger asChild>
        <Button type="button" variant="link" className="h-auto p-0">
          {t("storageProtection.forgotKey")}
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent className="space-y-2 rounded-lg bg-muted/50 p-4 text-sm text-muted-foreground">
        <p>{t(`storageProtection.recovery.${platform}`)}</p>
        <p>{t("storageProtection.recovery.binaryNotice")}</p>
      </CollapsibleContent>
    </Collapsible>
  );
}
