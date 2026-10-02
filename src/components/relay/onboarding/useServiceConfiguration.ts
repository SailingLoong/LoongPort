import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { PROVIDER_STORE_APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import { relayApi } from "@/lib/api/relay";
import { vendorApi } from "@/lib/api/vendor";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";
import { applicationOverviewApi } from "@/lib/api/applicationOverview";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  type ConnectedService,
  useServiceConfigurationChoices,
  useServiceOnboardingStatus,
  serviceOnboardingKey,
} from "./useServiceOnboarding";

export interface ConfigurationResult {
  id: string;
  state:
    | "pending"
    | "applying"
    | "success"
    | "failed"
    | "cancelled"
    | "changed"
    | "verificationFailed"
    | "unverified";
  revision?: string;
  error?: string;
}

export function useServiceConfiguration(
  account: ConnectedService,
  sourceAppId: AppId,
  onDone: () => void,
) {
  const client = useQueryClient();
  const { t } = useTranslation();
  const choices = useServiceConfigurationChoices(account);
  const status = useServiceOnboardingStatus();
  const [selection, setSelection] = useState<Partial<Record<AppId, string>>>(
    {},
  );
  const [shareData, setShareData] = useState(true);
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<
    Partial<Record<AppId, ConfigurationResult>>
  >({});
  const [completionError, setCompletionError] = useState<string | null>(null);
  const pending = useRef(false);
  const [confirmation, setConfirmation] = useState<string | null>(null);
  const confirmationResolver = useRef<((value: boolean | null) => void) | null>(
    null,
  );
  const mounted = useRef(true);
  const lifecycle = useRef(0);
  useEffect(() => {
    mounted.current = true;
    setBusy(pending.current);
    return () => {
      mounted.current = false;
      lifecycle.current += 1;
      confirmationResolver.current?.(null);
      confirmationResolver.current = null;
      setConfirmation(null);
      setBusy(false);
      setResults((previous) =>
        Object.fromEntries(
          Object.entries(previous).map(([app, result]) => [
            app,
            result?.state === "applying"
              ? { ...result, state: "unverified" }
              : result,
          ]),
        ),
      );
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
    if (pending.current || !choices.data || !status.data) return;
    const activeLifecycle = lifecycle.current;
    const isActive = () =>
      mounted.current && lifecycle.current === activeLifecycle;
    pending.current = true;
    setBusy(true);
    setCompletionError(null);
    const targets = PROVIDER_STORE_APP_IDS.flatMap((app) => {
      const id = results[app]?.id ?? selectFor(app);
      return id ? [{ app, id }] : [];
    });
    const update = (app: AppId, result: ConfigurationResult) => {
      if (isActive())
        setResults((previous) => ({ ...previous, [app]: result }));
    };
    const readState = async (app: AppId, id: string) => {
      const snapshot = await applicationOverviewApi.state(app);
      if (!snapshot.configurationRevision)
        throw new Error(t("loongport.onboarding.cannotVerify"));
      const target = snapshot.configurations.find(
        (item) => item.providerId === id,
      );
      const applied = snapshot.isAdditive
        ? target?.presentation.isInConfig
        : target?.presentation.isCurrent;
      return {
        revision: snapshot.configurationRevision,
        applied: Boolean(applied),
      };
    };
    try {
      // A process result is not a source fact. Recheck completed items before
      // any retry writes, using the backend's native-file revision receipt.
      let needsReview = false;
      for (const { app, id } of targets) {
        const previous = results[app];
        if (!previous) {
          update(app, { id, state: "pending" });
          continue;
        }
        if (previous.state === "changed" || previous.state === "unverified") {
          needsReview = true;
          continue;
        }
        if (!previous.revision) continue;
        try {
          const current = await readState(app, id);
          if (!isActive()) return;
          if (!current.applied || current.revision !== previous.revision) {
            update(app, { ...previous, state: "changed", error: undefined });
            needsReview = true;
          } else
            update(app, { ...previous, state: "success", error: undefined });
        } catch (error) {
          if (!isActive()) return;
          update(app, {
            ...previous,
            state: "verificationFailed",
            error: extractErrorMessage(error),
          });
          needsReview = true;
        }
      }
      if (!isActive() || needsReview) return;
      for (const { app, id } of targets) {
        if (!isActive()) return;
        if (results[app]?.revision) continue;
        update(app, { id, state: "applying" });
        const switchConfig = (quitChatgpt?: boolean) =>
          account.kind === "vendor"
            ? vendorApi.switch(account.rowId, id, app, quitChatgpt)
            : relayApi.switchTier(id, app, quitChatgpt);
        let result;
        try {
          result = await switchConfig();
        } catch (error) {
          update(app, {
            id,
            state: "failed",
            error: extractErrorMessage(error),
          });
          return;
        }
        if (!isActive()) return;
        if (result.status === "confirmationRequired") {
          setConfirmation(result.targetName);
          const answer = await new Promise<boolean | null>((resolve) => {
            confirmationResolver.current = resolve;
          });
          if (answer === null) {
            update(app, { id, state: "cancelled" });
            return;
          }
          if (!isActive()) return;
          try {
            result = await switchConfig(answer);
          } catch (error) {
            update(app, {
              id,
              state: "failed",
              error: extractErrorMessage(error),
            });
            return;
          }
        }
        if (!isActive()) return;
        if (result.status !== "switched") {
          update(app, { id, state: "cancelled" });
          return;
        }
        result.warnings.forEach((warning) => toast.warning(warning));
        // The switch already happened. If verification fails, do not pretend it
        // failed or repeat it automatically; require an explicit reapplication.
        update(app, { id, state: "unverified" });
        try {
          const current = await readState(app, id);
          if (!isActive()) return;
          if (!current.applied) {
            update(app, { id, state: "changed" });
            return;
          }
          update(app, { id, state: "success", revision: current.revision });
        } catch (error) {
          update(app, {
            id,
            state: "unverified",
            error: extractErrorMessage(error),
          });
          return;
        }
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
      if (isActive()) setCompletionError(extractErrorMessage(error));
    } finally {
      pending.current = false;
      if (mounted.current) setBusy(false);
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
    results,
    completionError,
    reapply: (app: AppId) => {
      if (pending.current) return;
      setResults((previous) =>
        previous[app]
          ? { ...previous, [app]: { id: previous[app]!.id, state: "pending" } }
          : previous,
      );
    },
    confirmation,
    selectFor,
    resolveConfirmation,
    finish,
  };
}
