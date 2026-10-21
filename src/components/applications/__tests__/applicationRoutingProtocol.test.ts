import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  applicationRoutingApi,
  isApplicationRoutingWriteBlocked,
} from "@/lib/api/applicationRouting";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
beforeEach(() => vi.resetAllMocks());
it.each([
  {},
  {
    id: "old-native",
    scope: "applicationRouting",
    draftDigest: "a",
    revision: "b",
  },
  { protocol: "native" },
  { protocol: "legacy" },
])(
  "rejects prepare output without its explicit protocol credential: %j",
  async (prepared) => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "prepare_application_routing_request"
        ? prepared
        : { status: "switched" },
    );
    await expect(
      applicationRoutingApi.apply("codex", { selection: { providerId: "b" } }),
    ).rejects.toThrow("mode.invalid_save_request");
    expect(invoke).toHaveBeenCalledTimes(1);
  },
);
it("does not reinterpret an explicit blocked reason as writable legacy", () => {
  expect(
    isApplicationRoutingWriteBlocked("codex", {
      status: "ready",
      canWrite: true,
      writeProtocol: "legacy",
      blockedReason: "upgrade.checkpoint_pending",
    } as any),
  ).toBe(true);
});
it("never switches protocols or retries after a failed legacy apply", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "prepare_application_routing_request")
      return {
        protocol: "legacy",
        review: { draftDigest: "a", revision: "b" },
      };
    throw new Error("upgrade.source_changed");
  });
  await expect(
    applicationRoutingApi.apply("codex", { selection: { providerId: "b" } }),
  ).rejects.toThrow("upgrade.source_changed");
  expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
    "prepare_application_routing_request",
    "apply_application_routing",
  ]);
});
