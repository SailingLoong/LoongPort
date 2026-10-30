import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { createAutomaticClaimCommand } from "../automaticClaimCommand";
import {
  accountPlatformPresenceKey,
  type PlatformAccountSummary,
  type PlatformPresenceSnapshot,
} from "../useAccountPlatformPresence";
const api = vi.hoisted(() => ({ set: vi.fn(), zread: vi.fn(), zset: vi.fn() }));
vi.mock("@/lib/api/workbuddy", () => ({
  workbuddyApi: { setAutomatic: api.set },
}));
vi.mock("@/lib/api/zcodeClaim", () => ({
  zcodeClaimApi: { state: api.zread, setAuto: api.zset },
}));
const row = (id = "a", generation = 2, revision = 3) =>
  ({
    id,
    source: "workbuddy",
    platform: "workbuddy",
    label: id,
    snapshot: {
      kind: "workbuddy",
      value: {
        id,
        generation,
        autoClaimRevision: revision,
        autoClaimPolicyVersion: 1,
        autoClaimEnabled: false,
        canSetAutomatic: true,
      },
    },
  }) as PlatformAccountSummary;
const data = (
  accounts: PlatformAccountSummary[],
): PlatformPresenceSnapshot => ({
  accounts,
  unavailable: [],
  platforms: ["workbuddy"],
  connections: [],
});
function setup(accounts = [row()]) {
  const client = new QueryClient();
  client.setQueryData(accountPlatformPresenceKey, data(accounts));
  const refresh = vi.fn(async () => ({
    data: client.getQueryData<PlatformPresenceSnapshot>(
      accountPlatformPresenceKey,
    ),
  }));
  return {
    client,
    refresh,
    command: createAutomaticClaimCommand({ client, refresh }),
  };
}
beforeEach(() => {
  vi.resetAllMocks();
  api.set.mockImplementation(async (value, enabled) => ({
    ...value,
    autoClaimEnabled: enabled,
    autoClaimRevision: value.autoClaimRevision + 1,
  }));
});
it("singleflights frozen targets and confirms only the clicked scope", async () => {
  const a = row();
  const { client, refresh, command } = setup([a]);
  let resolve!: (value: unknown) => void;
  api.set.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  refresh.mockImplementation(async () => ({ data: data([a, row("new")]) }));
  const first = command.execute({ targets: [a], enabled: true });
  const second = command.execute({ targets: [row("new")], enabled: true });
  await vi.waitFor(() => expect(api.set).toHaveBeenCalledTimes(1));
  resolve({
    ...a.snapshot!.value,
    autoClaimEnabled: true,
    autoClaimRevision: 4,
  });
  expect(await second).toEqual(await first);
  expect((await first).map((x) => [x.account.id, x.status])).toEqual([
    ["a", "confirmed"],
  ]);
  expect(
    client.getQueryData<PlatformPresenceSnapshot>(accountPlatformPresenceKey)
      ?.accounts?.[0].snapshot?.value,
  ).toMatchObject({ autoClaimEnabled: true, autoClaimRevision: 4 });
});
it("does not dispatch a changed generation or mark it as an attempted write", async () => {
  const a = row();
  const { refresh, command } = setup([a]);
  refresh.mockResolvedValue({ data: data([row("a", 9)]) });
  expect(
    (await command.execute({ targets: [a], enabled: true }))[0].status,
  ).toBe("notDispatched");
  expect(api.set).not.toHaveBeenCalled();
});
it("retains the submitted fresh stamp after a lost response without replaying", async () => {
  const a = row();
  const { refresh, command } = setup([a]);
  refresh.mockResolvedValue({ data: data([row("a", 2, 8)]) });
  api.set.mockRejectedValue(new Error("Synthetic lost response"));
  expect(await command.execute({ targets: [a], enabled: false })).toMatchObject(
    [{ status: "uncertain", stamp: "[2,8,1]" }],
  );
  expect(api.set).toHaveBeenCalledTimes(1);
});
it("rejects stale backend confirmation and leaves the cache unchanged", async () => {
  const a = row();
  const { client, command } = setup([a]);
  api.set.mockImplementation(async (value) => value);
  expect(
    (await command.execute({ targets: [a], enabled: true }))[0].status,
  ).toBe("uncertain");
  expect(
    client.getQueryData<PlatformPresenceSnapshot>(accountPlatformPresenceKey)
      ?.accounts?.[0],
  ).toEqual(a);
});
it("does not overwrite a newer cached owner with an old completion", async () => {
  const a = row();
  const { client, command } = setup([a]);
  api.set.mockImplementation(async (value) => {
    client.setQueryData(accountPlatformPresenceKey, data([row("a", 10, 12)]));
    return { ...value, autoClaimEnabled: true, autoClaimRevision: 4 };
  });
  await command.execute({ targets: [a], enabled: true });
  expect(
    client.getQueryData<PlatformPresenceSnapshot>(accountPlatformPresenceKey)
      ?.accounts?.[0].snapshot?.value,
  ).toMatchObject({ generation: 10, autoClaimRevision: 12 });
});
it("does not dispatch when the fresh presence cannot be read", async () => {
  const { refresh, command } = setup();
  refresh.mockRejectedValue(new Error("Synthetic read failure"));
  expect(
    (await command.execute({ targets: [row()], enabled: true }))[0].status,
  ).toBe("notDispatched");
  expect(api.set).not.toHaveBeenCalled();
});
const zrow = (id: string, revision = "context:1") =>
  ({
    id,
    label: id,
    platform: "zcode",
    source: "zcode",
    snapshot: {
      kind: "zcode",
      dataRoot: "synthetic-root",
      value: {
        enabled: true,
        participants: ["outside"],
        consentRevision: revision,
        busy: false,
        records: {},
      },
    },
  }) as PlatformAccountSummary;
it("keeps clicked ZCode revision and rejects a refreshed consent change", async () => {
  const a = zrow("a");
  const { refresh, command } = setup([a]);
  refresh.mockResolvedValue({ data: data([zrow("a", "context:2")]) });
  expect(
    (await command.execute({ targets: [a], enabled: true }))[0].status,
  ).toBe("notDispatched");
  expect(api.zset).not.toHaveBeenCalled();
});
it("submits ZCode once with scope outsiders preserved", async () => {
  const a = zrow("a"),
    b = zrow("b");
  const { command } = setup([a, b]);
  api.zread.mockResolvedValue(a.snapshot!.value);
  api.zset.mockImplementation(async (_root, enabled, participants) => ({
    ...a.snapshot!.value,
    enabled,
    participants,
    consentRevision: "context:2",
  }));
  expect(
    (await command.execute({ targets: [a, b], enabled: true })).map(
      (x) => x.status,
    ),
  ).toEqual(["confirmed", "confirmed"]);
  expect(api.zset).toHaveBeenCalledExactlyOnceWith(
    "synthetic-root",
    true,
    ["outside", "a", "b"],
    "context:1",
  );
});

it("does not dispatch a same-generation policy older than the clicked revision", async () => {
  const clicked = row("a", 2, 8);
  const { refresh, command } = setup([clicked]);
  refresh.mockResolvedValue({ data: data([row("a", 2, 3)]) });
  expect(
    await command.execute({ targets: [clicked], enabled: true }),
  ).toMatchObject([{ status: "notDispatched", stamp: "[2,8,1]" }]);
  expect(api.set).not.toHaveBeenCalled();
});

it.each(["read", "busy", "consent"])(
  "reports ZCode %s preflight failure as not dispatched",
  async (failure) => {
    const a = zrow("a");
    const { command } = setup([a]);
    if (failure === "read")
      api.zread.mockRejectedValue(new Error("Synthetic read failure"));
    else
      api.zread.mockResolvedValue({
        ...a.snapshot!.value,
        busy: failure === "busy",
        consentRevision: failure === "consent" ? "context:9" : "context:1",
      });
    expect(
      await command.execute({ targets: [a], enabled: true }),
    ).toMatchObject([
      {
        status: "notDispatched",
        stamp: '["synthetic-root","context:1",false]',
      },
    ]);
    expect(api.zset).not.toHaveBeenCalled();
  },
);

it("consumes an original owner's explicit no-dispatch proof", async () => {
  const { AutomaticClaimNotDispatchedError } =
    await import("../automaticClaimDispatch");
  const a = zrow("a");
  const { command } = setup([a]);
  api.zread.mockRejectedValue(
    new AutomaticClaimNotDispatchedError("Synthetic owner proof"),
  );
  expect(await command.execute({ targets: [a], enabled: true })).toMatchObject([
    { status: "notDispatched" },
  ]);
  expect(api.zset).not.toHaveBeenCalled();
});

it("does not dispatch when refetch resolves an error alongside stale cached data", async () => {
  const clicked = row();
  const { refresh, command } = setup([clicked]);
  refresh.mockImplementation(async () => ({
    data: data([clicked]),
    error: new Error("Synthetic refetch failure with retained cache"),
  }));
  expect(
    await command.execute({ targets: [clicked], enabled: true }),
  ).toMatchObject([{ status: "notDispatched", reason: "readFailed" }]);
  expect(api.set).not.toHaveBeenCalled();
});

it("does not dispatch when the real Query observer resolves a falsy refetch error with retained data", async () => {
  const clicked = row();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  client.setQueryData(accountPlatformPresenceKey, data([clicked]));
  const observer = new QueryObserver(client, {
    queryKey: accountPlatformPresenceKey,
    queryFn: async () => {
      throw null;
    },
    retry: false,
  });
  const command = createAutomaticClaimCommand({
    client,
    refresh: () => observer.refetch(),
  });
  expect(
    await command.execute({ targets: [clicked], enabled: true }),
  ).toMatchObject([{ status: "notDispatched", reason: "readFailed" }]);
  expect(api.set).not.toHaveBeenCalled();
  client.clear();
});
