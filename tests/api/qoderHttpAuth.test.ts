import { beforeEach, expect, it, vi } from "vitest";
import {
  qoderHttpAuthApi,
  type QoderHttpIdentityContext,
} from "@/lib/api/qoderHttpAuth";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const context: QoderHttpIdentityContext = {
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
