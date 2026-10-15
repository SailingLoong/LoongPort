import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { serviceSourcesApi } from "@/lib/api/serviceSources";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("service source read API", () => {
  it("reads the backend-owned projection without transmitting account data or credentials", async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    await expect(serviceSourcesApi.list()).resolves.toEqual([]);
    expect(invoke).toHaveBeenCalledExactlyOnceWith("get_service_sources");
  });
});
