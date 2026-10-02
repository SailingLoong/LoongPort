import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { getAppDisplayName, PROVIDER_STORE_APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import { isTextEditableTarget } from "@/utils/domUtils";
import { extractErrorMessage } from "@/utils/errorUtils";
import { SwitchTierConfirmDialog } from "../SwitchTierConfirmDialog";
import type { ConnectedService } from "./useServiceOnboarding";
import { useServiceConfiguration } from "./useServiceConfiguration";

export function ServiceConfiguration({
  account,
  sourceAppId,
  onBack,
  onDone,
}: {
  account: ConnectedService;
  sourceAppId: AppId;
  onBack: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const {
    choices,
    status,
    selection,
    setSelection,
    shareData,
    setShareData,
    busy,
    results,
    completionError,
    reapply,
    confirmation,
    selectFor,
    resolveConfirmation,
    finish,
  } = useServiceConfiguration(account, sourceAppId, onDone);
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (
        event.key !== "Escape" ||
        event.defaultPrevented ||
        document.body.style.overflow === "hidden" ||
        isTextEditableTarget(event.target)
      )
        return;
      event.preventDefault();
      if (!busy) onBack();
    };
    // The active stage handles Escape before the shell's window listener.
    // PreservedView suspends this effect when the stage is hidden.
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [busy, onBack]);

  return (
    <div className="mx-auto w-full max-w-2xl pb-6">
      <Button variant="ghost" onClick={onBack} disabled={busy}>
        <ArrowLeft className="h-4 w-4" />
        {t("common.back")}
      </Button>
      <h1 className="mt-5 text-xl font-semibold">
        {t("loongport.onboarding.configure")}
      </h1>
      <p className="mt-2 text-sm text-muted-foreground">{account.name}</p>
      <p className="mt-1 text-sm text-muted-foreground">
        {t("loongport.onboarding.chooseApps")}
      </p>
      {choices.isPending && <Loader2 className="my-6 h-5 w-5 animate-spin" />}
      {(choices.isError || status.isError) && (
        <div role="alert" className="my-4 text-sm">
          {extractErrorMessage(choices.error ?? status.error)}
          <Button
            variant="link"
            onClick={() => {
              void choices.refetch();
              void status.refetch();
            }}
          >
            {t("loongport.directory.actions.retry")}
          </Button>
        </div>
      )}
      <div className="my-6 divide-y rounded-lg border bg-background px-4">
        {PROVIDER_STORE_APP_IDS.map((app) => {
          const available =
            choices.data?.filter((choice) => choice.app === app) ?? [];
          if (!available.length) return null;
          return (
            <div
              key={app}
              className="flex items-center justify-between gap-4 py-4 text-sm"
            >
              <span className="space-y-1">
                <span className="block">{getAppDisplayName(app, t)}</span>
                {results[app] && (
                  <span
                    role="status"
                    className="block text-xs text-muted-foreground"
                  >
                    {t(`loongport.onboarding.results.${results[app]!.state}`)}
                  </span>
                )}
                {results[app]?.error && (
                  <span role="alert" className="block text-xs text-destructive">
                    {results[app]!.error}
                  </span>
                )}
                {(results[app]?.state === "changed" ||
                  results[app]?.state === "unverified") && (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={busy}
                    onClick={(event) => {
                      event.preventDefault();
                      reapply(app);
                    }}
                  >
                    {t("loongport.onboarding.reapply")}
                  </Button>
                )}
              </span>
              <select
                aria-label={getAppDisplayName(app, t)}
                disabled={busy || Object.keys(results).length > 0}
                value={selectFor(app)}
                onChange={(event) =>
                  setSelection({ ...selection, [app]: event.target.value })
                }
                className="max-w-[60%] rounded-md border bg-background px-3 py-2"
              >
                <option value="">{t("loongport.onboarding.skipApp")}</option>
                {available.map((choice) => (
                  <option value={choice.id} key={choice.id}>
                    {choice.name}
                  </option>
                ))}
              </select>
            </div>
          );
        })}
      </div>
      {choices.data?.length === 0 && (
        <p className="my-5 text-sm text-muted-foreground">
          {t("loongport.onboarding.noConfigurations")}
        </p>
      )}
      {Object.keys(results).length > 0 && (
        <p className="mb-4 text-sm text-muted-foreground">
          {t("loongport.onboarding.resultsHint")}
        </p>
      )}
      {completionError && (
        <p role="alert" className="mb-4 text-sm text-destructive">
          {completionError}
        </p>
      )}
      {status.data && !status.data.completed && (
        <div className="mb-6">
          <label className="flex items-start gap-2 text-[13px] leading-5">
            <input
              type="checkbox"
              className="mt-1 accent-blue-600"
              checked={shareData}
              disabled={busy}
              onChange={(event) => setShareData(event.target.checked)}
            />
            <span>{t("loongport.onboarding.shareLabel")}</span>
          </label>
          <details className="ml-5 mt-2 text-[13px] leading-5 text-muted-foreground">
            <summary className="cursor-pointer">
              {t("loongport.onboarding.shareDetails")}
            </summary>
            <p className="mt-2">{t("loongport.onboarding.shareUsage")}</p>
            <p className="mt-2">
              {t("loongport.onboarding.shareMeasurements")}
            </p>
            <p className="mt-2">{t("loongport.onboarding.shareControl")}</p>
          </details>
        </div>
      )}
      <div className="flex justify-end border-t border-border-default pt-5">
        <Button
          onClick={() => void finish()}
          disabled={busy || !choices.data || !status.data}
        >
          {busy && <Loader2 className="h-4 w-4 animate-spin" />}
          {t(
            Object.keys(results).length > 0
              ? "loongport.onboarding.continueSetup"
              : "loongport.onboarding.finish",
          )}
        </Button>
      </div>
      <SwitchTierConfirmDialog
        targetName={confirmation}
        onCancel={() => resolveConfirmation(null)}
        onSwitch={resolveConfirmation}
      />
    </div>
  );
}
