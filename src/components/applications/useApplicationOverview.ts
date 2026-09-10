import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import {
  applicationOverviewApi,
  type ApplicationConfiguration,
} from "@/lib/api/applicationOverview";
import { relayApi } from "@/lib/api/relay";
import { vendorApi } from "@/lib/api/vendor";
import { PROVIDER_SWITCHED, VENDOR_ACCOUNTS_CHANGED } from "@/lib/api/events";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { extractErrorMessage } from "@/utils/errorUtils";

export function useApplicationOverview(
  appId: AppId,
  providers: Record<string, Provider>,
  onSwitchProvider: (provider: Provider) => void,
) {
  const client = useQueryClient();
  const { t } = useTranslation();
  const key = ["applicationOverview", appId];
  const query = useQuery({
    queryKey: key,
    queryFn: () => applicationOverviewApi.get(appId),
  });
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const [confirmation, setConfirmation] = useState<{
    target: ApplicationConfiguration;
    name: string;
  } | null>(null);
  const lifecycle = useRef(0);
  useEffect(() => {
    return () => {
      lifecycle.current += 1;
    };
  }, [appId]);
  // Provider edits and removals already invalidate this source query. Read the
  // overview again when its public result changes, without starting maintenance.
  useEffect(() => {
    void client.invalidateQueries({ queryKey: ["applicationOverview", appId] });
  }, [appId, providers, client]);
  useTauriEvent(PROVIDER_SWITCHED, () =>
    client.invalidateQueries({ queryKey: key }),
  );
  useTauriEvent(VENDOR_ACCOUNTS_CHANGED, () =>
    client.invalidateQueries({ queryKey: key }),
  );
  const select = async (
    target: ApplicationConfiguration,
    quitChatgpt?: boolean,
  ) => {
    if (!target.canSelect || pending.current) return;
    if (target.selection.kind === "provider") {
      const provider = providers[target.providerId];
      if (provider) onSwitchProvider(provider);
      return;
    }
    const generation = lifecycle.current;
    pending.current = true;
    setBusy(true);
    setConfirmation(null);
    try {
      const result =
        target.selection.kind === "vendor"
          ? await vendorApi.switch(
              target.selection.rowId,
              target.selection.planId,
              appId,
              quitChatgpt,
            )
          : await relayApi.switchTier(target.providerId, appId, quitChatgpt);
      if (generation !== lifecycle.current) return;
      if (result.status === "confirmationRequired") {
        setConfirmation({ target, name: result.targetName });
        return;
      }
      toast.success(
        t(
          result.chatgptRelaunched
            ? "loongport.switch.doneRelaunched"
            : result.chatgptWasRunning
              ? "loongport.switch.doneNeedsRestart"
              : "loongport.switch.done",
          { name: target.name },
        ),
      );
      result.warnings.forEach((warning) => toast.warning(warning));
      await Promise.all([
        client.invalidateQueries({ queryKey: key }),
        client.invalidateQueries({ queryKey: ["providers", appId] }),
      ]);
    } catch (error) {
      if (generation === lifecycle.current)
        toast.error(extractErrorMessage(error));
    } finally {
      pending.current = false;
      if (generation === lifecycle.current) setBusy(false);
    }
  };
  return {
    ...query,
    busy,
    select,
    confirmation: confirmation?.name ?? null,
    cancel: () => setConfirmation(null),
    confirm: (quit: boolean) =>
      confirmation && void select(confirmation.target, quit),
  };
}
