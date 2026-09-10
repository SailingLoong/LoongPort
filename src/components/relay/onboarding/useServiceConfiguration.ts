import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import { relayApi } from "@/lib/api/relay";
import { vendorApi } from "@/lib/api/vendor";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  type ConnectedService,
  useServiceConfigurationChoices,
  useServiceOnboardingStatus,
  serviceOnboardingKey,
} from "./useServiceOnboarding";

export function useServiceConfiguration(
  account: ConnectedService,
  sourceAppId: AppId,
  onDone: () => void,
) {
  const client = useQueryClient();
  const choices = useServiceConfigurationChoices(account);
  const status = useServiceOnboardingStatus();
  const [selection, setSelection] = useState<Partial<Record<AppId, string>>>(
    {},
  );
  const [shareData, setShareData] = useState(true);
  const [busy, setBusy] = useState(false);
  const [confirmation, setConfirmation] = useState<string | null>(null);
  const confirmationResolver = useRef<((value: boolean | null) => void) | null>(
    null,
  );
  const mounted = useRef(true);
  const lifecycle = useRef(0);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      lifecycle.current += 1;
      confirmationResolver.current?.(null);
      confirmationResolver.current = null;
      setConfirmation(null);
      setBusy(false);
    };
  }, []);
  const selectFor = (app: AppId) =>
    selection[app] ??
    (app === sourceAppId
      ? (choices.data?.find((choice) => choice.app === app)?.id ?? "")
      : "");
  const resolveConfirmation = (value: boolean | null) => {
    confirmationResolver.current?.(value);
    confirmationResolver.current = null;
    setConfirmation(null);
  };
  const finish = async () => {
    if (busy || !choices.data || !status.data) return;
    const activeLifecycle = lifecycle.current;
    const isActive = () =>
      mounted.current && lifecycle.current === activeLifecycle;
    setBusy(true);
    try {
      for (const app of APP_IDS) {
        const id = selectFor(app);
        if (!id) continue;
        const switchConfig = (quitChatgpt?: boolean) =>
          account.kind === "vendor"
            ? vendorApi.switch(account.rowId, id, app, quitChatgpt)
            : relayApi.switchTier(id, app, quitChatgpt);
        let result = await switchConfig();
        if (!isActive()) return;
        if (result.status === "confirmationRequired") {
          setConfirmation(result.targetName);
          const answer = await new Promise<boolean | null>((resolve) => {
            confirmationResolver.current = resolve;
          });
          if (answer === null) return;
          result = await switchConfig(answer);
        }
        if (!isActive() || result.status !== "switched") return;
        result.warnings.forEach((warning) => toast.warning(warning));
      }
      if (!isActive()) return;
      if (!status.data.completed)
        await serviceOnboardingApi.complete(shareData);
      await Promise.all([
        client.invalidateQueries({ queryKey: serviceOnboardingKey }),
        client.invalidateQueries({ queryKey: ["settings"] }),
      ]);
      if (isActive()) onDone();
    } catch (error) {
      if (isActive()) toast.error(extractErrorMessage(error));
    } finally {
      if (isActive()) setBusy(false);
    }
  };
  return {
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
  };
}
