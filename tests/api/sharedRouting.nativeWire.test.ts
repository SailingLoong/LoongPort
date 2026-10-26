import { beforeEach, expect, it, vi } from "vitest";
import serialized from "../fixtures/shared-routing-wire.json";
import {
  sharedRoutingApi,
  type PreparedGroup,
  type PreparedRouteTarget,
  type RouteRequestResult,
  type SharedRoutingState,
} from "@/lib/api/sharedRouting";

// The Rust owner emits this frozen artifact with serde_json::to_value of the
// actual native DTOs; its integration test compares native output to this file.
// This test exercises JS consumption/round-trip only, not native publication.
const wire = serialized as {
  manual: PreparedRouteTarget;
  ordered: PreparedRouteTarget;
  preparedGroup: PreparedGroup;
  emptyState: SharedRoutingState;
  pendingState: SharedRoutingState;
  completed: RouteRequestResult;
};
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() => {
  invoke.mockReset();
});

it("consumes native manual/null and ordered/revision tags without inferring direct mode", async () => {
  expect(wire).toHaveProperty("manual.target.kind", "manual");
  expect(wire).toHaveProperty("ordered.target.kind", "orderedGroup");
  invoke.mockResolvedValueOnce(wire.manual).mockResolvedValueOnce(wire.ordered);
  const manual = await sharedRoutingApi.prepareTarget(
    "claude",
    wire.manual.target,
  );
  const ordered = await sharedRoutingApi.prepareTarget(
    "claude",
    wire.ordered.target,
  );
  expect(manual.target).toEqual({
    kind: "manual",
    sourceId: "synthetic-source",
    model: "model-a",
  });
  expect(manual.expectedBindingRevision).toBeNull();
  expect(manual.groupRevision).toBeNull();
  expect(ordered.target).toEqual({
    kind: "orderedGroup",
    groupId: "00000000-0000-4000-8000-000000000002",
  });
  expect(ordered.expectedBindingRevision).toBe(1);
  expect(ordered.groupRevision).toBe(2);
});

it("round-trips the actual application-scoped native receipt without provider credentials", async () => {
  expect(wire).toHaveProperty("manual.request.scope", "applicationRouting");
  invoke.mockResolvedValue(wire.completed);
  const result = await sharedRoutingApi.applyTarget("claude", wire.manual);
  expect(invoke.mock.calls).toEqual([
    [
      "apply_application_route_target",
      { appType: "claude", prepared: serialized.manual },
    ],
  ]);
  expect(result).toEqual({
    request: {
      id: "00000000-0000-4000-8000-000000000001",
      scope: "applicationRouting",
      draftDigest: "a".repeat(64),
      revision: "b".repeat(64),
    },
    status: "completed",
  });
  expect(Object.keys(result.request).sort()).toEqual([
    "draftDigest",
    "id",
    "revision",
    "scope",
  ]);
});

it("reads native missing binding and original pending journal as distinct states", async () => {
  expect(wire).toHaveProperty("emptyState.pendingRequest", null);
  invoke
    .mockResolvedValueOnce(wire.emptyState)
    .mockResolvedValueOnce(wire.pendingState);
  const empty = await sharedRoutingApi.get("claude");
  const pending = await sharedRoutingApi.get("claude");
  expect(empty.binding).toBeNull();
  expect(empty.canApply).toBe(false);
  expect(empty.blockedReason).toBe("routing_groups.connection_required");
  expect(pending.binding).toEqual({
    revision: 1,
    target: { kind: "manual", sourceId: "synthetic-source", model: "model-a" },
  });
  expect(pending.blockedReason).toBe("routing_groups.recovery_required");
  expect(pending.pendingRequest).toEqual(wire.manual.request);
  expect(invoke).toHaveBeenCalledTimes(2);
});

it("preserves native affected applications and the one global group's reviewed CAS", async () => {
  expect(wire).toHaveProperty("preparedGroup.affectedAppIds");
  invoke.mockResolvedValue(wire.preparedGroup.group);
  const result = await sharedRoutingApi.confirmGroup(wire.preparedGroup);
  expect(invoke.mock.calls).toEqual([
    ["confirm_shared_routing_group", { prepared: serialized.preparedGroup }],
  ]);
  expect(wire.preparedGroup.affectedAppIds).toEqual(["claude", "codex"]);
  expect(wire.preparedGroup.expectedRevision).toBe(1);
  expect(result.revision).toBe(2);
  expect(result.members).toEqual([
    { sourceId: "synthetic-source", model: "model-a" },
  ]);
});
