import {
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { ApplicationRoutingChange } from "@/lib/api/applicationRouting";
import type {
  CatalogPreview,
  ProviderCatalogDecision,
} from "@/lib/api/providers";
import type { SwitchTierCommandResult } from "@/lib/api/relay";
import type { TierSort } from "./tierMetrics";

/** Local editing intent. Applied configuration and routing remain backend facts. */
export function useApplicationRoutingDraft(
  apply: (
    change: ApplicationRoutingChange,
    quitChatgpt?: boolean,
  ) => Promise<SwitchTierCommandResult>,
) {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState<TierSort | null>(null);
  const [accountFilter, setAccountFilter] = useState<string | null>(null);
  const [modelFilter, setModelFilter] = useState<string | null>(null);
  const [stagedIds, setStagedIds] = useState<string[] | null>(null);
  const [profileName, setProfileName] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [confirmation, setConfirmation] = useState<{
    name: string;
    change: ApplicationRoutingChange;
    isCurrent?: () => boolean;
    catalog?: CatalogPreview;
    quitRequired?: boolean;
  } | null>(null);
  const intent = useRef(0);
  const submittedIntent = useRef<number | null>(null);
  const edit =
    <T>(setter: Dispatch<SetStateAction<T>>) =>
    (value: SetStateAction<T>) => {
      intent.current += 1;
      setter(value);
    };
  const pending = useRef(false);
  const lifecycle = useRef(0);
  useEffect(
    () => () => {
      lifecycle.current += 1;
    },
    [],
  );

  const discard = () => {
    intent.current += 1;
    setStagedIds(null);
    setProfileName(null);
    setShowAll(false);
    setSort(null);
    setAccountFilter(null);
    setModelFilter(null);
    setSearch("");
  };
  const discardSubmitted = () => {
    if (submittedIntent.current === intent.current) discard();
    submittedIntent.current = null;
  };
  const submit = async (
    change: ApplicationRoutingChange,
    quitChatgpt?: boolean,
    isCurrent?: () => boolean,
  ) => {
    if (pending.current) return;
    if (isCurrent && !isCurrent()) {
      setConfirmation(null);
      toast.warning(t("applications.reviewChanged"));
      return;
    }
    const generation = lifecycle.current;
    submittedIntent.current = intent.current;
    pending.current = true;
    setSubmitting(true);
    try {
      const result = await apply(change, quitChatgpt);
      if (generation !== lifecycle.current) return;
      if (result.status === "confirmationRequired") {
        if (
          change.catalogPreviewOnly &&
          (result.catalog?.choice !== change.catalogDecision?.choice ||
            result.catalog?.revision !== change.catalogDecision?.revision)
        ) {
          setConfirmation(null);
          toast.warning(t("provider.preview.sourceChanged"));
          return;
        }
        setConfirmation({
          name: result.targetName,
          change,
          isCurrent,
          catalog: result.catalog,
          quitRequired: result.quitRequired,
        });
        return;
      }
      setConfirmation(null);
      if (change.catalogPreviewOnly) return;
      if (change.order) discardSubmitted();
      toast.success(t("applications.changesApplied"));
      result.warnings.forEach((warning) => toast.warning(warning));
      if (result.chatgptWasRunning && !result.chatgptRelaunched) {
        toast.info(
          t("loongport.switch.doneNeedsRestart", { name: result.providerName }),
        );
      }
    } catch {
      if (generation === lifecycle.current) setConfirmation(null);
      // The mutation reports the original error. Keep local intent for retry.
    } finally {
      pending.current = false;
      if (generation === lifecycle.current) setSubmitting(false);
    }
  };
  return {
    search,
    setSearch: edit(setSearch),
    sort,
    setSort: edit(setSort),
    accountFilter,
    setAccountFilter: edit(setAccountFilter),
    modelFilter,
    setModelFilter: edit(setModelFilter),
    stagedIds,
    setStagedIds: edit(setStagedIds),
    profileName,
    setProfileName: edit(setProfileName),
    showAll,
    setShowAll: edit(setShowAll),
    submitting,
    confirmation,
    submit,
    discard,
    discardSubmitted,
    cancel: () => {
      lifecycle.current += 1;
      setConfirmation(null);
      setSubmitting(false);
    },
    previewCatalog: async (decision: ProviderCatalogDecision) => {
      if (
        !confirmation?.catalog ||
        decision.revision !== confirmation.catalog.revision
      )
        return;
      await submit(
        {
          ...confirmation.change,
          catalogDecision: decision,
          catalogPreviewOnly: true,
        },
        undefined,
        confirmation.isCurrent,
      );
    },
    confirm: (quit: boolean, decision?: ProviderCatalogDecision) => {
      if (!confirmation) return;
      if (
        confirmation.catalog &&
        (decision?.choice !== confirmation.catalog.choice ||
          decision.revision !== confirmation.catalog.revision)
      )
        return;
      void submit(
        decision
          ? {
              ...confirmation.change,
              catalogDecision: decision,
              catalogPreviewOnly: false,
            }
          : confirmation.change,
        quit,
        confirmation.isCurrent,
      );
    },
    saveAs: (name: string, ids: string[]) => {
      intent.current += 1;
      setProfileName(name);
      setStagedIds(ids);
      setSort(null);
      setShowAll(false);
    },
    load: (name: string, ids: string[]) => {
      discard();
      setProfileName(name);
      setStagedIds(ids);
    },
  };
}
