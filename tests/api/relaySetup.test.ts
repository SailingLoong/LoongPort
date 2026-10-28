import { beforeEach, expect, it, vi } from "vitest";
import wire from "../fixtures/relay-setup-wire.json";
import { relayApi } from "@/lib/api/relay";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() => {
  invoke.mockReset();
});
const operationId = "f8d02319-67c1-426b-a931-c836de11320e";

it("sends the complete confirmed operation, not an unjournaled relay/selections pair", async () => {
  const request = {
    kind: "start" as const,
    operationId,
    relayId: 7,
    discoveryRevision: "revision-1",
    selections: [{ app: "codex" as const, groupId: "a" }],
  };
  invoke.mockResolvedValue({ operationId });
  await relayApi.prepareTiers(request);
  expect(invoke.mock.calls).toEqual([["relay_prepare_tiers", { request }]]);
});
it("resumes only explicit failed apps of the existing receipt revision", async () => {
  const request = {
    kind: "resume" as const,
    operationId,
    expectedReceiptRevision: 4,
    apps: ["codex" as const],
  };
  invoke.mockResolvedValue({ operationId });
  await relayApi.prepareTiers(request);
  expect(invoke.mock.calls).toEqual([["relay_prepare_tiers", { request }]]);
});
it("queries and reconciles the same operation without silently retrying preparation", async () => {
  invoke.mockResolvedValue({ operationId });
  await relayApi.getSetupOperation(7, operationId);
  await relayApi.reconcileSetupOperation(operationId);
  expect(invoke.mock.calls).toEqual([
    [
      "relay_get_setup_operation",
      { relayId: 7, operationId, historyOffset: undefined },
    ],
    ["relay_reconcile_setup_operation", { operationId }],
  ]);
});
it("discovery has no prepare or refresh fallback when the read fails", async () => {
  invoke.mockRejectedValue(new Error("fixture read failure"));
  await expect(relayApi.discoverTiers(7)).rejects.toThrow(
    "fixture read failure",
  );
  expect(invoke.mock.calls).toEqual([["relay_discover_tiers", { relayId: 7 }]]);
});

it("consumes the fixture checked against actual native serialization without inferring key absence", async () => {
  invoke
    .mockResolvedValueOnce(wire.discovery)
    .mockResolvedValueOnce(wire.receipt);
  const discovery = await relayApi.discoverTiers(7);
  const receipt = await relayApi.getSetupOperation(7, operationId);
  expect(discovery.account.identity).toBe(wire.receipt.accountIdentity);
  expect(discovery.choices[0].localConfiguration).toBe("absent");
  expect(discovery.choices[0].keyStatus).toBe("unknown");
  expect(receipt!.targets[0].connection.state).toBe("notRequested");
  expect(receipt!.createdKeyCount).toBe(0);
  expect(JSON.stringify(receipt)).not.toMatch(
    /apiKey|authToken|settingsConfig/,
  );
});

it("reviews and applies a prepared target through the durable original-routing association", async () => {
  await relayApi.prepareApplicationConnection(operationId, "codex", 5, {
    choice: "keep",
    revision: "catalog-r",
  });
  await relayApi.applyPreparedTier(operationId, "codex", 6, false);
  expect(invoke.mock.calls).toEqual([
    [
      "relay_prepare_application_connection",
      {
        operationId,
        targetApp: "codex",
        expectedReceiptRevision: 5,
        catalogDecision: { choice: "keep", revision: "catalog-r" },
      },
    ],
    [
      "relay_apply_prepared_tier",
      {
        operationId,
        targetApp: "codex",
        expectedReceiptRevision: 6,
        quitChatgpt: false,
      },
    ],
  ]);
});
