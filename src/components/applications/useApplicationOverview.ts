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
import type {
  CatalogPreview,
  ProviderCatalogDecision,
} from "@/lib/api/providers";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
import { relayApi } from "@/lib/api/relay";
import {
  vendorApi,
  type VendorConnectionReview,
  type VendorPreparedReceipt,
  type VendorNewConnectionIntentReview,
} from "@/lib/api/vendor";
import { matchesVendorConnectionTarget } from "@/lib/vendorConnectionIdentity";
import { PROVIDER_SWITCHED, VENDOR_ACCOUNTS_CHANGED } from "@/lib/api/events";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { extractErrorMessage } from "@/utils/errorUtils";

type VendorRecoveryPrompt = {
  target: ApplicationConfiguration;
  review: VendorNewConnectionIntentReview;
};

export function useApplicationOverview(
  appId: AppId,
  providers: Record<string, Provider>,
  onSwitchProvider: (provider: Provider) => void | Promise<void>,
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
  // A pending IPC is never a reason to issue another write. Durable truth stays native.
  const vendorOperation = useRef<{ app: AppId; operationId: string } | null>(
    null,
  );
  const [confirmation, setConfirmationState] = useState<{
    target: ApplicationConfiguration;
    name: string;
    /** 中转档位要一并切过去的模型（模型筛选场景）；确认往返不丢。 */
    model?: string;
    catalog?: CatalogPreview;
    quitRequired?: boolean;
    vendor?: VendorPreparedReceipt;
    vendorReview?: VendorConnectionReview;
  } | null>(null);
  const confirmationRef = useRef<typeof confirmation>(null);
  const setConfirmation = (value: typeof confirmation) => {
    confirmationRef.current = value;
    setConfirmationState(value);
  };
  const [vendorRecovery, setVendorRecoveryState] =
    useState<VendorRecoveryPrompt | null>(null);
  const vendorRecoveryRef = useRef<VendorRecoveryPrompt | null>(null);
  const setVendorRecovery = (value: VendorRecoveryPrompt | null) => {
    vendorRecoveryRef.current = value;
    setVendorRecoveryState(value);
  };
  const lifecycle = useRef(0);
  const view = useRef({ app: appId });
  if (view.current.app !== appId) view.current = { app: appId };
  const renderedView = view.current;
  const active = useRef(false);
  useEffect(() => {
    active.current = true;
    setVendorRecovery(null);
    setConfirmation(null);
    setBusy(false);
    return () => {
      vendorRecoveryRef.current = null;
      confirmationRef.current = null;
      active.current = false;
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
    model?: string,
    catalogDecision?: ProviderCatalogDecision,
    catalogPreviewOnly = false,
    replacement?: VendorRecoveryPrompt,
  ) => {
    if (
      !active.current ||
      view.current !== renderedView ||
      !target.canSelect ||
      pending.current
    )
      return;
    if (
      replacement &&
      (replacement !== vendorRecoveryRef.current ||
        replacement.target !== target)
    )
      return;
    setVendorRecovery(null);
    const generation = lifecycle.current;
    pending.current = true;
    setBusy(true);
    try {
      if (target.selection.kind === "provider") {
        const provider = providers[target.providerId];
        if (provider) await onSwitchProvider(provider);
        return;
      }
      if (target.selection.kind === "vendor") {
        const identity = {
          app: appId,
          rowId: target.selection.rowId,
          plan: target.selection.planId,
          providerId: target.providerId,
        };
        const uncertain = () => {
          if (generation === lifecycle.current) {
            toast.error(t("loongport.onboarding.cannotVerify"));
          }
        };
        const prompt = confirmationRef.current;
        if (catalogPreviewOnly) {
          if (!prompt?.vendor || prompt.target !== target || !catalogDecision) {
            uncertain();
            return;
          }
          const preview = await vendorApi
            .previewConnectionCatalog({
              app: appId,
              operationId: prompt.vendor.operationId,
              catalogDecision,
              quitChatgpt: quitChatgpt ?? null,
            })
            .catch(() => ({ status: "unknown" as const }));
          if (
            generation !== lifecycle.current ||
            confirmationRef.current !== prompt
          )
            return;
          if (
            preview.status !== "confirmationRequired" ||
            preview.catalog?.choice !== catalogDecision.choice ||
            preview.catalog.revision !== catalogDecision.revision
          ) {
            uncertain();
            return;
          }
          const review = await vendorApi
            .previewConnectionReview({
              app: appId,
              operationId: prompt.vendor.operationId,
            })
            .catch(() => ({ status: "unknown" as const }));
          if (
            generation !== lifecycle.current ||
            confirmationRef.current !== prompt
          )
            return;
          if (
            review.status !== "reviewRequired" ||
            review.receiptRevision !== prompt.vendor.receiptRevision
          ) {
            uncertain();
            return;
          }
          setConfirmation({
            ...prompt,
            name: preview.targetName,
            catalog: preview.catalog,
            quitRequired: preview.quitRequired,
            vendorReview: review,
          });
          return;
        }
        if (catalogDecision && quitChatgpt === undefined) {
          uncertain();
          return;
        }
        let prepared;
        if (quitChatgpt !== undefined) {
          if (!prompt?.vendor || prompt.target !== target) {
            uncertain();
            return;
          }
          prepared = prompt.vendor;
          if (prompt.catalog) {
            if (
              !catalogDecision ||
              !prompt.vendorReview ||
              catalogDecision.choice !== prompt.catalog.choice ||
              catalogDecision.revision !== prompt.catalog.revision
            ) {
              uncertain();
              return;
            }
            setConfirmation(null);
            const retired = await vendorApi.reviewConnection({
              app: appId,
              operationId: prepared.operationId,
              expectedRevision: prompt.vendorReview.receiptRevision,
              observedRevision: prompt.vendorReview.observedRevision,
            });
            if (generation !== lifecycle.current) return;
            if (
              retired.status !== "failedBeforeWrite" ||
              !matchesVendorConnectionTarget(retired, {
                ...identity,
                operationId: prepared.operationId,
              })
            ) {
              uncertain();
              return;
            }
            const operationId = crypto.randomUUID();
            vendorOperation.current = { app: appId, operationId };
            const next = await vendorApi.prepareConnection({
              ...identity,
              operationId,
              catalogDecision,
            });
            if (generation !== lifecycle.current) return;
            if (
              next.status !== "prepared" ||
              !matchesVendorConnectionTarget(next, { ...identity, operationId })
            ) {
              uncertain();
              return;
            }
            prepared = next;
          } else {
            if (catalogDecision) {
              uncertain();
              return;
            }
            setConfirmation(null);
          }
        } else {
          if (replacement) {
            const admitted = await vendorApi.confirmNewConnectionIntent({
              app: appId,
              operationId: replacement.review.operationId,
              targetRowId: identity.rowId,
              targetPlan: identity.plan,
              reviewToken: replacement.review.reviewToken,
            });
            if (generation !== lifecycle.current) return;
            if (admitted.status !== "newIntentPermitted") {
              uncertain();
              return;
            }
          } else {
            const known =
              vendorOperation.current?.app === appId
                ? vendorOperation.current.operationId
                : null;
            const previous = await vendorApi.queryConnection(appId, known);
            if (generation !== lifecycle.current) return;
            if (
              previous.status === "completed" &&
              known !== null &&
              matchesVendorConnectionTarget(previous, {
                ...identity,
                operationId: known,
              })
            ) {
              if (previous.completedConfigurationRevision === null) {
                uncertain();
                return;
              }
              vendorOperation.current = null;
              setConfirmation(null);
              await client.invalidateQueries({ queryKey: key });
              return;
            }
            const retired =
              "operationState" in previous &&
              (previous.operationState === "failedBeforeWrite" ||
                previous.operationState === "reviewed");
            if (
              previous.status !== "none" &&
              previous.status !== "completed" &&
              !retired
            ) {
              const operationId =
                known ??
                ("operationId" in previous ? previous.operationId : undefined);
              if (!operationId) {
                uncertain();
                return;
              }
              const review = await vendorApi.previewNewConnectionIntent({
                app: appId,
                operationId,
                targetRowId: identity.rowId,
                targetPlan: identity.plan,
              });
              if (generation !== lifecycle.current) return;
              if (
                review.status !== "newIntentReviewRequired" ||
                review.targetProviderId !== identity.providerId
              ) {
                uncertain();
                return;
              }
              setVendorRecovery({ target, review });
              return;
            }
          }
          const operationId = crypto.randomUUID();
          vendorOperation.current = { app: appId, operationId };
          prepared = await vendorApi.prepareConnection({
            ...identity,
            operationId,
            catalogDecision: null,
          });
          if (generation !== lifecycle.current) return;
          if (
            prepared.status !== "prepared" ||
            !matchesVendorConnectionTarget(prepared, {
              ...identity,
              operationId,
            })
          ) {
            uncertain();
            return;
          }
        }
        const operationId = prepared.operationId;
        let feedback;
        try {
          feedback = await vendorApi.switchPreparedConnection({
            ...identity,
            operationId,
            expectedRevision: prepared.receiptRevision,
            quitChatgpt: quitChatgpt ?? null,
          });
        } catch {
          // A rejected transport may have written. Query the original operation.
        }
        if (feedback?.status === "confirmationRequired") {
          if (generation === lifecycle.current) {
            let vendorReview: VendorConnectionReview | undefined;
            if (feedback.catalog) {
              const review = await vendorApi
                .previewConnectionReview({ app: appId, operationId })
                .catch(() => ({ status: "unknown" as const }));
              if (generation !== lifecycle.current) return;
              if (
                review.status === "reviewRequired" &&
                review.receiptRevision === prepared.receiptRevision
              )
                vendorReview = review;
            }
            setConfirmation({
              target,
              name: feedback.targetName,
              catalog: feedback.catalog,
              quitRequired: feedback.quitRequired,
              vendor: prepared,
              vendorReview,
            });
          }
          return;
        }
        const completed = await vendorApi.queryConnection(appId, operationId);
        if (generation !== lifecycle.current) return;
        if (
          completed.status !== "completed" ||
          !matchesVendorConnectionTarget(completed, {
            ...identity,
            operationId,
          }) ||
          completed.completedConfigurationRevision === null
        ) {
          uncertain();
          return;
        }
        vendorOperation.current = null;
        setConfirmation(null);
        toast.success(t("loongport.switch.done", { name: target.name }));
        if (feedback?.status === "switched") {
          feedback.warnings.forEach((warning) => toast.warning(warning));
        }
        await Promise.all([
          client.invalidateQueries({ queryKey: key }),
          client.invalidateQueries({ queryKey: ["providers", appId] }),
        ]);
        return;
      }
      const result = catalogDecision
        ? await applicationRoutingApi.apply(
            appId,
            {
              selection: {
                providerId: target.providerId,
                ...(model ? { model } : {}),
              },
              catalogDecision,
              catalogPreviewOnly,
            },
            quitChatgpt,
          )
        : // 带模型 = 模型筛选场景：档位与模型一次切过去（后端校验目录成员，
          // 过期 UI 指向不支持的模型会得到明确报错，不会写坏配置）。
          model
          ? await relayApi.switchTierModel(
              target.providerId,
              appId,
              model,
              quitChatgpt,
            )
          : await relayApi.switchTier(target.providerId, appId, quitChatgpt);
      if (generation !== lifecycle.current) return;
      if (result.status === "confirmationRequired") {
        if (
          catalogPreviewOnly &&
          (result.catalog?.choice !== catalogDecision?.choice ||
            result.catalog?.revision !== catalogDecision?.revision)
        ) {
          setConfirmation(null);
          toast.error(t("provider.preview.sourceChanged"));
          return;
        }
        setConfirmation({
          target,
          name: result.targetName,
          model,
          catalog: result.catalog,
          quitRequired: result.quitRequired,
        });
        return;
      }
      setConfirmation(null);
      if (catalogPreviewOnly) return;
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
      if (generation === lifecycle.current) {
        setConfirmation(null);
        toast.error(extractErrorMessage(error));
      }
    } finally {
      pending.current = false;
      if (generation === lifecycle.current) setBusy(false);
    }
  };
  return {
    ...query,
    busy,
    select,
    vendorRecovery,
    cancelVendorRecovery: () => {
      if (
        !active.current ||
        view.current !== renderedView ||
        vendorRecovery !== vendorRecoveryRef.current
      )
        return;
      lifecycle.current += 1;
      setVendorRecovery(null);
      setBusy(false);
    },
    confirmVendorRecovery: async () => {
      if (!vendorRecovery || vendorRecovery !== vendorRecoveryRef.current)
        return;
      await select(
        vendorRecovery.target,
        undefined,
        undefined,
        undefined,
        false,
        vendorRecovery,
      );
    },
    confirmation: confirmation?.name ?? null,
    catalog: confirmation?.catalog,
    quitRequired: confirmation?.quitRequired,
    cancel: () => {
      if (
        !active.current ||
        view.current !== renderedView ||
        confirmation !== confirmationRef.current
      )
        return;
      lifecycle.current += 1;
      setConfirmation(null);
      setBusy(false);
    },
    previewCatalog: async (decision: ProviderCatalogDecision) => {
      if (
        confirmation !== confirmationRef.current ||
        !confirmation?.catalog ||
        decision.revision !== confirmation.catalog.revision
      )
        return;
      await select(
        confirmation.target,
        undefined,
        confirmation.model,
        decision,
        true,
      );
    },
    confirm: (quit: boolean, decision?: ProviderCatalogDecision) => {
      if (!confirmation || confirmation !== confirmationRef.current) return;
      if (
        confirmation.catalog &&
        (decision?.choice !== confirmation.catalog.choice ||
          decision.revision !== confirmation.catalog.revision)
      )
        return;
      void select(confirmation.target, quit, confirmation.model, decision);
    },
  };
}
