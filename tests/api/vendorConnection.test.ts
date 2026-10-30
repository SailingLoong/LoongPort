import { beforeEach, expect, expectTypeOf, it, vi } from "vitest";
import { vendorApi } from "@/lib/api/vendor";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() => {
  invoke.mockReset();
});

const operationId = "f8d02319-67c1-426b-a931-c836de11320e";
const input = {
  rowId: 7,
  plan: "standard",
  app: "codex" as const,
  operationId,
};
const prepared = {
  ...input,
  providerId: "provider-7",
  receiptRevision: 1,
  status: "prepared",
  operationState: "prepared",
  completedConfigurationRevision: null,
};
const completed = {
  ...prepared,
  status: "completed",
  operationState: "completed",
  receiptRevision: 3,
  completedConfigurationRevision: "native:catalog",
};

it("sends flat prepare fields and accepts the matching prepared receipt", async () => {
  invoke.mockResolvedValue(prepared);
  expect(
    await vendorApi.prepareConnection({ ...input, catalogDecision: null }),
  ).toEqual(prepared);
  expect(invoke.mock.calls).toEqual([
    ["vendor_prepare_connection", { ...input, catalogDecision: null }],
  ]);
});
it.each(["keep", "manage"] as const)(
  "preserves the reviewed %s catalog decision",
  async (choice) => {
    invoke.mockResolvedValue(prepared);
    const catalogDecision = { choice, revision: "catalog-r1" };
    await vendorApi.prepareConnection({ ...input, catalogDecision });
    expect(invoke.mock.calls).toEqual([
      ["vendor_prepare_connection", { ...input, catalogDecision }],
    ]);
  },
);
it("queries original UUID or null using flat fields", async () => {
  invoke
    .mockResolvedValueOnce(completed)
    .mockResolvedValueOnce({ status: "none" });
  expect(await vendorApi.queryConnection("codex", operationId)).toEqual(
    completed,
  );
  expect(await vendorApi.queryConnection("claude", null)).toEqual({
    status: "none",
  });
  expect(invoke.mock.calls).toEqual([
    ["vendor_query_connection", { app: "codex", operationId }],
    ["vendor_query_connection", { app: "claude", operationId: null }],
  ]);
});
it("preserves a null completed configuration revision without inventing one", async () => {
  const receipt = { ...completed, completedConfigurationRevision: null };
  invoke.mockResolvedValue(receipt);
  expect(await vendorApi.queryConnection("codex", operationId)).toEqual(
    receipt,
  );
});
it.each(["prepared", "applying", "unknown"])(
  "preserves query unknown operationState %s",
  async (operationState) => {
    const receipt = { ...prepared, status: "unknown", operationState };
    invoke.mockResolvedValue(receipt);
    expect(await vendorApi.queryConnection("codex", operationId)).toEqual(
      receipt,
    );
  },
);
it("keeps a redacted unknown and UUID-missing unknown uncertain", async () => {
  invoke
    .mockResolvedValueOnce({
      status: "unknown",
      operationId,
      app: "codex",
      receiptRevision: 2,
    })
    .mockResolvedValueOnce({ status: "unknown" });
  expect(await vendorApi.queryConnection("codex", operationId)).toEqual({
    status: "unknown",
    operationId,
    app: "codex",
    receiptRevision: 2,
  });
  expect(await vendorApi.queryConnection("codex", operationId)).toEqual({
    status: "unknown",
  });
});
it.each([
  { operationId: "wrong" },
  { app: "claude" },
  { rowId: 8 },
  { plan: "other" },
])("does not accept a mismatching prepare identity %#", async (change) => {
  invoke.mockResolvedValue({ ...prepared, ...change });
  expect(
    await vendorApi.prepareConnection({ ...input, catalogDecision: null }),
  ).toEqual({ status: "unknown" });
});
it.each([{ operationId: "wrong" }, { app: "claude" }])(
  "does not accept a mismatching query identity %#",
  async (change) => {
    invoke.mockResolvedValue({ ...completed, ...change });
    expect(await vendorApi.queryConnection("codex", operationId)).toEqual({
      status: "unknown",
    });
  },
);
it("does not treat UUID-specific none as absent or successful", async () => {
  invoke.mockResolvedValue({ status: "none" });
  expect(await vendorApi.queryConnection("codex", operationId)).toEqual({
    status: "unknown",
  });
});
it.each([
  { ...completed, status: "future" },
  { ...completed, operationState: "future" },
  { ...completed, receiptRevision: "3" },
  { ...completed, providerId: null },
  { ...completed, completedConfigurationRevision: undefined },
  null,
  [],
])(
  "fails closed on unrecognized or malformed query responses %#",
  async (response) => {
    invoke.mockResolvedValue(response);
    expect(await vendorApi.queryConnection("codex", operationId)).toEqual({
      status: "unknown",
    });
  },
);
it("preserves a matching completed receipt from a repeated prepare without calling it prepared", async () => {
  invoke.mockResolvedValue(completed);
  expect(
    await vendorApi.prepareConnection({ ...input, catalogDecision: null }),
  ).toEqual(completed);
});
it("propagates failed query and prepare without retry or legacy switch", async () => {
  const error = new Error("IPC failed");
  invoke.mockRejectedValue(error);
  await expect(vendorApi.queryConnection("codex", operationId)).rejects.toBe(
    error,
  );
  await expect(
    vendorApi.prepareConnection({ ...input, catalogDecision: null }),
  ).rejects.toBe(error);
  expect(invoke.mock.calls).toEqual([
    ["vendor_query_connection", { app: "codex", operationId }],
    ["vendor_prepare_connection", { ...input, catalogDecision: null }],
  ]);
});
it("returns a narrowed conservative result rather than arbitrary IPC data", () => {
  expectTypeOf<
    Awaited<ReturnType<typeof vendorApi.queryConnection>>
  >().toMatchTypeOf<{ status: "unknown" | "none" | "completed" }>();
});

it("does not accept a decorated or foreign none as app-wide absence", async () => {
  invoke.mockResolvedValue({ status: "none", app: "claude" });
  expect(await vendorApi.queryConnection("codex", null)).toEqual({
    status: "unknown",
  });
});
it.each([
  { status: "unknown", operationId: "other" },
  { status: "unknown", app: "claude" },
  { status: "unknown", receiptRevision: -1 },
])("strips invalid redacted identity %#", async (response) => {
  invoke.mockResolvedValue(response);
  expect(await vendorApi.queryConnection("codex", operationId)).toEqual({
    status: "unknown",
  });
});

it.each(["applying", "unknown", "failedBeforeWrite", "reviewed"] as const)(
  "preserves same-UUID prepare state %s conservatively",
  async (operationState) => {
    const receipt = { ...prepared, status: operationState, operationState };
    invoke.mockResolvedValue(receipt);
    expect(
      await vendorApi.prepareConnection({ ...input, catalogDecision: null }),
    ).toEqual({ ...receipt, status: "unknown" });
  },
);
it.each(["failedBeforeWrite", "reviewed"] as const)(
  "preserves query state %s without granting retry or success",
  async (operationState) => {
    const receipt = { ...prepared, status: operationState, operationState };
    invoke.mockResolvedValue(receipt);
    expect(await vendorApi.queryConnection("codex", operationId)).toEqual({
      ...receipt,
      status: "unknown",
    });
  },
);
it("sends prepared switch as flat identity, revision and explicit null quit flag", async () => {
  invoke.mockResolvedValue({ status: "unknown" });
  const switchInput = { ...input, expectedRevision: 2, quitChatgpt: null };
  expect(await vendorApi.switchPreparedConnection(switchInput)).toEqual({
    status: "unknown",
  });
  expect(invoke.mock.calls).toEqual([["vendor_switch", switchInput]]);
});
it("keeps an explicit true quit flag and never retries a rejected switch", async () => {
  const error = new Error("lost response");
  invoke.mockRejectedValue(error);
  const switchInput = { ...input, expectedRevision: 2, quitChatgpt: true };
  await expect(vendorApi.switchPreparedConnection(switchInput)).rejects.toBe(
    error,
  );
  expect(invoke.mock.calls).toEqual([["vendor_switch", switchInput]]);
});
it("fails closed on an unrecognized switch reply instead of asserting a write", async () => {
  invoke.mockResolvedValue({ status: "future-success" });
  const switchInput = { ...input, expectedRevision: 2, quitChatgpt: false };
  expect(await vendorApi.switchPreparedConnection(switchInput)).toEqual({
    status: "unknown",
  });
});

it.each([
  { catalog: { choices: ["keep", "manage"] } },
  { quitRequired: true },
  { confirmationRequired: true },
  { review: { required: true } },
  { futureProtection: true },
])(
  "does not strip an unrecognized prepare field and authorize a write %#",
  async (extra) => {
    invoke.mockResolvedValue({ ...prepared, ...extra });
    expect(
      await vendorApi.prepareConnection({ ...input, catalogDecision: null }),
    ).toEqual({ status: "unknown" });
    expect(invoke).toHaveBeenCalledTimes(1);
  },
);
it("does not promote a completed receipt with unrecognized protection fields", async () => {
  invoke.mockResolvedValue({ ...completed, review: { required: true } });
  expect(await vendorApi.queryConnection("codex", operationId)).toEqual({
    status: "unknown",
  });
});

const switched = {
  status: "switched",
  providerName: "Provider",
  chatgptWasRunning: true,
  chatgptRelaunched: false,
  warnings: ["relaunch failed"],
};
const catalog = {
  ownership: "configuredExternal",
  choice: "keep",
  canManage: true,
  canRestore: false,
  revision: "rev1",
  beforePointer: null,
  afterPointer: "a",
};
const switchInput = { ...input, quitChatgpt: null, expectedRevision: 1 };
it("returns exact switched feedback without asserting persistent completion", async () => {
  invoke.mockResolvedValue(switched);
  expect(await vendorApi.switchPreparedConnection(switchInput)).toEqual(
    switched,
  );
  expect(invoke.mock.calls).toEqual([["vendor_switch", switchInput]]);
});
it("preserves the confirmation gate and complete catalog preview", async () => {
  const confirmation = {
    status: "confirmationRequired",
    targetName: "Provider",
    quitRequired: true,
    catalog,
  };
  invoke.mockResolvedValue(confirmation);
  expect(await vendorApi.switchPreparedConnection(switchInput)).toEqual(
    confirmation,
  );
});
it("accepts confirmation without a catalog preview", async () => {
  const confirmation = {
    status: "confirmationRequired",
    targetName: "Provider",
    quitRequired: false,
  };
  invoke.mockResolvedValue(confirmation);
  expect(await vendorApi.switchPreparedConnection(switchInput)).toEqual(
    confirmation,
  );
});
it("returns matching completed receipt on repeated switch without invented feedback", async () => {
  invoke.mockResolvedValue(completed);
  expect(await vendorApi.switchPreparedConnection(switchInput)).toEqual(
    completed,
  );
});
it.each([
  { ...completed, rowId: 8 },
  { ...completed, app: "claude" },
  { ...completed, plan: "wrong" },
  { ...completed, operationId: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa" },
])(
  "does not accept a switch receipt for another intent %#",
  async (receipt) => {
    invoke.mockResolvedValue(receipt);
    expect(await vendorApi.switchPreparedConnection(switchInput)).toEqual({
      status: "unknown",
    });
  },
);
it("retains the exact original operation ID for a matching unknown switch response", async () => {
  invoke.mockResolvedValue({ status: "unknown", operationId });
  expect(await vendorApi.switchPreparedConnection(switchInput)).toEqual({
    status: "unknown",
    operationId,
  });
});
it("keeps requestResolved conservative without fabricating feedback", async () => {
  invoke.mockResolvedValue({
    status: "requestResolved",
    result: { status: "completed" },
  });
  expect(await vendorApi.switchPreparedConnection(switchInput)).toEqual({
    status: "unknown",
  });
});
it.each([
  { ...switched, warnings: [9] },
  { ...switched, chatgptRelaunched: "yes" },
  { ...switched, futureProtection: true },
  { status: "confirmationRequired", targetName: "Provider" },
  {
    status: "confirmationRequired",
    targetName: "Provider",
    quitRequired: true,
    catalog: { ...catalog, choice: "future" },
  },
  {
    status: "confirmationRequired",
    targetName: "Provider",
    quitRequired: true,
    futureProtection: true,
  },
])(
  "does not promote malformed or future switch output %#",
  async (response) => {
    invoke.mockResolvedValue(response);
    expect(await vendorApi.switchPreparedConnection(switchInput)).toEqual({
      status: "unknown",
    });
  },
);
it("does not accept receipt revision zero", async () => {
  invoke.mockResolvedValue({ ...prepared, receiptRevision: 0 });
  expect(
    await vendorApi.prepareConnection({ ...input, catalogDecision: null }),
  ).toEqual({ status: "unknown" });
});
it("does not accept a non-UUID receipt identity", async () => {
  invoke.mockResolvedValue({ ...prepared, operationId: "invalid" });
  expect(
    await vendorApi.prepareConnection({
      ...input,
      operationId: "invalid",
      catalogDecision: null,
    }),
  ).toEqual({ status: "unknown" });
});
it.each([
  "AABBCCDDEEFF00112233445566778899",
  "aabbccdd-eeff-0011-2233-445566778899",
  "{aabbccdd-eeff-0011-2233-445566778899}",
  "urn:uuid:aabbccdd-eeff-0011-2233-445566778899",
])(
  "preserves a supported UUID exactly without v4 enforcement: %s",
  async (id) => {
    const receipt = { ...prepared, operationId: id };
    invoke.mockResolvedValue(receipt);
    expect(
      await vendorApi.prepareConnection({
        ...input,
        operationId: id,
        catalogDecision: null,
      }),
    ).toEqual(receipt);
    expect(invoke.mock.calls[0][1].operationId).toBe(id);
  },
);
it("does not promote a switch when the expected revision is zero", async () => {
  invoke.mockResolvedValue(switched);
  expect(
    await vendorApi.switchPreparedConnection({
      ...switchInput,
      expectedRevision: 0,
    }),
  ).toEqual({ status: "unknown" });
});
