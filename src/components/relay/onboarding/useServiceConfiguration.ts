import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { PROVIDER_STORE_APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import type { SwitchTierCommandResult } from "@/lib/api/relay";
import type { ProviderCatalogDecision } from "@/lib/api/providers";
import {
  vendorApi,
  type VendorNewConnectionIntentReview,
} from "@/lib/api/vendor";
import { matchesVendorConnectionTarget } from "@/lib/vendorConnectionIdentity";
import { credentialProtectionKey } from "@/lib/api/credentialProtection";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";
import { applicationOverviewApi } from "@/lib/api/applicationOverview";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  type ConnectedService,
  useVendorConfigurationChoices,
  useServiceOnboardingStatus,
  serviceOnboardingKey,
} from "./useServiceOnboarding";

type ConfirmationAction = {
  quitChatgpt?: boolean;
  catalogDecision?: ProviderCatalogDecision;
  catalogPreviewOnly: boolean;
};

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

export function useVendorServiceConfiguration(
  account: Pick<ConnectedService, "rowId" | "name">,
  sourceAppId: AppId,
  onDone: () => void,
) {
  const client = useQueryClient();
  const { t } = useTranslation();
  const choices = useVendorConfigurationChoices(account);
  const status = useServiceOnboardingStatus();
  const [selection, setSelection] = useState<Partial<Record<AppId, string>>>(
    {},
  );
  const [shareData, setShareData] = useState(true);
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<
    Partial<Record<AppId, ConfigurationResult>>
  >({});
  // Synchronous owner for commands; React state is its render projection. A
  // retained callback must observe outcomes even before a render commits.
  const resultsOwner = useRef<Partial<Record<AppId, ConfigurationResult>>>({});
  const setResultProjection = (
    next: Partial<Record<AppId, ConfigurationResult>>,
  ) => {
    resultsOwner.current = next;
    setResults(next);
  };
  const [completionError, setCompletionError] = useState<string | null>(null);
  const pending = useRef(false);
  type PendingReview = {
    app: AppId;
    id: string;
    providerId: string;
    proof: VendorNewConnectionIntentReview | null;
    observedRevision?: string;
  };
  const [pendingReview, setPendingReview] = useState<PendingReview | null>(
    null,
  );
  const reviewOwner = useRef<PendingReview | null>(null);
  const newIntentPermits = useRef<
    Partial<Record<AppId, { rowId: number; plan: string; providerId: string }>>
  >({});
  const clearReview = () => {
    reviewOwner.current = null;
    setPendingReview(null);
  };

  // Per-app native operation identity survives Activity suspension. A remount
  // recovers through the backend's app-scoped query rather than replaying.
  const operationsOwner = useRef<
    Partial<
      Record<
        AppId,
        {
          id: string;
          rowId: number;
          plan: string;
          bound: boolean;
          observed?: { rowId: number; plan: string; providerId: string };
        }
      >
    >
  >({});
  const [confirmation, setConfirmation] = useState<{
    value: Extract<SwitchTierCommandResult, { status: "confirmationRequired" }>;
  } | null>(null);
  const confirmationOwner = useRef<typeof confirmation>(null);
  const confirmationResolver = useRef<
    ((value: ConfirmationAction | null) => void) | null
  >(null);
  const mounted = useRef(true);
  const lifecycle = useRef(0);
  const [renderLifecycle, setRenderLifecycle] = useState(0);
  const [renderOwnerVersion, setRenderOwnerVersion] = useState(0);
  // Ordered choice identity matters: the first source-app choice is the default.
  const choicesKey = JSON.stringify(
    choices.data?.map(({ app, id, providerId }) => [app, id, providerId]) ??
      null,
  );
  const committedOwner = useRef({
    rowId: account.rowId,
    sourceAppId,
    selection,
    shareData,
    choicesKey,
    version: 0,
  });
  useLayoutEffect(() => {
    const previous = committedOwner.current;
    if (
      previous.rowId !== account.rowId ||
      previous.sourceAppId !== sourceAppId ||
      previous.selection !== selection ||
      previous.shareData !== shareData ||
      previous.choicesKey !== choicesKey
    ) {
      clearReview();
      newIntentPermits.current = {};
      confirmationResolver.current?.(null);
      confirmationResolver.current = null;
      confirmationOwner.current = null;
      setConfirmation(null);
      const version = previous.version + 1;
      committedOwner.current = {
        rowId: account.rowId,
        sourceAppId,
        selection,
        shareData,
        choicesKey,
        version,
      };
      setRenderOwnerVersion(version);
      // Results describe this account, not merely an app/configuration revision.
      // Keep native operation IDs for reconciliation, but never project a prior
      // account's success onto the newly selected account.
      setResultProjection(
        previous.rowId !== account.rowId
          ? {}
          : Object.fromEntries(
              Object.entries(resultsOwner.current).map(([app, result]) => [
                app,
                result?.state === "applying"
                  ? { ...result, state: "unverified" }
                  : result,
              ]),
            ),
      );
    }
  }, [account.rowId, sourceAppId, selection, shareData, choicesKey]);
  const isCurrentOwner = () =>
    mounted.current &&
    renderLifecycle === lifecycle.current &&
    renderOwnerVersion === committedOwner.current.version &&
    committedOwner.current.rowId === account.rowId &&
    committedOwner.current.sourceAppId === sourceAppId &&
    committedOwner.current.selection === selection &&
    committedOwner.current.shareData === shareData &&
    committedOwner.current.choicesKey === choicesKey;
  useEffect(() => {
    mounted.current = true;
    setRenderLifecycle(lifecycle.current);
    setBusy(pending.current);
    return () => {
      mounted.current = false;
      lifecycle.current += 1;
      clearReview();
      newIntentPermits.current = {};
      confirmationResolver.current?.(null);
      confirmationResolver.current = null;
      confirmationOwner.current = null;
      setConfirmation(null);
      setBusy(false);
      setResultProjection(
        Object.fromEntries(
          Object.entries(resultsOwner.current).map(([app, result]) => [
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
  const resolveConfirmation = (
    value: boolean | null | undefined,
    catalogDecision?: ProviderCatalogDecision,
    catalogPreviewOnly = false,
  ) => {
    if (
      !isCurrentOwner() ||
      !confirmation ||
      confirmation !== confirmationOwner.current
    )
      return;
    if (value !== null && !catalogPreviewOnly) {
      const displayed = confirmation.value.catalog;
      if (
        (displayed &&
          (!catalogDecision ||
            catalogDecision.choice !== displayed.choice ||
            catalogDecision.revision !== displayed.revision)) ||
        (!displayed && catalogDecision)
      ) {
        setCompletionError(t("provider.preview.sourceChanged"));
        return;
      }
    }
    confirmationResolver.current?.(
      value === null
        ? null
        : { quitChatgpt: value, catalogDecision, catalogPreviewOnly },
    );
    confirmationResolver.current = null;
    confirmationOwner.current = null;
    setConfirmation(null);
  };
  const finish = async (onlyApp?: AppId) => {
    if (!isCurrentOwner() || pending.current || !choices.data || !status.data)
      return;
    const activeLifecycle = lifecycle.current;
    const isActive = () =>
      isCurrentOwner() && lifecycle.current === activeLifecycle;
    pending.current = true;
    setBusy(true);
    setCompletionError(null);
    const targets = PROVIDER_STORE_APP_IDS.flatMap((app) => {
      const id = resultsOwner.current[app]?.id ?? selectFor(app);
      return id ? [{ app, id }] : [];
    });
    const update = (app: AppId, result: ConfigurationResult) => {
      if (isActive())
        setResultProjection({ ...resultsOwner.current, [app]: result });
    };
    const readState = async (app: AppId, id: string) => {
      const snapshot = await applicationOverviewApi.state(app);
      if (!snapshot.configurationRevision)
        throw new Error(t("loongport.onboarding.cannotVerify"));
      const target = snapshot.configurations.find(
        (item) =>
          item.selection.kind === "vendor" &&
          item.selection.rowId === account.rowId &&
          item.selection.planId === id,
      );
      const applied = snapshot.isAdditive
        ? target?.presentation.isInConfig
        : target?.presentation.isCurrent;
      return {
        revision: snapshot.configurationRevision,
        applied: Boolean(applied),
      };
    };
    // Native receipt identity, not a response or a current-looking snapshot,
    // owns whether an earlier operation may be treated as completed.
    const reconcile = async (
      app: AppId,
      id: string,
      operationId: string | null,
      allowDifferentCompleted = false,
    ) => {
      let receipt;
      try {
        receipt = await vendorApi.queryConnection(app, operationId);
      } catch (error) {
        update(app, {
          id,
          state: "unverified",
          error: extractErrorMessage(error),
        });
        return "blocked" as const;
      }
      if (!isActive()) return "blocked" as const;
      if (receipt.status === "none" && operationId === null)
        return "none" as const;
      const previousOperation = operationsOwner.current[app];
      // A native operation's identity is immutable. A contradictory full
      // receipt cannot replace an earlier identity, even if the ID matches.
      if (
        operationId !== null &&
        previousOperation?.id === operationId &&
        previousOperation.observed &&
        "rowId" in receipt &&
        !matchesVendorConnectionTarget(receipt, {
          app,
          operationId,
          ...previousOperation.observed,
        })
      ) {
        update(app, { id, state: "unverified" });
        return "blocked" as const;
      }
      const providerId = choices.data!.find(
        (choice) => choice.app === app && choice.id === id,
      )?.providerId;
      const identityMatches =
        Boolean(providerId) &&
        matchesVendorConnectionTarget(receipt, {
          app,
          rowId: account.rowId,
          plan: id,
          ...(operationId === null ? {} : { operationId }),
          providerId,
        });
      // An app-scoped terminal receipt for a different target is not success
      // for this target. An explicit new intent may proceed to native prepare;
      // the backend rechecks the single-unresolved-per-app invariant there.
      if (
        receipt.status === "completed" &&
        allowDifferentCompleted &&
        (receipt.rowId !== account.rowId ||
          receipt.plan !== id ||
          (providerId && receipt.providerId !== providerId)) &&
        matchesVendorConnectionTarget(receipt, {
          app,
          rowId: receipt.rowId,
          plan: receipt.plan,
          ...(operationId === null ? {} : { operationId }),
        })
      )
        return "priorCompleted" as const;
      // Any unresolved ID is only a lookup hint until the full target matches.
      if (
        receipt.status === "unknown" &&
        typeof receipt.operationId === "string" &&
        receipt.operationId.length > 0 &&
        (receipt.app === undefined || receipt.app === app) &&
        (operationId === null || receipt.operationId === operationId)
      ) {
        operationsOwner.current[app] = {
          id: receipt.operationId,
          rowId: account.rowId,
          plan: id,
          // Redaction cannot erase an existing binding of this same operation.
          bound:
            operationsOwner.current[app]?.id === receipt.operationId &&
            operationsOwner.current[app]?.bound === true,
          observed:
            operationsOwner.current[app]?.id === receipt.operationId
              ? operationsOwner.current[app]?.observed
              : undefined,
        };
        if (
          "rowId" in receipt &&
          "plan" in receipt &&
          "providerId" in receipt &&
          typeof receipt.rowId === "number" &&
          typeof receipt.plan === "string" &&
          typeof receipt.providerId === "string" &&
          matchesVendorConnectionTarget(receipt, {
            app,
            rowId: receipt.rowId,
            plan: receipt.plan,
          })
        ) {
          operationsOwner.current[app]!.observed = {
            rowId: receipt.rowId,
            plan: receipt.plan,
            providerId: receipt.providerId,
          };
        }
      }
      if (receipt.status === "completed" && identityMatches) {
        operationsOwner.current[app] = {
          id: receipt.operationId,
          rowId: account.rowId,
          plan: id,
          bound: true,
          observed: {
            rowId: receipt.rowId,
            plan: receipt.plan,
            providerId: receipt.providerId,
          },
        };
      }
      if (
        receipt.status === "unknown" &&
        identityMatches &&
        typeof providerId === "string" &&
        typeof receipt.operationId === "string"
      ) {
        operationsOwner.current[app] = {
          id: receipt.operationId,
          rowId: account.rowId,
          plan: id,
          bound: true,
          observed: {
            rowId: account.rowId,
            plan: id,
            providerId,
          },
        };
      }
      if (
        receipt.status !== "completed" ||
        !identityMatches ||
        !receipt.completedConfigurationRevision
      ) {
        update(app, { id, state: "unverified" });
        return "blocked" as const;
      }
      operationsOwner.current[app] = {
        id: receipt.operationId,
        rowId: account.rowId,
        plan: id,
        bound: true,
        observed: {
          rowId: receipt.rowId,
          plan: receipt.plan,
          providerId: receipt.providerId,
        },
      };
      try {
        const current = await readState(app, id);
        if (!isActive()) return "blocked" as const;
        if (
          !current.applied ||
          current.revision !== receipt.completedConfigurationRevision
        ) {
          update(app, {
            id,
            state: "changed",
            revision: receipt.completedConfigurationRevision,
          });
          return "blocked" as const;
        }
        update(app, {
          id,
          state: "success",
          revision: receipt.completedConfigurationRevision,
        });
        return "completed" as const;
      } catch (error) {
        update(app, {
          id,
          state: "verificationFailed",
          error: extractErrorMessage(error),
        });
        return "blocked" as const;
      }
    };
    try {
      // Recheck every completed target and all unresolved operations before
      // any new write, so a partial result cannot silently replay another app.
      const blockedApps = new Set<AppId>();
      let recoveredUnresolved = false;
      // Query other apps represented by backend choices without treating their
      // unresolved writes as a prohibition on an independent selected app.
      for (const app of new Set(choices.data.map((choice) => choice.app))) {
        if (targets.some((target) => target.app === app)) continue;
        let recovered;
        try {
          recovered = await vendorApi.queryConnection(app, null);
        } catch (error) {
          if (isActive()) setCompletionError(extractErrorMessage(error));
          continue;
        }
        if (!isActive()) return;
        if (recovered.status !== "unknown") continue;
        if ("rowId" in recovered && recovered.rowId !== account.rowId) continue;
        if (
          "plan" in recovered &&
          typeof recovered.plan === "string" &&
          choices.data.some(
            (choice) => choice.app === app && choice.id === recovered.plan,
          ) &&
          matchesVendorConnectionTarget(recovered, {
            app,
            rowId: account.rowId,
            plan: recovered.plan,
          })
        ) {
          recoveredUnresolved = true;
          operationsOwner.current[app] = {
            id: recovered.operationId,
            rowId: account.rowId,
            plan: recovered.plan,
            bound: true,
            observed: {
              rowId: recovered.rowId,
              plan: recovered.plan,
              providerId: recovered.providerId,
            },
          };
          update(app, { id: recovered.plan, state: "unverified" });
        } else {
          setCompletionError(t("loongport.onboarding.cannotVerify"));
        }
      }
      for (const { app, id } of targets) {
        const permit = newIntentPermits.current[app];
        if (
          permit &&
          permit.rowId === account.rowId &&
          permit.plan === id &&
          choices.data.some(
            (choice) =>
              choice.app === app &&
              choice.id === id &&
              choice.providerId === permit.providerId,
          )
        ) {
          update(app, { id, state: "pending" });
          continue;
        }
        const previous = resultsOwner.current[app];
        if (previous?.state === "changed") {
          blockedApps.add(app);
          continue;
        }
        if (previous?.revision && previous.state === "success") {
          try {
            const current = await readState(app, id);
            if (!isActive()) return;
            if (!current.applied || current.revision !== previous.revision) {
              update(app, { ...previous, state: "changed", error: undefined });
              blockedApps.add(app);
            }
          } catch (error) {
            update(app, {
              ...previous,
              state: "verificationFailed",
              error: extractErrorMessage(error),
            });
            blockedApps.add(app);
          }
          continue;
        }
        const operation = operationsOwner.current[app];
        if (
          operation &&
          (operation.rowId !== account.rowId || operation.plan !== id)
        ) {
          update(app, { id, state: "unverified" });
          blockedApps.add(app);
          continue;
        }
        const outcome = await reconcile(
          app,
          id,
          operation?.id ?? null,
          (!previous && !operation) || operation?.bound === false,
        );
        if (!isActive()) return;
        if (outcome === "blocked") blockedApps.add(app);
        else if (outcome === "none" || outcome === "priorCompleted") {
          if (outcome === "priorCompleted") delete operationsOwner.current[app];
          update(app, { id, state: "pending" });
        }
      }
      if (!isActive()) return;
      for (const { app, id } of targets) {
        if (onlyApp && app !== onlyApp) continue;
        if (!isActive()) return;
        if (
          blockedApps.has(app) ||
          resultsOwner.current[app]?.state === "success"
        )
          continue;
        const providerId = choices.data.find(
          (choice) => choice.app === app && choice.id === id,
        )?.providerId;
        if (!providerId) {
          update(app, { id, state: "unverified" });
          blockedApps.add(app);
          continue;
        }
        delete newIntentPermits.current[app];
        let operationId = crypto.randomUUID();
        operationsOwner.current[app] = {
          id: operationId,
          rowId: account.rowId,
          plan: id,
          bound: true,
          observed: { rowId: account.rowId, plan: id, providerId },
        };
        update(app, { id, state: "applying" });
        let prepared;
        try {
          prepared = await vendorApi.prepareConnection({
            rowId: account.rowId,
            plan: id,
            app,
            operationId,
            catalogDecision: null,
          });
        } catch {
          // Never repeat prepare after a lost reply; inspect its original ID.
          const outcome = await reconcile(app, id, operationId);
          if (!isActive()) return;
          if (outcome !== "completed") blockedApps.add(app);
          continue;
        }
        if (!isActive()) return;
        if (
          prepared.status !== "prepared" ||
          !providerId ||
          !matchesVendorConnectionTarget(prepared, {
            app,
            rowId: account.rowId,
            plan: id,
            operationId,
            providerId,
          })
        ) {
          const outcome = await reconcile(app, id, operationId);
          if (!isActive()) return;
          if (outcome !== "completed") blockedApps.add(app);
          continue;
        }
        let preparedDecision: ProviderCatalogDecision | null = null;
        let switchResult: unknown;
        let switchError: string | undefined;
        try {
          switchResult = await vendorApi.switchPreparedConnection({
            rowId: account.rowId,
            plan: id,
            app,
            quitChatgpt: null,
            operationId,
            expectedRevision: prepared.receiptRevision,
          });
        } catch (error) {
          // Keep useful feedback without turning a lost reply into failed-write proof.
          switchError = extractErrorMessage(error);
        }
        if (!isActive()) return;
        while (
          switchResult &&
          typeof switchResult === "object" &&
          "status" in switchResult &&
          switchResult.status === "confirmationRequired" &&
          "targetName" in switchResult &&
          typeof switchResult.targetName === "string" &&
          "quitRequired" in switchResult &&
          typeof switchResult.quitRequired === "boolean"
        ) {
          let review: Awaited<
            ReturnType<typeof vendorApi.previewConnectionReview>
          > | null = null;
          if ("catalog" in switchResult && switchResult.catalog) {
            try {
              review = await vendorApi.previewConnectionReview({
                app,
                operationId,
              });
            } catch (error) {
              update(app, { id, state: "unverified" });
              if (isActive()) setCompletionError(extractErrorMessage(error));
              return;
            }
          }
          if (!isActive()) return;
          if (review && review.status !== "reviewRequired") {
            update(app, { id, state: "unverified" });
            setCompletionError(t("loongport.onboarding.cannotVerify"));
            return;
          }
          const prompt = {
            value: switchResult as Extract<
              SwitchTierCommandResult,
              { status: "confirmationRequired" }
            >,
          };
          confirmationOwner.current = prompt;
          setConfirmation(prompt);
          const answer = await new Promise<ConfirmationAction | null>(
            (resolve) => {
              confirmationResolver.current = resolve;
            },
          );
          if (!isActive()) return;
          if (answer === null) {
            update(app, { id, state: "cancelled" });
            return;
          }
          if (answer.catalogPreviewOnly && answer.catalogDecision) {
            try {
              const preview = await vendorApi.previewConnectionCatalog({
                app,
                operationId,
                catalogDecision: answer.catalogDecision,
                quitChatgpt: answer.quitChatgpt ?? null,
              });
              if (!isActive()) return;
              if (preview.status === "confirmationRequired")
                switchResult = preview;
              else setCompletionError(t("loongport.onboarding.cannotVerify"));
            } catch (error) {
              if (!isActive()) return;
              setCompletionError(extractErrorMessage(error));
            }
            // A read-only preview failure retains the prior dialog for retry.
            continue;
          }
          if (
            answer.catalogDecision &&
            JSON.stringify(answer.catalogDecision) !==
              JSON.stringify(preparedDecision)
          ) {
            if (!review || review.status !== "reviewRequired") {
              update(app, { id, state: "unverified" });
              return;
            }
            try {
              const retired = await vendorApi.reviewConnection({
                app,
                operationId,
                expectedRevision: review.receiptRevision,
                observedRevision: review.observedRevision,
              });
              if (!isActive()) return;
              if (
                (retired.status !== "failedBeforeWrite" &&
                  retired.status !== "reviewed") ||
                retired.app !== app ||
                retired.operationId !== operationId ||
                retired.rowId !== account.rowId ||
                retired.plan !== id ||
                retired.providerId !== providerId
              ) {
                update(app, { id, state: "unverified" });
                return;
              }
              operationId = crypto.randomUUID();
              operationsOwner.current[app] = {
                id: operationId,
                rowId: account.rowId,
                plan: id,
                bound: true,
                observed: { rowId: account.rowId, plan: id, providerId },
              };
              preparedDecision = answer.catalogDecision;
              prepared = await vendorApi.prepareConnection({
                app,
                operationId,
                rowId: account.rowId,
                plan: id,
                catalogDecision: preparedDecision,
              });
              if (!isActive()) return;
              if (
                prepared.status !== "prepared" ||
                !matchesVendorConnectionTarget(prepared, {
                  app,
                  operationId,
                  rowId: account.rowId,
                  plan: id,
                  providerId,
                })
              ) {
                await reconcile(app, id, operationId);
                return;
              }
            } catch (error) {
              if (!isActive()) return;
              await reconcile(app, id, operationId);
              if (isActive()) setCompletionError(extractErrorMessage(error));
              return;
            }
          }
          try {
            switchResult = await vendorApi.switchPreparedConnection({
              rowId: account.rowId,
              plan: id,
              app,
              quitChatgpt: answer.quitChatgpt ?? null,
              operationId,
              expectedRevision: prepared.receiptRevision,
            });
          } catch (error) {
            // The continuation may have written before losing its response.
            switchError = extractErrorMessage(error);
            switchResult = undefined;
          }
          if (!isActive()) return;
        }
        if (
          switchResult &&
          typeof switchResult === "object" &&
          "status" in switchResult &&
          switchResult.status === "switched" &&
          "warnings" in switchResult &&
          Array.isArray(switchResult.warnings)
        ) {
          switchResult.warnings.forEach((warning: unknown) => {
            if (typeof warning === "string") toast.warning(warning);
          });
        }
        const outcome = await reconcile(app, id, operationId);
        if (!isActive()) return;
        if (outcome !== "completed") {
          blockedApps.add(app);
          if (switchError && resultsOwner.current[app])
            update(app, { ...resultsOwner.current[app]!, error: switchError });
        }
      }
      if (
        !isActive() ||
        recoveredUnresolved ||
        blockedApps.size > 0 ||
        targets.some(
          ({ app }) => resultsOwner.current[app]?.state !== "success",
        )
      )
        return;
      if (!status.data.completed)
        await serviceOnboardingApi.complete(shareData);
      await Promise.all([
        client.invalidateQueries({ queryKey: serviceOnboardingKey }),
        client.invalidateQueries({ queryKey: credentialProtectionKey }),
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
  const reapply = async (app: AppId) => {
    if (!isCurrentOwner() || pending.current) return;
    const shown = results[app];
    const operation = operationsOwner.current[app];
    const providerId = choices.data?.find(
      (choice) => choice.app === app && choice.id === shown?.id,
    )?.providerId;
    if (
      !shown ||
      shown !== resultsOwner.current[app] ||
      !operation ||
      !providerId
    ) {
      setCompletionError(t("loongport.onboarding.cannotVerify"));
      return;
    }
    pending.current = true;
    setBusy(true);
    clearReview();
    try {
      const currentReceipt =
        shown.state === "unverified"
          ? await vendorApi.queryConnection(app, operation.id)
          : null;
      if (!isCurrentOwner() || resultsOwner.current[app] !== shown) return;
      const retired =
        currentReceipt &&
        "operationState" in currentReceipt &&
        ["reviewed", "failedBeforeWrite"].includes(
          currentReceipt.operationState,
        ) &&
        matchesVendorConnectionTarget(currentReceipt, {
          app,
          operationId: operation.id,
          rowId: account.rowId,
          plan: shown.id,
          providerId,
        });
      if (shown.state === "changed" || retired) {
        const snapshot = await applicationOverviewApi.state(app);
        if (!isCurrentOwner() || resultsOwner.current[app] !== shown) return;
        if (!snapshot.configurationRevision) {
          setCompletionError(t("loongport.onboarding.cannotVerify"));
          return;
        }
        const next = {
          app,
          id: shown.id,
          providerId,
          proof: null,
          observedRevision: snapshot.configurationRevision,
        };
        reviewOwner.current = next;
        setPendingReview(next);
        return;
      }
      const proof = await vendorApi.previewNewConnectionIntent({
        app,
        operationId: operation.id,
        targetRowId: account.rowId,
        targetPlan: shown.id,
      });
      if (!isCurrentOwner() || resultsOwner.current[app] !== shown) return;
      if (
        proof.status !== "newIntentReviewRequired" ||
        proof.targetProviderId !== providerId
      ) {
        setCompletionError(t("loongport.onboarding.cannotVerify"));
        return;
      }
      const next = { app, id: shown.id, providerId, proof };
      reviewOwner.current = next;
      setPendingReview(next);
    } catch (error) {
      if (isCurrentOwner()) setCompletionError(extractErrorMessage(error));
    } finally {
      pending.current = false;
      if (mounted.current) setBusy(false);
    }
  };
  const confirmReview = async () => {
    const shown = pendingReview;
    if (
      !isCurrentOwner() ||
      pending.current ||
      !shown ||
      shown !== reviewOwner.current
    )
      return;
    pending.current = true;
    setBusy(true);
    let permitted = false;
    try {
      if (shown.proof) {
        const result = await vendorApi.confirmNewConnectionIntent({
          app: shown.app,
          operationId: shown.proof.operationId,
          targetRowId: account.rowId,
          targetPlan: shown.id,
          reviewToken: shown.proof.reviewToken,
        });
        if (!isCurrentOwner() || shown !== reviewOwner.current) return;
        if (
          result.status !== "newIntentPermitted" ||
          result.app !== shown.app
        ) {
          setCompletionError(t("loongport.onboarding.cannotVerify"));
          return;
        }
      } else {
        const snapshot = await applicationOverviewApi.state(shown.app);
        if (!isCurrentOwner() || shown !== reviewOwner.current) return;
        if (
          !shown.observedRevision ||
          snapshot.configurationRevision !== shown.observedRevision
        ) {
          clearReview();
          setCompletionError(t("provider.preview.sourceChanged"));
          return;
        }
      }
      delete operationsOwner.current[shown.app];
      newIntentPermits.current[shown.app] = {
        rowId: account.rowId,
        plan: shown.id,
        providerId: shown.providerId,
      };
      setResultProjection({
        ...resultsOwner.current,
        [shown.app]: { id: shown.id, state: "pending" },
      });
      clearReview();
      permitted = true;
    } catch (error) {
      if (isCurrentOwner()) setCompletionError(extractErrorMessage(error));
    } finally {
      pending.current = false;
      if (mounted.current) setBusy(false);
    }
    if (permitted && isCurrentOwner()) await finish(shown.app);
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
    reapply,
    pendingReview,
    confirmReview,
    cancelReview: () => {
      if (isCurrentOwner() && pendingReview === reviewOwner.current)
        clearReview();
    },
    confirmation: confirmation?.value.targetName ?? null,
    catalog: confirmation?.value.catalog,
    quitRequired: confirmation?.value.quitRequired,
    selectFor,
    resolveConfirmation,
    finish: () => finish(),
  };
}
