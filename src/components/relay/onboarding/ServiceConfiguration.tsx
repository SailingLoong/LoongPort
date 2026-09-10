import { useTranslation } from "react-i18next";
import { ArrowLeft, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { getAppDisplayName, APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
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
    confirmation,
    selectFor,
    resolveConfirmation,
    finish,
  } = useServiceConfiguration(account, sourceAppId, onDone);
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
        {APP_IDS.map((app) => {
          const available =
            choices.data?.filter((choice) => choice.app === app) ?? [];
          if (!available.length) return null;
          return (
            <label
              key={app}
              className="flex items-center justify-between gap-4 py-4 text-sm"
            >
              <span>{getAppDisplayName(app, t)}</span>
              <select
                aria-label={getAppDisplayName(app, t)}
                disabled={busy}
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
            </label>
          );
        })}
      </div>
      {choices.data?.length === 0 && (
        <p className="my-5 text-sm text-muted-foreground">
          {t("loongport.onboarding.noConfigurations")}
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
          {t("loongport.onboarding.finish")}
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
