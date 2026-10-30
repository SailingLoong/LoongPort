import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  useAutomaticClaimControlsVM,
  type AutomaticClaimControlsInput,
} from "./useAutomaticClaimControlsVM";

export function AutomaticClaimControls(props: AutomaticClaimControlsInput) {
  const { t } = useTranslation();
  const vm = useAutomaticClaimControlsVM(props);
  const { rows, enabled, busy, completed } = vm;
  return (
    <section
      aria-label={t("claimSettings.title")}
      className="rounded-md border border-border bg-card px-4 py-3"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="space-y-1">
          <h3 className="text-sm font-semibold">
            {t("claimSettings.heading")}{" "}
            <span className="ml-2 text-xs font-normal text-muted-foreground">
              {t(vm.status)}
            </span>
          </h3>
          <p className="text-xs text-muted-foreground">
            {t("claimSettings.scope", {
              scope: props.scope,
              count: props.accounts.length,
              enabled,
            })}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={vm.enableDisabled}
            onClick={() => void vm.enable()}
          >
            {t("claimSettings.enable")}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={vm.disableDisabled}
            onClick={() => void vm.disable()}
          >
            {t("claimSettings.disable")}
          </Button>
        </div>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        {t("claimSettings.notice")}
      </p>
      <div aria-live="polite" className="mt-2 space-y-1 text-xs">
        {busy && <p role="status">{t("claimSettings.saving")}</p>}
        {completed !== null && (
          <p role="status">
            {t("claimSettings.saved", { count: completed.count })} ·{" "}
            {completed.scope}
          </p>
        )}
        {rows.map(({ account, key, message, alert }) =>
          message ? (
            <p key={key} role={alert ? "alert" : undefined}>
              {account.label}：{t(message)}
            </p>
          ) : null,
        )}
      </div>
    </section>
  );
}
