import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { workbuddyApi } from "@/lib/api/workbuddy";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const account = { id: "api-row", label: "Reviewed", realm: "www.workbuddy.cn" };
const ready = {
  flowId: "flow-a",
  available: true,
  state: "ready",
  account,
  saved: null,
  reason: null,
  expiresInMs: 1000,
};
beforeEach(() => vi.clearAllMocks());
it("prepares only the requested id and consumes backend summary", async () => {
  vi.mocked(invoke).mockResolvedValue({ ...ready, token: "not-retained" });
  expect(await workbuddyApi.prepareNativeAttachment("api-row")).toEqual(ready);
  expect(invoke).toHaveBeenCalledExactlyOnceWith(
    "prepare_workbuddy_native_attachment",
    { id: "api-row" },
  );
});
it.each(["confirm", "inspect", "cancel"] as const)(
  "%s retains exact flow and refuses another receipt",
  async (operation) => {
    vi.mocked(invoke).mockResolvedValue({ ...ready, flowId: "other" });
    await expect(
      workbuddyApi[`${operation}NativeAttachment`]("flow-a"),
    ).rejects.toMatchObject({ code: "invalidResponse" });
    expect(invoke).toHaveBeenCalledTimes(1);
  },
);
it.each([
  { ...ready, flowId: null },
  { ...ready, account: null },
  { ...ready, expiresInMs: -1 },
  { ...ready, state: "saved", saved: null },
  {
    ...ready,
    state: "saved",
    saved: {
      selection: {
        id: "other",
        generation: 1,
        incarnation: "d54c6503-ae85-4ec5-b591-6393e9d41930",
      },
      label: "Reviewed",
      realm: "www.workbuddy.cn",
    },
  },
  { ...ready, available: "true" },
  { ...ready, reason: "secret" },
  { ...ready, account: { ...account, realm: "evil.example" } },
])("rejects malformed or incoherent receipt %#", async (value) => {
  vi.mocked(invoke).mockResolvedValue(value);
  await expect(
    workbuddyApi.prepareNativeAttachment("api-row"),
  ).rejects.toMatchObject({ code: "invalidResponse" });
});
it("allows unowned current-slot discovery but never changes a request", async () => {
  vi.mocked(invoke).mockResolvedValue({
    flowId: null,
    available: false,
    state: "unknown",
    account: null,
    saved: null,
    reason: "unsupportedPlatform",
    expiresInMs: 0,
  });
  expect((await workbuddyApi.inspectNativeAttachment(null)).available).toBe(
    false,
  );
  expect(invoke).toHaveBeenCalledExactlyOnceWith(
    "inspect_workbuddy_native_attachment",
    { flowId: null },
  );
});
it("redacts transport secrets", async () => {
  vi.mocked(invoke).mockRejectedValue({ token: "secret" });
  const result = await workbuddyApi
    .confirmNativeAttachment("flow-a")
    .catch((x) => x);
  expect(result.code).toBe("unknown");
  expect(JSON.stringify(result)).not.toContain("secret");
});
