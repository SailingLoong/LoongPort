import { useQueryClient } from "@tanstack/react-query";
import { accountPlatformPresenceKey } from "@/components/relay/accounts/useAccountPlatformPresence";
import { workbuddyNativeCapturesKey } from "@/components/relay/accounts/useWorkBuddyNativeCatalog";
import { useEffect, useRef, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { usePreservedViewActive } from "@/components/ui/PreservedView";
import {
  workbuddyApi,
  type WorkBuddyNativeAttachmentView,
} from "@/lib/api/workbuddy";

type AttachmentState = {
  owner: string | null;
  flowId: string | null;
  view: WorkBuddyNativeAttachmentView | null;
  busy: boolean;
  hidden: boolean;
  abandoned: boolean;
  unknown: boolean;
  unowned: boolean;
  epoch: number;
  discovered: WorkBuddyNativeAttachmentView | null;
  previousUnknown: boolean;
  submitted: boolean;
  requestVersion: number;
  refreshed: boolean;
};
// One ephemeral IPC gesture. A component lifetime cannot revoke or replay a worker.
// Nothing here is persisted as an account, permission, current-client, or login fact.
let state: AttachmentState = {
  owner: null,
  flowId: null,
  view: null,
  busy: false,
  hidden: false,
  abandoned: false,
  unknown: false,
  unowned: false,
  epoch: 0,
  discovered: null,
  previousUnknown: false,
  submitted: false,
  requestVersion: 0,
  refreshed: false,
};
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};
const snapshot = () => state;
function update(patch: Partial<AttachmentState>) {
  state = { ...state, ...patch };
  listeners.forEach((f) => f());
}
type RequestTicket = { epoch: number; version: number };
function beginRequest(): RequestTicket {
  const ticket = { epoch: state.epoch, version: state.requestVersion + 1 };
  update({ busy: true, requestVersion: ticket.version });
  return ticket;
}
function current(ticket: RequestTicket) {
  return (
    state.epoch === ticket.epoch && state.requestVersion === ticket.version
  );
}
function receive(view: WorkBuddyNativeAttachmentView, ticket: RequestTicket) {
  if (!current(ticket)) return;
  if (view.account && view.account.id !== state.owner) {
    update({ view: null, busy: false, unknown: true });
    return;
  }
  update({
    view,
    busy: false,
    unknown:
      view.state === "unknown" || (state.submitted && view.state === "ready"),
  });
}
function failed(ticket: RequestTicket) {
  if (current(ticket))
    update({ busy: false, unknown: state.view?.state !== "saved" });
}
async function discard(flowId: string, epoch: number) {
  if (state.epoch !== epoch || state.flowId !== flowId) return;
  const ticket = beginRequest();
  try {
    receive(await workbuddyApi.cancelNativeAttachment(flowId), ticket);
  } catch {
    failed(ticket);
  }
}
export function WorkBuddyNativeAttachment({
  accountId,
  onAttached,
}: {
  accountId: string;
  onAttached?: () => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const shared = useSyncExternalStore(subscribe, snapshot, snapshot);
  const active = useRef(false);
  const viewActive = usePreservedViewActive();
  const latest = useRef({ accountId, viewActive });
  latest.current = { accountId, viewActive };
  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);
  useEffect(() => {
    if (
      !active.current ||
      !viewActive ||
      state.owner !== accountId ||
      state.refreshed ||
      state.view?.state !== "saved" ||
      !state.flowId ||
      state.view.flowId !== state.flowId ||
      state.view.account?.id !== accountId ||
      state.view.saved?.selection.id !== accountId
    )
      return;
    // Only the matching receipt triggers reads, never the inventory or discovered slot.
    update({ refreshed: true });
    void client
      .invalidateQueries({ queryKey: accountPlatformPresenceKey })
      .catch(() => {});
    void client
      .invalidateQueries({ queryKey: workbuddyNativeCapturesKey })
      .catch(() => {});
    try {
      onAttached?.();
    } catch {
      /* A read refresh cannot undo the receipt. */
    }
  }, [shared, accountId, viewActive, client, onAttached]);
  const live = () =>
    active.current &&
    latest.current.viewActive &&
    latest.current.accountId === accountId;
  const gestureCurrent = () =>
    live() &&
    state.epoch === shared.epoch &&
    state.requestVersion === shared.requestVersion &&
    state.flowId === shared.flowId;
  const owned = shared.owner === accountId;
  const start = async () => {
    if (
      !gestureCurrent() ||
      state.busy ||
      (state.owner !== null &&
        state.view?.state !== "cancelled" &&
        state.view?.state !== "saved")
    )
      return;
    const epoch = state.epoch + 1;
    update({
      owner: accountId,
      flowId: null,
      view: null,
      busy: true,
      hidden: false,
      abandoned: false,
      unknown: false,
      unowned: false,
      epoch,
      discovered: null,
      previousUnknown: false,
      submitted: false,
      requestVersion: 0,
      refreshed: false,
    });
    const ticket = beginRequest();
    try {
      const view = await workbuddyApi.prepareNativeAttachment(accountId);
      if (!current(ticket)) return;
      // Only the direct prepare response establishes ownership of its flow.
      if (view.account && view.account.id !== accountId)
        throw new Error("mismatched target");
      update({
        view,
        flowId: view.flowId,
        busy: false,
        unknown:
          view.state === "unknown" ||
          (state.submitted && view.state === "ready"),
      });
      if (state.abandoned && view.flowId) {
        update({ busy: true });
        await discard(view.flowId, epoch);
      }
    } catch {
      failed(ticket);
    }
  };
  const inspect = async () => {
    if (!gestureCurrent() || state.busy || state.owner !== accountId) return;
    const { flowId } = state;
    const ticket = beginRequest();
    try {
      const view = await workbuddyApi.inspectNativeAttachment(flowId);
      if (!current(ticket)) return;
      // Current-slot discovery cannot identify a lost prepare request, even for the same id.
      if (flowId === null)
        update({ unowned: true, unknown: true, busy: false, discovered: view });
      else receive(view, ticket);
    } catch {
      failed(ticket);
    }
  };
  const confirm = async () => {
    if (
      !gestureCurrent() ||
      state.busy ||
      state.owner !== accountId ||
      state.hidden ||
      state.unowned ||
      state.submitted ||
      state.unknown ||
      state.view?.state !== "ready" ||
      !state.view.available ||
      !state.flowId ||
      state.view.account?.id !== accountId
    )
      return;
    const { flowId } = state;
    const ticket = beginRequest();
    update({
      busy: true,
      view: { ...state.view, state: "applying", expiresInMs: 0 },
      submitted: true,
    });
    try {
      const view = await workbuddyApi.confirmNativeAttachment(flowId);
      receive(view, ticket);
    } catch {
      failed(ticket);
    }
  };
  const close = () => {
    if (!gestureCurrent() || state.owner !== accountId) return;
    update({ hidden: true, abandoned: true, busy: false });
    // Closing an applying/unknown operation only detaches the presentation.
    if (
      state.view?.state === "ready" &&
      !state.submitted &&
      !state.busy &&
      state.flowId
    ) {
      update({ busy: true });
      void discard(state.flowId, state.epoch);
    }
  };
  const stopTracking = () => {
    if (
      !gestureCurrent() ||
      state.owner !== accountId ||
      (!state.unknown && !state.unowned) ||
      state.view?.state === "ready" ||
      state.discovered?.state === "ready"
    )
      return;
    // Retire this waiter's callbacks, not the backend transaction or its outcome.
    update({
      owner: null,
      flowId: null,
      view: null,
      discovered: null,
      busy: false,
      hidden: false,
      unknown: false,
      unowned: false,
      abandoned: false,
      submitted: false,
      previousUnknown: false,
      requestVersion: 0,
      refreshed: false,
      epoch: state.epoch + 1,
    });
  };
  const reviewedAccount = shared.view?.account ?? shared.discovered?.account;
  const reviewCurrent = () => {
    const view = state.discovered;
    if (
      !gestureCurrent() ||
      state.busy ||
      !state.unowned ||
      view !== shared.discovered ||
      view?.state !== "ready" ||
      !view.available ||
      !view.flowId ||
      view.account?.id !== accountId
    )
      return;
    // A fresh explicit user gesture adopts this displayed plan, not the lost request.
    update({
      view,
      flowId: view.flowId,
      discovered: null,
      previousUnknown: true,
      submitted: false,
      requestVersion: 0,
      refreshed: false,
      unowned: false,
      unknown: false,
      abandoned: false,
      epoch: state.epoch + 1,
    });
  };
  const text = (key: string) => t(`workbuddy.attachment.${key}`);
  const canStart =
    !shared.busy &&
    (shared.owner === null ||
      shared.view?.state === "cancelled" ||
      shared.view?.state === "saved");
  return (
    <section
      className="space-y-2 rounded-lg border border-border p-3"
      aria-label={text("title")}
    >
      <p className="text-xs text-muted-foreground">{text("boundary")}</p>
      {(!owned || shared.view?.state === "cancelled") && (
        <Button
          size="sm"
          variant="outline"
          disabled={!canStart}
          onClick={() => void start()}
        >
          {text("prepare")}
        </Button>
      )}
      {owned &&
        shared.view?.state !== "cancelled" &&
        (shared.hidden ? (
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              if (gestureCurrent()) update({ hidden: false });
            }}
          >
            {text("show")}
          </Button>
        ) : (
          <>
            {reviewedAccount && (
              <div>
                <p>{reviewedAccount.label}</p>
                <p className="text-xs text-muted-foreground">
                  {reviewedAccount.realm}
                </p>
              </div>
            )}
            {shared.previousUnknown && (
              <p className="text-xs text-muted-foreground">
                {text("previousUnknown")}
              </p>
            )}
            {shared.unowned &&
              shared.discovered?.state === "ready" &&
              shared.discovered.available &&
              shared.discovered.account?.id === accountId && (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={shared.busy}
                  onClick={reviewCurrent}
                >
                  {text("reviewCurrent")}
                </Button>
              )}
            {(shared.unknown || shared.unowned) &&
              shared.view?.state !== "ready" &&
              shared.discovered?.state !== "ready" && (
                <>
                  <p className="text-xs text-muted-foreground">
                    {text("stopTrackingHint")}
                  </p>
                  <Button size="sm" variant="outline" onClick={stopTracking}>
                    {text("stopTracking")}
                  </Button>
                </>
              )}
            <p role="status">
              {text(
                shared.unowned
                  ? "unowned"
                  : shared.unknown
                    ? "unknown"
                    : (shared.view?.state ?? "preparing"),
              )}
            </p>
            {(shared.view?.reason ?? shared.discovered?.reason) && (
              <p className="text-xs text-muted-foreground">
                {text(
                  `reason.${shared.view?.reason ?? shared.discovered?.reason}`,
                )}
              </p>
            )}
            {shared.view?.state === "ready" && !shared.unknown && (
              <Button
                size="sm"
                disabled={shared.busy || !shared.view.available}
                onClick={() => void confirm()}
              >
                {text("confirm")}
              </Button>
            )}
            {shared.view?.state === "applying" && shared.busy && (
              <Button size="sm" disabled>
                {text("confirm")}
              </Button>
            )}
            <Button
              size="sm"
              variant="outline"
              disabled={shared.busy}
              onClick={() => void inspect()}
            >
              {text("inspect")}
            </Button>
            <Button size="sm" variant="ghost" onClick={close}>
              {text("close")}
            </Button>
          </>
        ))}
    </section>
  );
}
