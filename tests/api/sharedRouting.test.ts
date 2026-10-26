import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  sharedRoutingApi,
  type PreparedGroup,
  type PreparedRouteTarget,
} from "@/lib/api/sharedRouting";

// Only the desktop IPC boundary is replaced. These tests prove wire behavior,
// not native journal persistence, authorization, routing, or successful forwarding.
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const receipt = Object.freeze({
  id: "00000000-0000-4000-8000-000000000001",
  scope: "applicationRouting" as const,
  draftDigest:
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  revision: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
});
const manual: PreparedRouteTarget = Object.freeze({
  target: Object.freeze({
    kind: "manual" as const,
    sourceId: "source-a",
    model: "model-a",
  }),
  request: receipt,
  expectedBindingRevision: 3,
  groupRevision: null,
});
const ordered: PreparedRouteTarget = Object.freeze({
  target: Object.freeze({ kind: "orderedGroup" as const, groupId: "group-id" }),
  request: receipt,
  expectedBindingRevision: 3,
  groupRevision: 7,
});
const group: PreparedGroup = {
  group: {
    id: "native-group-id",
    name: "Daily",
    revision: 8,
    members: [
      { sourceId: "source-b", model: "model-b" },
      { sourceId: "source-a", model: "model-a" },
    ],
  },
  affectedAppIds: ["claude", "codex"],
  expectedRevision: 7,
  draftDigest: "native-group-digest",
};
beforeEach(() => {
  invoke.mockReset();
});

describe("shared routing IPC boundary", () => {
  it("rejects a mismatched application response before consumers can adopt its facts or pending receipt", async () => {
    invoke.mockResolvedValue({
      appId: "codex",
      binding: null,
      groups: [],
      pendingRequest: receipt,
      canApply: true,
      blockedReason: null,
    });
    await expect(sharedRoutingApi.get("claude")).rejects.toThrow(
      "routing_groups.invalid_state",
    );
    expect(invoke.mock.calls).toEqual([
      ["get_shared_routing_state", { appType: "claude" }],
    ]);
  });
  it("keeps an unconfigured binding null and never initiates recovery on read", async () => {
    expect(sharedRoutingApi).toHaveProperty("get", expect.any(Function));
    invoke.mockResolvedValue({
      appId: "claude",
      binding: null,
      groups: [],
      pendingRequest: null,
      canApply: false,
      blockedReason: "routing_groups.connection_required",
    });
    expect(await sharedRoutingApi.get("claude")).toEqual({
      appId: "claude",
      binding: null,
      groups: [],
      pendingRequest: null,
      canApply: false,
      blockedReason: "routing_groups.connection_required",
    });
    expect(invoke.mock.calls).toEqual([
      ["get_shared_routing_state", { appType: "claude" }],
    ]);
  });
  it.each([manual, ordered])(
    "prepares only the explicit target, without manufacturing a native receipt",
    async (prepared) => {
      expect(sharedRoutingApi).toHaveProperty(
        "prepareTarget",
        expect.any(Function),
      );
      invoke.mockResolvedValue(prepared);
      expect(
        await sharedRoutingApi.prepareTarget("claude", prepared.target),
      ).toBe(prepared);
      expect(invoke.mock.calls).toEqual([
        [
          "prepare_application_route_target",
          { appType: "claude", target: prepared.target },
        ],
      ]);
    },
  );
  it.each([manual, ordered])(
    "confirms the exact reviewed receipt and group revision, without connecting or re-preparing",
    async (prepared) => {
      expect(sharedRoutingApi).toHaveProperty(
        "applyTarget",
        expect.any(Function),
      );
      invoke.mockResolvedValue({ request: receipt, status: "pending" });
      expect(await sharedRoutingApi.applyTarget("claude", prepared)).toEqual({
        request: receipt,
        status: "pending",
      });
      expect(invoke.mock.calls).toEqual([
        ["apply_application_route_target", { appType: "claude", prepared }],
      ]);
    },
  );
  it.each(["unknown", "notRecorded", "conflict", "verificationRequired"])(
    "query retains %s instead of promoting it to success or retrying",
    async (status) => {
      expect(sharedRoutingApi).toHaveProperty(
        "queryTarget",
        expect.any(Function),
      );
      invoke.mockResolvedValue({ request: receipt, status });
      expect(await sharedRoutingApi.queryTarget("codex", receipt)).toEqual({
        request: receipt,
        status,
      });
      expect(invoke.mock.calls).toEqual([
        [
          "get_application_route_target_result",
          { appType: "codex", request: receipt },
        ],
      ]);
    },
  );
  it.each([
    "routing_groups.conflict",
    "routing_groups.invalid",
    "routing_groups.unavailable",
  ])(
    "propagates %s without fallback, new receipt, or optimistic success",
    async (code) => {
      expect(sharedRoutingApi).toHaveProperty(
        "applyTarget",
        expect.any(Function),
      );
      invoke.mockRejectedValue(code);
      await expect(sharedRoutingApi.applyTarget("claude", manual)).rejects.toBe(
        code,
      );
      expect(invoke).toHaveBeenCalledTimes(1);
    },
  );
  it("recovers only the explicitly selected application's original receipt", async () => {
    expect(sharedRoutingApi).toHaveProperty(
      "recoverTarget",
      expect.any(Function),
    );
    invoke.mockResolvedValue({ request: receipt, status: "completed" });
    await sharedRoutingApi.recoverTarget("claude", receipt);
    expect(invoke.mock.calls).toEqual([
      [
        "recover_application_route_target",
        { appType: "claude", request: receipt },
      ],
    ]);
  });
  it("previews a new group without allocating client IDs or mutating local revision", async () => {
    expect(sharedRoutingApi).toHaveProperty(
      "prepareGroup",
      expect.any(Function),
    );
    const draft = {
      id: null,
      expectedRevision: null,
      name: "Daily",
      members: [{ sourceId: "source-a", model: "model-a" }],
    };
    invoke.mockResolvedValue(group);
    expect(await sharedRoutingApi.prepareGroup(draft)).toBe(group);
    expect(invoke.mock.calls).toEqual([
      ["prepare_shared_routing_group", { draft }],
    ]);
    expect(draft.id).toBeNull();
    expect(draft.expectedRevision).toBeNull();
  });
  it("publishes one global ordered group CAS, not per-application copies", async () => {
    expect(sharedRoutingApi).toHaveProperty(
      "confirmGroup",
      expect.any(Function),
    );
    invoke.mockResolvedValue(group.group);
    expect(await sharedRoutingApi.confirmGroup(group)).toEqual({
      id: "native-group-id",
      name: "Daily",
      revision: 8,
      members: [
        { sourceId: "source-b", model: "model-b" },
        { sourceId: "source-a", model: "model-a" },
      ],
    });
    expect(invoke.mock.calls).toEqual([
      ["confirm_shared_routing_group", { prepared: group }],
    ]);
  });
  it("leaves an ambiguous group publication rejected, without replay or new UUID", async () => {
    expect(sharedRoutingApi).toHaveProperty(
      "confirmGroup",
      expect.any(Function),
    );
    invoke.mockRejectedValue(new Error("transport lost"));
    await expect(sharedRoutingApi.confirmGroup(group)).rejects.toThrow(
      "transport lost",
    );
    expect(invoke.mock.calls).toEqual([
      ["confirm_shared_routing_group", { prepared: group }],
    ]);
  });
});
