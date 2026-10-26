import { useImperativeHandle, useRef, useState, type Ref } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowRight, Plus, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { AppId } from "@/lib/api";
import {
  sharedRoutingApi,
  type GroupDraft,
  type PreparedGroup,
  type SharedGroup,
} from "@/lib/api/sharedRouting";
import { serviceSourcesApi } from "@/lib/api/serviceSources";
import { serviceSourcesKey } from "@/components/relay/accounts/ServiceSourceList";
import type { ApplicationWorkspaceHandle } from "./ApplicationWorkspace";
import { sharedRoutingKey } from "./useSharedRouteWorkspace";
import { SharedGroupEditor } from "./SharedGroupEditor";
const image = (value: unknown) => JSON.stringify(value);
const sameGroup = (left: SharedGroup | undefined, right: SharedGroup) =>
  left?.id === right.id &&
  left.revision === right.revision &&
  left.name === right.name &&
  image(left.members) === image(right.members);
export function SharedGroupsPage({
  appId,
  ref,
  onOpenApplications,
}: {
  appId: AppId;
  ref?: Ref<ApplicationWorkspaceHandle>;
  onOpenApplications: () => void;
}) {
  const { t } = useTranslation(),
    client = useQueryClient();
  const query = useQuery({
    queryKey: sharedRoutingKey(appId),
    queryFn: () => sharedRoutingApi.get(appId),
    retry: false,
  });
  const sourceQuery = useQuery({
    queryKey: serviceSourcesKey,
    queryFn: serviceSourcesApi.list,
    retry: false,
  });
  const [draft, setDraft] = useState<GroupDraft | null>(null),
    [prepared, setPrepared] = useState<PreparedGroup | null>(null),
    [busy, setBusy] = useState(false),
    [notice, setNotice] = useState<
      "saved" | "failed" | "changed" | "unknown" | null
    >(null),
    [uncertain, setUncertain] = useState<PreparedGroup | null>(null);
  const submitting = useRef(false),
    basis = useRef<string | null>(null),
    initialDraft = useRef<string | null>(null);
  const readFailed =
      query.isError ||
      sourceQuery.isError ||
      Boolean(query.data && query.data.appId !== appId),
    ready =
      query.isSuccess &&
      sourceQuery.isSuccess &&
      !readFailed &&
      !query.isFetching &&
      !sourceQuery.isFetching;
  const close = () => {
    setDraft(null);
    setPrepared(null);
    initialDraft.current = null;
  };
  useImperativeHandle(ref, () => ({
    hasPendingChanges:
      uncertain !== null ||
      (draft !== null && image(draft) !== initialDraft.current),
    submitting: busy,
    discard: close,
  }));
  const refresh = async () => {
    await client.invalidateQueries({ queryKey: ["sharedRouting"] });
  };
  const start = (group?: SharedGroup) => {
    const next: GroupDraft = group
      ? {
          id: group.id,
          expectedRevision: group.revision,
          name: group.name,
          members: group.members.map((member) => ({ ...member })),
        }
      : { id: null, expectedRevision: null, name: "", members: [] };
    initialDraft.current = image(next);
    setDraft(next);
    setPrepared(null);
    setNotice(null);
  };
  const prepare = async () => {
    if (submitting.current || !ready || !draft || uncertain) return;
    submitting.current = true;
    setBusy(true);
    setNotice(null);
    const input = structuredClone(draft),
      before = image(query.data?.groups);
    try {
      const result = await sharedRoutingApi.prepareGroup(input);
      if (
        result.expectedRevision !== input.expectedRevision ||
        !result.draftDigest ||
        !result.group?.id ||
        result.group.name !== input.name ||
        image(result.group.members) !== image(input.members) ||
        (input.id !== null && result.group.id !== input.id) ||
        !Array.isArray(result.affectedAppIds) ||
        result.affectedAppIds.some((app) => typeof app !== "string") ||
        before !==
          image(
            client.getQueryData<{ groups: SharedGroup[] }>(
              sharedRoutingKey(appId),
            )?.groups,
          )
      ) {
        setNotice("changed");
        return;
      }
      basis.current = before;
      setPrepared(result);
    } catch {
      setNotice("failed");
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  const accepted = async () => {
    setNotice("saved");
    setUncertain(null);
    close();
    await refresh();
  };
  const inspect = async (review: PreparedGroup) => {
    try {
      const state = await sharedRoutingApi.get(appId);
      if (state.appId !== appId)
        throw new Error("routing_groups.invalid_state");
      client.setQueryData(sharedRoutingKey(appId), state);
      if (
        sameGroup(
          state.groups.find((group) => group.id === review.group.id),
          review.group,
        )
      ) {
        await accepted();
        return;
      }
    } catch {
      /* Public status only: backend details can contain account data. */
    }
    setUncertain(review);
    setNotice("unknown");
  };
  const confirm = async () => {
    if (submitting.current || !ready || !prepared || uncertain) return;
    if (
      basis.current !==
      image(
        client.getQueryData<{ groups: SharedGroup[] }>(sharedRoutingKey(appId))
          ?.groups,
      )
    ) {
      setPrepared(null);
      setNotice("changed");
      return;
    }
    const review = prepared;
    submitting.current = true;
    setBusy(true);
    setPrepared(null);
    try {
      try {
        const result = await sharedRoutingApi.confirmGroup(review);
        if (sameGroup(result, review.group)) await accepted();
        else await inspect(review);
      } catch {
        await inspect(review);
      }
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  const inspectUncertain = async () => {
    if (submitting.current || !uncertain) return;
    submitting.current = true;
    setBusy(true);
    try {
      await inspect(uncertain);
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  const status = notice && (
    <div role="status" className="rounded-xl border border-border-default p-4">
      <p className="text-sm">
        {t(
          notice === "saved"
            ? "sharedGroups.savedNotBound"
            : notice === "unknown"
              ? "sharedGroups.outcomeUnknown"
              : notice === "changed"
                ? "sharedGroups.changed"
                : "sharedGroups.failed",
        )}
      </p>
      {notice === "saved" && (
        <Button
          variant="outline"
          size="sm"
          className="mt-3"
          onClick={onOpenApplications}
        >
          {t("sharedGroups.openApplications")}
          <ArrowRight className="ml-2 h-4 w-4" />
        </Button>
      )}
      {uncertain && (
        <Button
          variant="outline"
          size="sm"
          className="mt-3"
          disabled={busy}
          onClick={() => void inspectUncertain()}
        >
          {t("sharedGroups.inspect")}
        </Button>
      )}
    </div>
  );
  return (
    <div className="page-content space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h1 className="text-[26px] font-semibold">
            {t("sharedGroups.title")}
          </h1>
          <p className="mt-2 max-w-2xl text-sm text-muted-foreground">
            {t("sharedGroups.subtitle")}
          </p>
        </div>
        <Button
          disabled={!ready || busy || Boolean(uncertain)}
          onClick={() => start()}
        >
          <Plus className="mr-2 h-4 w-4" />
          {t("sharedGroups.create")}
        </Button>
      </div>
      {readFailed ? (
        <div role="alert" className="rounded-xl border border-amber-500/30 p-4">
          <p className="text-sm">{t("sharedGroups.readFailed")}</p>
          <Button
            variant="outline"
            size="sm"
            className="mt-3"
            onClick={() =>
              void Promise.all([query.refetch(), sourceQuery.refetch()])
            }
          >
            <RefreshCw className="mr-2 h-4 w-4" />
            {t("common.retry")}
          </Button>
        </div>
      ) : query.isPending || sourceQuery.isPending ? (
        <p role="status">{t("common.loading")}</p>
      ) : (
        <div className="grid items-start gap-4 lg:grid-cols-2">
          {query.data?.groups.map((group) => (
            <section
              key={group.id}
              className="rounded-2xl border border-border-default bg-card p-5"
            >
              <div className="flex items-center justify-between gap-3">
                <h2 className="min-w-0 break-words text-base font-semibold">
                  {group.name}
                </h2>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={!ready || busy || Boolean(uncertain)}
                  onClick={() => start(group)}
                >
                  {t("sharedGroups.edit")}
                </Button>
              </div>
              <p className="mt-2 text-xs text-muted-foreground">
                {t("sharedGroups.originalOrder")}
              </p>
              <ol
                data-testid={`group-members-${group.id}`}
                className="mt-4 space-y-2"
              >
                {group.members.map((member, index) => (
                  <li
                    key={`${index}:${member.sourceId}`}
                    className="flex gap-3 rounded-xl bg-muted/40 p-3"
                  >
                    <span className="text-xs text-muted-foreground">
                      {index + 1}
                    </span>
                    <div className="min-w-0">
                      <p className="break-words text-sm font-medium">
                        {sourceQuery.data?.find(
                          (source) => source.sourceId === member.sourceId,
                        )?.name ?? member.sourceId}
                      </p>
                      <p className="mt-1 break-all text-xs text-muted-foreground">
                        {member.model}
                      </p>
                    </div>
                  </li>
                ))}
              </ol>
            </section>
          ))}
          {query.data?.groups.length === 0 && (
            <p className="text-sm text-muted-foreground">
              {t("sharedGroups.empty")}
            </p>
          )}
        </div>
      )}
      {!draft && status}
      <SharedGroupEditor
        status={status}
        draft={draft}
        sources={sourceQuery.data ?? []}
        onChange={(next) => {
          setDraft(next);
          setPrepared(null);
        }}
        onClose={close}
        onPreview={() => void prepare()}
        onConfirm={() => void confirm()}
        onBack={() => setPrepared(null)}
        prepared={prepared}
        busy={busy}
        blocked={!ready || Boolean(uncertain)}
        canConfirm={
          Boolean(prepared) && basis.current === image(query.data?.groups)
        }
      />
    </div>
  );
}
