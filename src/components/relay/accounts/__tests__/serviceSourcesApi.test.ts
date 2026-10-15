import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { serviceSourcesApi } from "@/lib/api/serviceSources";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("service source API", () => {
  it("reads the backend-owned projection without transmitting account data or credentials", async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    await expect(serviceSourcesApi.list()).resolves.toEqual([]);
    expect(invoke).toHaveBeenCalledExactlyOnceWith("get_service_sources");
  });
});

it("binds a source using references only, without credentials or activation options", async () => {
  vi.mocked(invoke).mockClear();
  vi.mocked(invoke).mockResolvedValue(true);
  await expect(
    serviceSourcesApi.bindVendor("synthetic-source", "claude"),
  ).resolves.toBe(true);
  expect(invoke).toHaveBeenCalledExactlyOnceWith("bind_vendor_service_source", {
    sourceId: "synthetic-source",
    app: "claude",
  });
});
