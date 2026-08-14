import { beforeEach, describe, expect, it, vi } from "vitest";
import { streamCheckProvider } from "@/lib/api/connectivity-check";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

describe("streamCheckProvider", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("returns the backend typed model probe verdict unchanged", async () => {
    const backendResult = {
      status: "operational",
      overallStatus: "healthy",
      success: true,
      message: "Reachable",
      responseTimeMs: 12,
      httpStatus: 200,
      modelProbe: {
        kind: "models",
        total: 4,
        head: ["alpha", "beta", "…"],
      },
      testedAt: 1,
      retryCount: 0,
    };
    invokeMock.mockResolvedValue(backendResult);

    await expect(streamCheckProvider("codex", "provider-1")).resolves.toBe(
      backendResult,
    );
    expect(invokeMock).toHaveBeenCalledWith("stream_check_provider", {
      appType: "codex",
      providerId: "provider-1",
    });
  });
});
