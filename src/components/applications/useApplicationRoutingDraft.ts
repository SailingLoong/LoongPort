import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { ApplicationRoutingChange } from "@/lib/api/applicationRouting";
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
  } | null>(null);
  const pending = useRef(false);
  const lifecycle = useRef(0);
  useEffect(
    () => () => {
      lifecycle.current += 1;
    },
    [],
  );

  const discard = () => {
    setStagedIds(null);
    setProfileName(null);
    setShowAll(false);
    setSort(null);
    setAccountFilter(null);
    setModelFilter(null);
    setSearch("");
  };
  const submit = async (
    change: ApplicationRoutingChange,
    quitChatgpt?: boolean,
  ) => {
    if (pending.current) return;
    const generation = lifecycle.current;
    pending.current = true;
    setSubmitting(true);
    setConfirmation(null);
    try {
      const result = await apply(change, quitChatgpt);
      if (generation !== lifecycle.current) return;
      if (result.status === "confirmationRequired") {
        setConfirmation({ name: result.targetName, change });
        return;
      }
      if (change.order) discard();
      toast.success(t("applications.changesApplied"));
      result.warnings.forEach((warning) => toast.warning(warning));
      if (result.chatgptWasRunning && !result.chatgptRelaunched) {
        toast.info(
          t("loongport.switch.doneNeedsRestart", { name: result.providerName }),
        );
      }
    } catch {
      // The mutation reports the original error. Keep local intent for retry.
    } finally {
      pending.current = false;
      if (generation === lifecycle.current) setSubmitting(false);
    }
  };
  return {
    search,
    setSearch,
    sort,
    setSort,
    accountFilter,
    setAccountFilter,
    modelFilter,
    setModelFilter,
    stagedIds,
    setStagedIds,
    profileName,
    setProfileName,
    showAll,
    setShowAll,
    submitting,
    confirmation,
    submit,
    discard,
    cancel: () => setConfirmation(null),
    confirm: (quit: boolean) =>
      confirmation && void submit(confirmation.change, quit),
    saveAs: (name: string, ids: string[]) => {
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
