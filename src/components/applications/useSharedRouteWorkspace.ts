import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { AppId } from "@/lib/api";
import {
  sharedRoutingApi,
  type PreparedRouteTarget,
  type RouteRequestResult,
  type RouteTarget,
  type SharedRoutingState,
} from "@/lib/api/sharedRouting";
import { serviceSourcesApi } from "@/lib/api/serviceSources";
import {
  applicationRoutingApi,
  type ApplicationRoutingRequest,
  type ApplicationRouting,
} from "@/lib/api/applicationRouting";
import { applicationRoutingErrorModeState } from "./useApplicationRouting";
import { serviceSourcesKey } from "@/components/relay/accounts/ServiceSourceList";

export const sharedRoutingKey = (appId: AppId) =>
  ["sharedRouting", appId] as const;
const image = (value: unknown) => JSON.stringify(value);
const requestMatches = (
  a: ApplicationRoutingRequest,
  b: ApplicationRoutingRequest,
) =>
  a.id === b.id &&
  a.scope === b.scope &&
  a.draftDigest === b.draftDigest &&
  a.revision === b.revision;
const resolved = (result: RouteRequestResult | null) =>
  !result ||
  ["completed", "notRecorded", "discarded", "abandoned"].includes(
    result.status,
  );
function checkedResult(
  result: RouteRequestResult,
  request: ApplicationRoutingRequest,
): RouteRequestResult {
  if (
    !result?.request ||
    !requestMatches(result.request, request) ||
    ![
      "completed",
      "pending",
      "verificationRequired",
      "discarded",
      "abandoned",
      "notRecorded",
      "unknown",
      "conflict",
    ].includes(result.status)
  ) {
    return { request, status: "unknown" };
  }
  return result;
}
function reviewBasis(state: SharedRoutingState | undefined) {
  return image([state?.appId, state?.binding, state?.groups]);
}

/** Local draft intent only. All publication identities and outcomes remain native-owned. */
export function useSharedRouteWorkspace(appId: AppId) {
  const client = useQueryClient();
  const stateQuery = useQuery({
    queryKey: sharedRoutingKey(appId),
    queryFn: () => sharedRoutingApi.get(appId),
    retry: false,
  });
  const sourcesQuery = useQuery({
    queryKey: serviceSourcesKey,
    queryFn: serviceSourcesApi.list,
    retry: false,
  });
  const connectionQuery = useQuery({
    queryKey: ["applicationRouting", appId],
    queryFn: () => applicationRoutingApi.get(appId),
    retry: false,
  });
  const state = stateQuery.data?.appId === appId ? stateQuery.data : undefined;
  const error =
    stateQuery.isError ||
    sourcesQuery.isError ||
    connectionQuery.isError ||
    Boolean(stateQuery.data && !state);
  const [draft, setDraftState] = useState<RouteTarget | null>(null);
  const [prepared, setPrepared] = useState<PreparedRouteTarget | null>(null);
  const [requestResult, setRequestResult] = useState<RouteRequestResult | null>(
    null,
  );
  const [notice, setNotice] = useState<
    "saved" | "notSaved" | "unknown" | "changed" | "failed" | null
  >(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const intent = useRef(0);
  const lifecycle = useRef(0);
  const reviewedBasis = useRef<string | null>(null);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const preparedRef = useRef(prepared);
  preparedRef.current = prepared;
  useEffect(() => {
    lifecycle.current += 1;
    intent.current += 1;
    setDraftState(null);
    setPrepared(null);
    setRequestResult(null);
    setNotice(null);
    return () => {
      lifecycle.current += 1;
    };
  }, [appId]);
  const setDraft = (next: RouteTarget) => {
    intent.current += 1;
    setDraftState({ ...next });
    setPrepared(null);
    reviewedBasis.current = null;
    if (resolved(requestResult)) setNotice(null);
  };
  const discard = () => {
    intent.current += 1;
    setDraftState(null);
    setPrepared(null);
    reviewedBasis.current = null;
    if (resolved(requestResult)) setNotice(null);
  };
  const refresh = async () => {
    await Promise.all([
      client.invalidateQueries({ queryKey: sharedRoutingKey(appId) }),
      client.invalidateQueries({ queryKey: serviceSourcesKey }),
      client.invalidateQueries({ queryKey: ["applicationRouting", appId] }),
      client.invalidateQueries({ queryKey: ["applicationOverview", appId] }),
      client.invalidateQueries({ queryKey: ["providers", appId] }),
    ]);
  };
  const writable = () => {
    const current = client.getQueryState<SharedRoutingState>(
      sharedRoutingKey(appId),
    );
    return (
      current?.data?.appId === appId &&
      current.data.canApply === true &&
      current.data.blockedReason === null &&
      current.data.pendingRequest === null &&
      !current.error &&
      current.fetchStatus !== "fetching" &&
      [
        client.getQueryState(serviceSourcesKey),
        client.getQueryState(["applicationRouting", appId]),
      ].every(
        (query) =>
          query?.status === "success" &&
          query.data !== undefined &&
          !query.error &&
          query.fetchStatus !== "fetching",
      ) &&
      resolved(requestResult)
    );
  };
  const targetValid = (target: RouteTarget | null) =>
    Boolean(
      target &&
      (target.kind === "manual"
        ? target.sourceId.length > 0 && target.model.length > 0
        : target.groupId.length > 0),
    );
  const hasPendingChanges =
    draft !== null && image(draft) !== image(state?.binding?.target);
  const canPreview =
    state?.canApply === true &&
    state.blockedReason === null &&
    !state.pendingRequest &&
    !error &&
    !busy &&
    !stateQuery.isFetching &&
    !sourcesQuery.isFetching &&
    !connectionQuery.isFetching &&
    resolved(requestResult) &&
    hasPendingChanges &&
    targetValid(draft);
  const canConfirm = Boolean(
    prepared &&
    !busy &&
    writable() &&
    reviewedBasis.current === reviewBasis(state) &&
    image(prepared.target) === image(draft),
  );
  const prepare = async () => {
    const target = draftRef.current;
    if (inFlight.current || !writable() || !targetValid(target) || !target)
      return;
    inFlight.current = true;
    setBusy(true);
    setNotice(null);
    const generation = lifecycle.current,
      edit = intent.current,
      basis = reviewBasis(state);
    try {
      const result = await sharedRoutingApi.prepareTarget(appId, target);
      if (generation !== lifecycle.current || edit !== intent.current) return;
      if (
        image(result.target) !== image(target) ||
        result.request?.scope !== "applicationRouting" ||
        !result.request.id ||
        !result.request.draftDigest ||
        !result.request.revision ||
        basis !== reviewBasis(client.getQueryData(sharedRoutingKey(appId)))
      ) {
        setNotice("changed");
        return;
      }
      reviewedBasis.current = basis;
      setPrepared(result);
    } catch {
      if (generation === lifecycle.current) setNotice("failed");
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };
  const inspect = async (request: ApplicationRoutingRequest) => {
    try {
      return checkedResult(
        await sharedRoutingApi.queryTarget(appId, request),
        request,
      );
    } catch {
      return { request, status: "unknown" as const };
    }
  };
  const accept = async (
    result: RouteRequestResult,
    edit: number,
    generation: number,
  ) => {
    if (generation !== lifecycle.current) return;
    setRequestResult(result);
    setPrepared(null);
    if (result.status === "completed") {
      if (edit === intent.current) {
        setDraftState(null);
        intent.current += 1;
      }
      setNotice("saved");
    } else setNotice(resolved(result) ? "notSaved" : "unknown");
    await refresh();
  };
  const confirm = async () => {
    const review = preparedRef.current;
    if (inFlight.current || !review || !writable()) return;
    if (
      reviewedBasis.current !==
        reviewBasis(client.getQueryData(sharedRoutingKey(appId))) ||
      image(review.target) !== image(draftRef.current)
    ) {
      setPrepared(null);
      setNotice("changed");
      return;
    }
    inFlight.current = true;
    setBusy(true);
    const generation = lifecycle.current,
      edit = intent.current;
    // Consume this prepared submit once. An ambiguous reply never creates another write.
    setPrepared(null);
    preparedRef.current = null;
    try {
      let result: RouteRequestResult;
      try {
        result = checkedResult(
          await sharedRoutingApi.applyTarget(appId, review),
          review.request,
        );
      } catch {
        result = await inspect(review.request);
      }
      await accept(result, edit, generation);
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };
  const pendingRequest =
    state?.pendingRequest ??
    (!resolved(requestResult) ? requestResult?.request : null) ??
    null;
  const inspectRequest = async () => {
    if (inFlight.current || !pendingRequest) return;
    inFlight.current = true;
    setBusy(true);
    const generation = lifecycle.current,
      edit = intent.current;
    try {
      await accept(await inspect(pendingRequest), edit, generation);
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };
  const recoverableRequest = () => {
    const current = client.getQueryState<SharedRoutingState>(
      sharedRoutingKey(appId),
    );
    const connection = client.getQueryState<ApplicationRouting>([
      "applicationRouting",
      appId,
    ]);
    const inventory = client.getQueryState(serviceSourcesKey);
    const blocked = applicationRoutingErrorModeState(connection?.error);
    // A real pending journal blocks normal routing reads. That structured denial
    // is not permission to write a new route, but must not hide its original recovery.
    const knownConnection =
      connection?.status === "success" ||
      (connection?.status === "error" &&
        blocked?.status === "pending" &&
        blocked.blockedReason === "mode.verification_required");
    const original = current?.data?.pendingRequest;
    if (
      current?.status !== "success" ||
      current.data?.appId !== appId ||
      current.fetchStatus !== "idle" ||
      inventory?.status !== "success" ||
      inventory.fetchStatus !== "idle" ||
      connection?.fetchStatus !== "idle" ||
      !knownConnection ||
      !original ||
      (pendingRequest && !requestMatches(original, pendingRequest))
    )
      return null;
    return original;
  };
  const recoverRequest = async () => {
    // Re-read current native authority at invocation, including stale handlers.
    const original = recoverableRequest();
    if (inFlight.current || !original) return;
    inFlight.current = true;
    setBusy(true);
    const generation = lifecycle.current,
      edit = intent.current;
    try {
      let result: RouteRequestResult;
      try {
        result = checkedResult(
          await sharedRoutingApi.recoverTarget(appId, original),
          original,
        );
      } catch {
        result = await inspect(original);
      }
      await accept(result, edit, generation);
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };
  return {
    state,
    sources: sourcesQuery.data,
    connection: connectionQuery.error ? undefined : connectionQuery.data,
    // Native blocked reads reject with modeState. Reuse the original admissible
    // error projection; an arbitrary failed read must not become a fault authority.
    modeState:
      applicationRoutingErrorModeState(connectionQuery.error) ??
      (connectionQuery.error ? undefined : connectionQuery.data?.modeState),
    loading:
      stateQuery.isPending ||
      sourcesQuery.isPending ||
      connectionQuery.isPending,
    error,
    draft,
    setDraft,
    discard,
    hasPendingChanges,
    prepared,
    busy,
    canPreview,
    canConfirm,
    prepare,
    confirm,
    closeReview: () => setPrepared(null),
    requestResult,
    pendingRequest,
    inspectRequest,
    recoverRequest,
    canRecover: recoverableRequest() !== null,
    refresh,
    notice,
  };
}
