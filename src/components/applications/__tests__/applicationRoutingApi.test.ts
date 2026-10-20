import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
beforeEach(() => vi.resetAllMocks());
it("sends profile identity, complete model selection and confirmation as one command", async () => {
  vi.mocked(invoke)
    .mockResolvedValueOnce({
      protocol: "legacy",
      review: { draftDigest: "a", revision: "b" },
    })
    .mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Example",
    });
  const change = {
    order: { profileName: "Travel", providerIds: ["b", "a"] },
    selection: { providerId: "b", model: "model-b" },
  };
  await expect(
    applicationRoutingApi.apply("codex", change, false),
  ).resolves.toEqual({ status: "confirmationRequired", targetName: "Example" });
  expect(invoke).toHaveBeenCalledWith("apply_application_routing", {
    appType: "codex",
    change,
    quitChatgpt: false,
    legacyReview: { draftDigest: "a", revision: "b" },
  });
});

it.each(["legacy", "native"] as const)(
  "prepares a bare explicit commit once for %s and sends only its credential",
  async (protocol) => {
    const review = { draftDigest: "a".repeat(64), revision: "b".repeat(64) };
    const request = { ...review, id: "native", scope: "applicationRouting" };
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "prepare_application_routing_request"
        ? protocol === "legacy"
          ? { protocol, review }
          : { protocol, request }
        : { status: "switched", warnings: [] },
    );
    const change = { selection: { providerId: "b" } };
    await applicationRoutingApi.apply("codex", change, false);
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke).toHaveBeenNthCalledWith(
      1,
      "prepare_application_routing_request",
      { appType: "codex", change, requestId: expect.any(String) },
    );
    expect(invoke).toHaveBeenNthCalledWith(2, "apply_application_routing", {
      appType: "codex",
      change,
      quitChatgpt: false,
      ...(protocol === "legacy" ? { legacyReview: review } : { request }),
    });
  },
);
it("keeps catalog preview read-only with no prepare", async () => {
  const change = { selection: { providerId: "b" }, catalogPreviewOnly: true };
  await applicationRoutingApi.apply("codex", change);
  expect(invoke).toHaveBeenCalledExactlyOnceWith("apply_application_routing", {
    appType: "codex",
    change,
    quitChatgpt: undefined,
  });
});
it("passes a prepared legacy review once and never overwrites its revision", async () => {
  const review = { draftDigest: "a".repeat(64), revision: "old-reviewed" };
  const change = { selection: { providerId: "b" } };
  await applicationRoutingApi.apply("codex", change, undefined, {
    protocol: "legacy",
    review,
  });
  expect(invoke).toHaveBeenCalledExactlyOnceWith("apply_application_routing", {
    appType: "codex",
    change,
    quitChatgpt: undefined,
    legacyReview: review,
  });
});
