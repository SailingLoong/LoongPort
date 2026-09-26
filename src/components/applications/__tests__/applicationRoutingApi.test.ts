import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
beforeEach(() => vi.resetAllMocks());
it("sends profile identity, complete model selection and confirmation as one command", async () => {
  vi.mocked(invoke).mockResolvedValue({
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
  });
});
