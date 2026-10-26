import { invoke } from "@tauri-apps/api/core";
import type {
  ApplicationRoutingRequest,
  ApplicationRoutingRequestResult,
} from "./applicationRouting";

/** Routing policy within the gateway; neither variant means native direct mode. */
export type RouteTarget =
  | { kind: "manual"; sourceId: string; model: string }
  | { kind: "orderedGroup"; groupId: string };
export interface SharedGroupMember {
  sourceId: string;
  model: string;
}
export interface SharedGroup {
  id: string;
  name: string;
  revision: number;
  members: SharedGroupMember[];
}
export interface RouteBinding {
  revision: number;
  target: RouteTarget;
}
export interface SharedRoutingState {
  appId: string;
  /** Null is unconfigured/legacy, not an implicit manual or automatic target. */
  binding: RouteBinding | null;
  groups: SharedGroup[];
  /** Only this route-target operation's original app-local pending journal receipt. */
  pendingRequest: ApplicationRoutingRequest | null;
  canApply: boolean;
  blockedReason: string | null;
}
export interface GroupDraft {
  id: string | null;
  expectedRevision: number | null;
  name: string;
  members: SharedGroupMember[];
}
export interface PreparedGroup {
  group: SharedGroup;
  expectedRevision: number | null;
  draftDigest: string;
  affectedAppIds: string[];
}
export interface PreparedRouteTarget {
  target: RouteTarget;
  request: ApplicationRoutingRequest;
  expectedBindingRevision: number | null;
  groupRevision: number | null;
}
export type RouteRequestResult = ApplicationRoutingRequestResult;

/** Native owners issue all IDs, digests, revisions and publication status.
 * No retries: ambiguous writes are resolved by explicit reads/query/recovery.
 */
export const sharedRoutingApi = {
  get: async (appType: string): Promise<SharedRoutingState> => {
    const state = await invoke<SharedRoutingState>("get_shared_routing_state", {
      appType,
    });
    // Refuse cross-application facts before either overview or editor caches them.
    if (state?.appId !== appType)
      throw new Error("routing_groups.invalid_state");
    return state;
  },
  prepareGroup: (draft: GroupDraft): Promise<PreparedGroup> =>
    invoke("prepare_shared_routing_group", { draft }),
  confirmGroup: (prepared: PreparedGroup): Promise<SharedGroup> =>
    invoke("confirm_shared_routing_group", { prepared }),
  prepareTarget: (
    appType: string,
    target: RouteTarget,
  ): Promise<PreparedRouteTarget> =>
    invoke("prepare_application_route_target", { appType, target }),
  applyTarget: (
    appType: string,
    prepared: PreparedRouteTarget,
  ): Promise<RouteRequestResult> =>
    invoke("apply_application_route_target", { appType, prepared }),
  queryTarget: (
    appType: string,
    request: ApplicationRoutingRequest,
  ): Promise<RouteRequestResult> =>
    invoke("get_application_route_target_result", { appType, request }),
  recoverTarget: (
    appType: string,
    request: ApplicationRoutingRequest,
  ): Promise<RouteRequestResult> =>
    invoke("recover_application_route_target", { appType, request }),
};
