import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { settingsApi } from "@/lib/api/settings";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const base = {
  deviceName: "synthetic-device",
  createdAt: "synthetic-date",
  snapshotId: "synthetic-snapshot",
  version: 3,
  protocolVersion: 3,
  dbCompatVersion: 6,
  compatible: true,
  artifacts: ["db.sql"],
  layout: "current",
  remotePath: "synthetic/path",
};
beforeEach(() => {
  vi.mocked(invoke).mockReset();
});
for (const [fetch, command] of [
  [settingsApi.webdavSyncFetchRemoteInfo, "webdav_sync_fetch_remote_info"],
  [settingsApi.s3SyncFetchRemoteInfo, "s3_sync_fetch_remote_info"],
] as const) {
  it(`${command}: legacy fields default to source input, never plaintext`, async () => {
    vi.mocked(invoke).mockResolvedValue(base);
    expect(await fetch()).toEqual({
      ...base,
      protectionMode: "encrypted",
      sourceInputRequired: true,
    });
  });
  it.each([
    { protectionMode: "encrypted", sourceInputRequired: true },
    { protectionMode: "plaintext", sourceInputRequired: false },
    { protectionMode: null, sourceInputRequired: false, compatible: false },
  ])(
    `${command}: accepts the explicit native protection pair (%j)`,
    async (fields) => {
      vi.mocked(invoke).mockResolvedValue({ ...base, ...fields });
      expect(await fetch()).toEqual({ ...base, ...fields });
      expect(invoke).toHaveBeenCalledExactlyOnceWith(command);
    },
  );
  it.each([
    { protectionMode: "plaintext" },
    { sourceInputRequired: false },
    { protectionMode: "plaintext", sourceInputRequired: true },
    { protectionMode: "encrypted", sourceInputRequired: false },
    { protectionMode: null, sourceInputRequired: false },
    { protectionMode: null, sourceInputRequired: true, compatible: false },
    {
      protectionMode: "encrypted",
      sourceInputRequired: true,
      compatible: false,
    },
    {
      protectionMode: "plaintext",
      sourceInputRequired: false,
      compatible: false,
    },
    { protectionMode: "future", sourceInputRequired: true },
    { protectionMode: "plaintext", sourceInputRequired: "false" },
  ])(
    `${command}: fails closed on partial, unknown or contradictory fields (%j)`,
    async (fields) => {
      vi.mocked(invoke).mockResolvedValue({ ...base, ...fields });
      await expect(fetch()).rejects.toThrow("sync.invalid_remote_info");
    },
  );
  it(`${command}: retains exact empty result and does not invent source protection`, async () => {
    vi.mocked(invoke).mockResolvedValue({ empty: true });
    expect(await fetch()).toEqual({ empty: true });
  });
}
it.each([settingsApi.webdavSyncRestore, settingsApi.s3SyncRestore])(
  "keeps explicit no-input restore and bound snapshot on the original wire",
  async (restore) => {
    vi.mocked(invoke).mockResolvedValue({ status: "restored" });
    await restore("", "synthetic-snapshot");
    expect(vi.mocked(invoke).mock.calls[0][1]).toEqual({
      password: "",
      expectedSnapshotId: "synthetic-snapshot",
    });
  },
);
