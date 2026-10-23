import { beforeEach, expect, it, vi } from "vitest";
import {
  qoderHttpAuthApi,
  type QoderHttpIdentityContext,
} from "@/lib/api/qoderHttpAuth";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const context: QoderHttpIdentityContext = {
  source: "internationalQoderPat",
  providerId: "provider-a",
  credentialGeneration: "displayed-source-generation",
  status: "unestablished",
  expiresAtMs: null,
  allowedActions: ["establish"],
};

beforeEach(() => invoke.mockReset());

it("reads the original Provider through the real context command", async () => {
  invoke.mockResolvedValue(context);
  await expect(qoderHttpAuthApi.context("provider-a")).resolves.toEqual(
    context,
  );
  expect(invoke).toHaveBeenCalledExactlyOnceWith(
    "qoder_http_identity_context",
    {
      providerId: "provider-a",
    },
  );
});

it.each(["establish", "recover", "rebind"] as const)(
  "sends one strict %s request with only the displayed source generation",
  async (action) => {
    invoke.mockResolvedValue({ status: "cancelled" });
    // Even an accidental extra field at a call site cannot enter the request.
    const contaminated = {
      ...context,
      token: "SYNTHETIC-SECRET-NEVER-SEND",
      uid: "SYNTHETIC-UID",
      machineId: "SYNTHETIC-MACHINE",
      receipt: "SYNTHETIC-PRIVATE-RECEIPT",
      confirmed: true,
      origin: "https://example.invalid",
    };
    await expect(
      qoderHttpAuthApi.action(contaminated, action),
    ).resolves.toEqual({
      status: "cancelled",
    });
    expect(invoke).toHaveBeenCalledExactlyOnceWith(
      "qoder_http_identity_action",
      {
        request: {
          declaration: {
            providerId: "provider-a",
            credentialGeneration: "displayed-source-generation",
            source: "internationalQoderPat",
          },
          action,
        },
      },
    );
    expect(JSON.stringify(invoke.mock.calls)).not.toContain("SYNTHETIC-");
  },
);

it("preserves the safe closed-gate unavailable context without invoking an action", async () => {
  const unavailable: QoderHttpIdentityContext = {
    providerId: "provider-a",
    credentialGeneration: "",
    status: "unavailable",
    expiresAtMs: null,
    allowedActions: [],
  };
  invoke.mockResolvedValue(unavailable);
  await expect(qoderHttpAuthApi.context("provider-a")).resolves.toEqual(
    unavailable,
  );
  expect(invoke).toHaveBeenCalledTimes(1);
});

it("carries the backend CN source declaration without substituting international", async () => {
  invoke.mockResolvedValue({ status: "cancelled" });
  await qoderHttpAuthApi.action(
    { ...context, source: "chinaQoderPat" },
    "establish",
  );
  expect(invoke.mock.calls[0][1].request.declaration.source).toBe(
    "chinaQoderPat",
  );
});

it("refuses to invent a source declaration when backend context has no realm", async () => {
  invoke.mockResolvedValue({ status: "cancelled" });
  await expect(
    qoderHttpAuthApi.action({ ...context, source: null }, "establish"),
  ).rejects.toThrow();
  expect(invoke).not.toHaveBeenCalled();
});

it("keeps device-authorized identities out of the PAT exchange command", async () => {
  await expect(
    qoderHttpAuthApi.action(
      { ...context, source: "chinaQoderDevice" },
      "establish",
    ),
  ).rejects.toThrow();
  expect(invoke).not.toHaveBeenCalled();
});
