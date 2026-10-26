import { useState, useImperativeHandle, type ReactNode, type Ref } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  ArrowRight,
  ChevronDown,
  Layers3,
  RefreshCw,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { AppId } from "@/lib/api";
import type {
  ServiceAccountReference,
  ServiceSource,
} from "@/lib/api/serviceSources";
import type { ApplicationWorkspaceHandle } from "./ApplicationWorkspace";
import { useSharedRouteWorkspace } from "./useSharedRouteWorkspace";
import { ApplicationRouteAdvanced } from "./ApplicationRouteAdvanced";
import { ApplicationStartupCheckpoint } from "./ApplicationStartupCheckpoint";
import { RouteTargetSummary, SharedRouteReview } from "./SharedRouteReview";

export const routeSelectClass =
  "mt-2 h-11 w-full rounded-lg border border-input bg-background px-3 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50";
/** Native projected selection facts only; saved identities need not still be selectable. */
export function selectableSourceTargets(source: ServiceSource, appId: AppId) {
  const facts =
    source.account.kind === "localProvider"
      ? (source.supportedTargets ?? [])
      : (source.supportedTargets ?? source.bindings);
  return [
    ...new Set(
      facts
        .filter(
          (fact) =>
            fact.appId === appId &&
            fact.canSelect === true &&
            typeof fact.model === "string" &&
            fact.model.length > 0,
        )
        .map((fact) => fact.model as string),
    ),
  ];
}
export function ApplicationRouteWorkspace({
  appId,
  ref,
  onOpenSource,
  onManageGroups,
  onAdd,
  children,
}: {
  appId: AppId;
  ref?: Ref<ApplicationWorkspaceHandle>;
  onOpenSource: (account: ServiceAccountReference) => void;
  onManageGroups: () => void;
  onAdd: () => void;
  children?: ReactNode | ((disabled: boolean) => ReactNode);
}) {
  const { t } = useTranslation(),
    workspace = useSharedRouteWorkspace(appId);
  const { state, sources, connection, draft, error } = workspace;
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [advancedBusy, setAdvancedBusy] = useState(false);
  const busy = workspace.busy || advancedBusy;
  useImperativeHandle(ref, () => ({
    hasPendingChanges: workspace.hasPendingChanges,
    submitting: busy,
    discard: workspace.discard,
  }));
  const savedTarget = state?.binding === null ? null : state?.binding?.target;
  const current = draft ?? savedTarget ?? null,
    groups = state?.groups ?? [];
  const group =
    current?.kind === "orderedGroup"
      ? groups.find((group) => group.id === current.groupId)
      : undefined;
  const candidates = (sources ?? [])
    .map((source) => ({
      source,
      models: selectableSourceTargets(source, appId),
    }))
    .filter((item) => item.models.length > 0);
  const manual = current?.kind === "manual" ? current : null;
  const selected = candidates.find(
    (item) => item.source.sourceId === manual?.sourceId,
  );
  const manualSource = sources?.find(
    (source) => source.sourceId === manual?.sourceId,
  );
  const disabled =
    busy ||
    error ||
    workspace.loading ||
    state?.canApply !== true ||
    state.blockedReason !== null ||
    Boolean(workspace.pendingRequest);
  const mode = workspace.modeState;
  return (
    <section className="space-y-6" aria-label={t("routingWorkspace.workspace")}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-[26px] font-semibold leading-8 tracking-tight">
            {t("routingWorkspace.title", { app: t(`apps.${appId}`) })}
          </h1>
          <p className="mt-2 text-sm text-muted-foreground">
            {t("routingWorkspace.subtitle")}
          </p>
        </div>
        <Button
          size="sm"
          variant="ghost"
          disabled={busy || workspace.loading}
          onClick={() => void workspace.refresh()}
        >
          <RefreshCw className="mr-2 h-4 w-4" />
          {t("common.refresh")}
        </Button>
      </div>
      {error && (
        <div
          role="alert"
          className="rounded-xl border border-amber-500/30 bg-amber-500/5 p-4 text-sm"
        >
          {t("routingWorkspace.readFailed")}
        </div>
      )}
      {workspace.loading && (
        <p role="status" className="text-sm text-muted-foreground">
          {t("common.loading")}
        </p>
      )}
      <div className="grid gap-3 md:grid-cols-[1fr_1fr_1.17fr]">
        <section className="rounded-xl border border-border-default bg-card p-4">
          <h2 className="mb-2 text-[11px] font-semibold leading-4 text-muted-foreground">
            01 · {t("routingWorkspace.connection")}
          </h2>
          <p className="text-lg font-semibold leading-7">
            {t(
              mode?.attached === true
                ? "routingWorkspace.attached"
                : mode?.attached === false
                  ? "routingWorkspace.detached"
                  : "routingWorkspace.connectionUnknown",
            )}
          </p>
          <p className="mt-2 text-xs text-muted-foreground">
            {t(
              connection?.routingActive === true
                ? "routingWorkspace.gatewayRunning"
                : "routingWorkspace.gatewayNotVerified",
            )}
          </p>
        </section>
        <section className="rounded-xl border border-border-default bg-card p-4">
          <h2 className="mb-2 text-[11px] font-semibold leading-4 text-muted-foreground">
            02 · {t("routingWorkspace.savedConfiguration")}
          </h2>
          <RouteTargetSummary
            target={savedTarget}
            className="text-lg leading-7"
            groups={groups}
            sources={sources}
          />
        </section>
        <section
          data-testid="actual-request-fact"
          className="rounded-xl border border-border-default bg-card p-4"
        >
          <h2 className="mb-2 text-[11px] font-semibold leading-4 text-muted-foreground">
            03 · {t("routingWorkspace.actualRequest")}
          </h2>
          <p className="text-lg font-semibold leading-7">
            {t("routingWorkspace.requestUnknown")}
          </p>
          <p className="mt-2 text-xs text-muted-foreground">
            {t("routingWorkspace.requestDisclaimer")}
          </p>
        </section>
      </div>
      <div className="grid items-start gap-6 lg:grid-cols-[1.66fr_1fr]">
        <section className="min-w-0 rounded-2xl border border-border-default bg-card p-5 sm:p-6">
          <div className="flex items-center justify-between gap-3">
            <h2 className="text-xl font-bold leading-7">
              {t("routingWorkspace.edit")}
            </h2>
            <span
              data-testid="route-edit-state"
              className={cn(
                "rounded-full px-2.5 py-1 text-xs",
                workspace.hasPendingChanges
                  ? "bg-amber-500/10 text-amber-700 dark:text-amber-400"
                  : "bg-muted text-muted-foreground",
              )}
            >
              {t(
                workspace.hasPendingChanges
                  ? "routingWorkspace.unsaved"
                  : savedTarget === undefined || error
                    ? "routingWorkspace.savedStateUnknown"
                    : savedTarget === null
                      ? "routingWorkspace.unconfigured"
                      : "routingWorkspace.saved",
              )}
            </span>
          </div>
          <p className="mt-2 text-sm text-muted-foreground">
            {t("routingWorkspace.editHelp", { app: t(`apps.${appId}`) })}
          </p>
          <fieldset
            disabled={disabled}
            className="mt-5 grid gap-3 sm:grid-cols-2"
          >
            <legend className="sr-only">{t("routingWorkspace.policy")}</legend>
            {(["manual", "orderedGroup"] as const).map((kind) => (
              <label
                key={kind}
                className={cn(
                  "flex cursor-pointer items-start gap-3 rounded-xl border p-4 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring has-[:focus-visible]:ring-offset-2",
                  current?.kind === kind
                    ? "border-[#315feb] bg-[#edf2ff] text-[#315feb] dark:border-primary/50 dark:bg-primary/10 dark:text-blue-400"
                    : "border-border-default",
                  disabled && "cursor-not-allowed opacity-60",
                )}
              >
                <input
                  className="mt-1 accent-primary"
                  type="radio"
                  name={`route-policy-${appId}`}
                  checked={current?.kind === kind}
                  onChange={() =>
                    workspace.setDraft(
                      kind === "manual"
                        ? { kind, sourceId: "", model: "" }
                        : { kind, groupId: "" },
                    )
                  }
                  aria-label={t(
                    kind === "manual"
                      ? "routingWorkspace.manual"
                      : "routingWorkspace.ordered",
                  )}
                />
                <span>
                  <span className="block text-sm font-semibold">
                    {t(
                      kind === "manual"
                        ? "routingWorkspace.manual"
                        : "routingWorkspace.ordered",
                    )}
                  </span>
                  <span className="mt-1 block text-xs leading-relaxed text-muted-foreground">
                    {t(
                      kind === "manual"
                        ? "routingWorkspace.manualHelp"
                        : "routingWorkspace.orderedHelp",
                    )}
                  </span>
                </span>
              </label>
            ))}
          </fieldset>
          {current?.kind === "orderedGroup" ? (
            <div className="mt-5">
              <div className="flex items-center justify-between gap-3">
                <label
                  htmlFor={`route-group-${appId}`}
                  className="text-sm font-medium"
                >
                  {t("routingWorkspace.group")}
                </label>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={onManageGroups}
                >
                  {t("routingWorkspace.manageGroups")}
                  <ArrowRight className="ml-1 h-3.5 w-3.5" />
                </Button>
              </div>
              <select
                id={`route-group-${appId}`}
                className={routeSelectClass}
                disabled={disabled}
                value={current.groupId}
                onChange={(event) =>
                  workspace.setDraft({
                    kind: "orderedGroup",
                    groupId: event.target.value,
                  })
                }
              >
                <option value="">{t("routingWorkspace.chooseGroup")}</option>
                {current.groupId && !group && (
                  <option value={current.groupId} disabled>
                    {current.groupId}
                  </option>
                )}
                {groups.map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.name}
                  </option>
                ))}
              </select>
              {group ? (
                <>
                  <div className="mt-5 flex flex-wrap justify-between gap-2 text-xs text-muted-foreground">
                    <span>{t("routingWorkspace.originalOrder")}</span>
                    <span>{t("routingWorkspace.groupEligibilityUnknown")}</span>
                  </div>
                  <ol
                    data-testid="shared-member-list"
                    className="mt-3 space-y-2"
                  >
                    {group.members.map((member, index) => {
                      const source = sources?.find(
                        (item) => item.sourceId === member.sourceId,
                      );
                      return (
                        <li
                          key={`${index}:${member.sourceId}:${member.model}`}
                          className="grid min-h-[45px] grid-cols-[20px_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 rounded-lg bg-[#f8f9fc] px-3 py-2 dark:bg-muted/40 md:grid-cols-[20px_minmax(0,1.3fr)_minmax(0,1fr)_auto]"
                        >
                          <span className="row-span-2 text-center text-xs text-muted-foreground md:row-span-1">
                            {index + 1}
                          </span>
                          <div className="min-w-0 flex-1">
                            <p className="break-words text-[13px] font-semibold leading-5">
                              {source?.name ?? member.sourceId}
                            </p>
                            {!source && (
                              <p className="mt-1 text-xs text-muted-foreground">
                                {t("routingWorkspace.sourceUnknown")}
                              </p>
                            )}
                          </div>
                          <p className="col-start-2 row-start-2 break-all text-xs text-muted-foreground md:col-start-auto md:row-start-auto">
                            {member.model}
                          </p>
                          <div className="col-start-3 row-span-2 row-start-1 flex shrink-0 items-center gap-1 md:col-start-auto md:row-span-1 md:row-start-auto">
                            <span className="rounded-full bg-muted px-2 py-1 text-xs text-muted-foreground">
                              {t("routingWorkspace.eligibilityUnknown")}
                            </span>
                            {source && (
                              <Button
                                variant="ghost"
                                size="sm"
                                className="h-6 w-6 p-0"
                                aria-label={t("routingWorkspace.manageSource")}
                                title={t("routingWorkspace.manageSource")}
                                disabled={busy}
                                onClick={() => onOpenSource(source.account)}
                              >
                                <ArrowRight className="h-3.5 w-3.5" />
                              </Button>
                            )}
                          </div>
                        </li>
                      );
                    })}
                  </ol>
                  {group.members.length === 0 && (
                    <p className="mt-3 text-sm text-muted-foreground">
                      {t("routingWorkspace.emptyGroup")}
                    </p>
                  )}
                </>
              ) : current.groupId ? (
                <p className="mt-3 text-sm text-muted-foreground">
                  {t("routingWorkspace.groupUnknown")}
                </p>
              ) : null}
            </div>
          ) : current?.kind === "manual" ? (
            <div className="mt-5 space-y-4">
              <div>
                <label
                  htmlFor={`route-source-${appId}`}
                  className="text-sm font-medium"
                >
                  {t("routingWorkspace.source")}
                </label>
                <select
                  id={`route-source-${appId}`}
                  className={routeSelectClass}
                  value={current.sourceId}
                  disabled={disabled}
                  onChange={(event) =>
                    workspace.setDraft({
                      kind: "manual",
                      sourceId: event.target.value,
                      model: "",
                    })
                  }
                >
                  <option value="">{t("routingWorkspace.chooseSource")}</option>
                  {current.sourceId && !selected && (
                    <option value={current.sourceId} disabled>
                      {sources?.find(
                        (source) => source.sourceId === current.sourceId,
                      )?.name ?? current.sourceId}
                    </option>
                  )}
                  {candidates.map(({ source }) => (
                    <option key={source.sourceId} value={source.sourceId}>
                      {source.name}
                    </option>
                  ))}
                </select>
              </div>
              <div>
                <label
                  htmlFor={`route-model-${appId}`}
                  className="text-sm font-medium"
                >
                  {t("routingWorkspace.model")}
                </label>
                <select
                  id={`route-model-${appId}`}
                  className={routeSelectClass}
                  value={current.model}
                  disabled={disabled || !selected}
                  onChange={(event) =>
                    workspace.setDraft({
                      ...current,
                      model: event.target.value,
                    })
                  }
                >
                  <option value="">{t("routingWorkspace.chooseModel")}</option>
                  {current.model &&
                    !selected?.models.includes(current.model) && (
                      <option value={current.model} disabled>
                        {current.model}
                      </option>
                    )}
                  {selected?.models.map((model) => (
                    <option key={model} value={model}>
                      {model}
                    </option>
                  ))}
                </select>
              </div>
              {manualSource &&
                manual?.model &&
                !selected?.models.includes(manual.model) && (
                  <div className="rounded-xl border border-amber-500/25 bg-amber-500/5 p-4">
                    <p className="text-sm text-muted-foreground">
                      {t("routingWorkspace.savedTargetNotSelectable")}
                    </p>
                    <Button
                      variant="outline"
                      size="sm"
                      className="mt-3"
                      disabled={busy}
                      onClick={() => onOpenSource(manualSource.account)}
                    >
                      {t("routingWorkspace.manageSource")}
                    </Button>
                  </div>
                )}
              {sources && candidates.length === 0 && (
                <div className="rounded-xl bg-muted/50 p-4">
                  <p className="text-sm text-muted-foreground">
                    {t("routingWorkspace.noSelectableTargets")}
                  </p>
                  <Button
                    variant="outline"
                    size="sm"
                    className="mt-3"
                    disabled={busy}
                    onClick={onAdd}
                  >
                    {t("routingWorkspace.addSource")}
                  </Button>
                </div>
              )}
            </div>
          ) : (
            <p className="mt-5 text-sm text-muted-foreground">
              {t("routingWorkspace.choosePolicy")}
            </p>
          )}
          {disabled && !busy && !workspace.loading && !error && (
            <p className="mt-4 rounded-lg bg-amber-500/5 p-3 text-sm text-amber-700 dark:text-amber-400">
              {t("routingWorkspace.writeUnavailable")}
            </p>
          )}
        </section>
        <aside className="min-w-0 space-y-4">
          <section className="rounded-xl border border-primary/10 bg-[#f5f8ff] p-5 dark:bg-primary/5 sm:p-6">
            <h2 className="text-xl font-bold leading-7">
              {t("routingWorkspace.compare")}
            </h2>
            <div className="mt-5 border-b border-primary/10 pb-5">
              <h3 className="mb-2 text-[11px] font-semibold text-muted-foreground">
                {t("routingWorkspace.saved")}
              </h3>
              <RouteTargetSummary
                target={savedTarget}
                className="text-[15px] leading-6"
                groups={groups}
                sources={sources}
                inlineModel
              />
            </div>
            <div className="mt-5">
              <h3 className="mb-2 text-[11px] font-semibold text-amber-700 dark:text-amber-400">
                {t("routingWorkspace.draft")}
              </h3>
              {draft ? (
                <RouteTargetSummary
                  target={draft}
                  className="text-[17px] leading-7 text-[#315feb] dark:text-blue-400"
                  groups={groups}
                  sources={sources}
                  inlineModel
                />
              ) : (
                <p className="text-sm text-muted-foreground">
                  {t("routingWorkspace.noDraft")}
                </p>
              )}
              <p className="mt-3 text-xs leading-relaxed text-muted-foreground">
                {t("routingWorkspace.referenceOnly")}
              </p>
            </div>
            <p className="mt-4 text-xs text-muted-foreground">
              {t("routingWorkspace.requestStillUnknown")}
            </p>
          </section>
          <section className="rounded-xl border border-amber-500/25 bg-amber-500/5 p-4">
            <div className="flex items-start gap-2">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-600" />
              <div>
                <h2 className="text-sm font-semibold">
                  {t("routingWorkspace.unknownTitle")}
                </h2>
                <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                  {t("routingWorkspace.unknownHelp")}
                </p>
              </div>
            </div>
          </section>
          <section className="rounded-xl border border-border-default bg-card p-4">
            <h2 className="text-sm font-semibold">
              {t("routingWorkspace.recoveryTitle")}
            </h2>
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
              {t("routingWorkspace.recoveryHelp")}
            </p>
            <ApplicationStartupCheckpoint
              appId={appId}
              modeState={mode}
              onRefresh={workspace.refresh}
            />
            {workspace.pendingRequest ? (
              <div className="mt-3 flex flex-wrap gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() => void workspace.inspectRequest()}
                >
                  {t("routingWorkspace.inspectOperation")}
                </Button>
                {workspace.canRecover && (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onClick={() => void workspace.recoverRequest()}
                  >
                    {t("routingWorkspace.recoverOperation")}
                  </Button>
                )}
              </div>
            ) : (
              <details className="mt-3 text-xs text-muted-foreground">
                <summary className="cursor-pointer rounded focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                  {t("routingWorkspace.recoveryDetails")}
                </summary>
                <p className="mt-2 leading-relaxed">
                  {t("routingWorkspace.recoveryBoundary")}
                </p>
              </details>
            )}
          </section>
        </aside>
      </div>
      {workspace.notice && (
        <p role="status" className="text-sm text-muted-foreground">
          {t(`routingWorkspace.notice.${workspace.notice}`)}
        </p>
      )}
      <footer className="flex flex-wrap items-center justify-between gap-4 rounded-xl bg-[#172b50] px-5 py-4 text-slate-50 dark:bg-slate-100 dark:text-slate-900">
        <div>
          <h2 className="text-sm font-semibold">
            {t(
              workspace.hasPendingChanges
                ? "routingWorkspace.draftUnsaved"
                : "routingWorkspace.noDraft",
            )}
          </h2>
          <p className="mt-1 text-xs opacity-70">
            {t("routingWorkspace.executionBoundary")}
          </p>
        </div>
        <div className="flex gap-2">
          <Button
            variant="outline"
            size="lg"
            className="bg-white font-semibold text-slate-900 hover:bg-slate-100 dark:bg-slate-900 dark:text-white dark:hover:bg-slate-800"
            disabled={busy || !workspace.hasPendingChanges}
            onClick={workspace.discard}
          >
            {t("routingWorkspace.discard")}
          </Button>
          <Button
            size="lg"
            className="bg-[#315feb] text-white hover:bg-[#264ec9]"
            disabled={!workspace.canPreview}
            onClick={() => void workspace.prepare()}
          >
            {t("routingWorkspace.preview")}
            <ArrowRight className="ml-2 h-4 w-4" />
          </Button>
        </div>
      </footer>
      <details
        open={advancedOpen}
        onToggle={(event) => {
          if (advancedBusy && !event.currentTarget.open)
            event.currentTarget.open = true;
          else setAdvancedOpen(event.currentTarget.open);
        }}
        className="rounded-xl border border-border-default bg-card p-4"
      >
        <summary className="flex cursor-pointer items-center gap-2 text-sm text-muted-foreground">
          <Layers3 className="h-4 w-4" />
          {t("routingWorkspace.advanced")}
          <ChevronDown className="h-4 w-4" />
        </summary>
        <div className="mt-4">
          {advancedOpen && (
            <ApplicationRouteAdvanced
              appId={appId}
              disabled={disabled || workspace.hasPendingChanges}
              onRefresh={workspace.refresh}
              onBusyChange={setAdvancedBusy}
            />
          )}
          {typeof children === "function"
            ? children(
                busy ||
                  workspace.hasPendingChanges ||
                  Boolean(workspace.pendingRequest),
              )
            : children}
        </div>
      </details>
      <SharedRouteReview
        open={Boolean(workspace.prepared)}
        onClose={workspace.closeReview}
        onConfirm={() => void workspace.confirm()}
        canConfirm={workspace.canConfirm}
        busy={busy}
        saved={savedTarget}
        draft={draft}
        groups={groups}
        sources={sources}
      />
    </section>
  );
}
