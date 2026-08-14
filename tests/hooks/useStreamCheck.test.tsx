import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useStreamCheck } from "@/hooks/useStreamCheck";

const mocks = vi.hoisted(() => ({
  streamCheckProvider: vi.fn(),
  toastSuccess: vi.fn(),
  toastWarning: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@/lib/api/connectivity-check", () => ({
  streamCheckProvider: (...args: unknown[]) =>
    mocks.streamCheckProvider(...args),
}));

vi.mock("sonner", () => ({
  toast: {
    success: (...args: unknown[]) => mocks.toastSuccess(...args),
    warning: (...args: unknown[]) => mocks.toastWarning(...args),
    error: (...args: unknown[]) => mocks.toastError(...args),
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) => {
      switch (key) {
        case "streamCheck.reachable":
          return `${options?.providerName} reachable`;
        case "streamCheck.modelProbe.keyExpired":
          return `key expired (${options?.status})`;
        case "streamCheck.modelProbe.models":
          return `${options?.total} models: ${options?.models}`;
        default:
          return key;
      }
    },
  }),
}));

describe("useStreamCheck", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("displays the typed model probe verdict returned by the backend", async () => {
    mocks.streamCheckProvider.mockResolvedValue({
      status: "operational",
      overallStatus: "healthy",
      success: true,
      message: "Reachable",
      responseTimeMs: 12,
      httpStatus: 200,
      modelProbe: {
        kind: "models",
        total: 4,
        head: ["gpt-5", "gpt-4", "…"],
      },
      testedAt: 1,
      retryCount: 0,
    });
    const { result } = renderHook(() => useStreamCheck("codex"));

    await act(async () => {
      await result.current.checkProvider("provider-1", "Provider One");
    });

    expect(mocks.toastSuccess).toHaveBeenCalledWith("Provider One reachable", {
      closeButton: true,
      description: "4 models: gpt-5 / gpt-4 / …",
    });
  });

  it("uses the backend overall status for an unusable model probe", async () => {
    mocks.streamCheckProvider.mockResolvedValue({
      status: "operational",
      overallStatus: "unusable",
      success: true,
      message: "Reachable",
      responseTimeMs: 12,
      httpStatus: 200,
      modelProbe: {
        kind: "keyExpired",
        status: 401,
      },
      testedAt: 1,
      retryCount: 0,
    });
    const { result } = renderHook(() => useStreamCheck("codex"));

    await act(async () => {
      await result.current.checkProvider("provider-1", "Provider One");
    });

    expect(mocks.toastError).toHaveBeenCalledWith("key expired (401)", {
      closeButton: true,
      description: "Provider One reachable",
    });
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
    expect(mocks.toastWarning).not.toHaveBeenCalled();
  });

  it("does not interpret a legacy JSON string as a frontend verdict", async () => {
    mocks.streamCheckProvider.mockResolvedValue({
      status: "operational",
      overallStatus: "healthy",
      success: true,
      message: "Reachable",
      responseTimeMs: 12,
      httpStatus: 200,
      modelUsed: JSON.stringify({ kind: "keyExpired", status: 401 }),
      testedAt: 1,
      retryCount: 0,
    });
    const { result } = renderHook(() => useStreamCheck("codex"));

    await act(async () => {
      await result.current.checkProvider("provider-1", "Provider One");
    });

    expect(mocks.toastSuccess).toHaveBeenCalledWith("Provider One reachable", {
      closeButton: true,
      description: undefined,
    });
  });
});
