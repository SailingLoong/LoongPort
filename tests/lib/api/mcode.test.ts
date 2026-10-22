import { beforeEach, describe, expect, it, vi } from "vitest";
import { mcodeApi } from "@/lib/api/mcode";
import { providersApi } from "@/lib/api/providers";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() => {
  invoke.mockReset();
});

describe("MiniMax Code native application command boundary", () => {
  it("keeps explicit native import in the provider API, separate from account commands", async () => {
    invoke.mockResolvedValue(true);
    await providersApi.importDefault("mcode");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("import_default_config", {
      app: "mcode",
    });
  });

  it("only reads backup availability", async () => {
    invoke.mockResolvedValue(null);
    expect(await mcodeApi.getBackup()).toBeNull();
    expect(invoke).toHaveBeenCalledExactlyOnceWith("get_mcode_backup");
  });

  it("restores only the exact selected backup and current revision without replay", async () => {
    invoke.mockRejectedValue("sourceChanged");
    await expect(
      mcodeApi.restoreBackup({ id: "backup-1", revision: "revision-1" }),
    ).rejects.toBe("sourceChanged");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("restore_mcode_backup", {
      id: "backup-1",
      revision: "revision-1",
    });
  });
});
