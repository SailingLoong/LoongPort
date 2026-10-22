import { beforeEach, expect, it, vi } from "vitest";
import { qoderCnDeviceLoginApi } from "@/lib/api/qoderCnDeviceLogin";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() => invoke.mockReset().mockResolvedValue({ status: "cancelled" }));

it.each([
  { providerId: null, credentialGeneration: null, rebind: false },
  {
    providerId: "original-cn",
    credentialGeneration: "original-revision",
    rebind: false,
  },
  {
    providerId: "original-cn",
    credentialGeneration: "original-revision",
    rebind: true,
  },
])("sends only the explicit original binding for %j", async (request) => {
  const contaminated = {
    ...request,
    token: "SYNTHETIC-PRIVATE-TOKEN",
    uid: "SYNTHETIC-PRIVATE-UID",
    verifier: "SYNTHETIC-PRIVATE-VERIFIER",
    region: "international",
    machineId: "SYNTHETIC-PRIVATE-MACHINE",
    authUrl: "https://attacker.invalid",
  };
  await qoderCnDeviceLoginApi.start(contaminated);
  expect(invoke).toHaveBeenCalledExactlyOnceWith(
    "qoder_cn_device_login_start",
    { request },
  );
  expect(JSON.stringify(invoke.mock.calls)).not.toContain("SYNTHETIC-PRIVATE");
});
it("polls exactly one public session handle without supplying credentials or region", async () => {
  await qoderCnDeviceLoginApi.poll("opaque-session");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("qoder_cn_device_login_poll", {
    sessionId: "opaque-session",
  });
});
it("cancels exactly one public session handle", async () => {
  await expect(qoderCnDeviceLoginApi.cancel("opaque-session")).resolves.toEqual(
    { status: "cancelled" },
  );
  expect(invoke).toHaveBeenCalledExactlyOnceWith(
    "qoder_cn_device_login_cancel",
    { sessionId: "opaque-session" },
  );
});
