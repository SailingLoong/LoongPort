import type { ReactNode } from "react";
import { renderHook, act, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  reconciliationKeys,
  useReconciliationQuery,
} from "@/components/relay/useReconciliationQuery";
import type { ReconciliationReport } from "@/lib/api/relay";
import { createTestQueryClient } from "../utils/testQueryClient";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

function createWrapper() {
  const queryClient = createTestQueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  return { wrapper, queryClient };
}

const report: ReconciliationReport = {
  relayId: 7,
  snapshotCount: 4,
  baselineRatio: 0.5,
  windows: [
    {
      startSecs: 300,
      endSecs: 400,
      startBalanceUsd: 6,
      endBalanceUsd: 4,
      balanceDeltaUsd: -2,
      estimatedCostUsd: 1,
      ratio: 0.5,
      flag: "normal",
    },
    {
      startSecs: 200,
      endSecs: 300,
      startBalanceUsd: 8,
      endBalanceUsd: 12,
      balanceDeltaUsd: 4,
      estimatedCostUsd: 1,
      ratio: null,
      flag: "skippedTopUp",
    },
  ],
};

describe("useReconciliationQuery", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation((command: string) => {
      if (command === "relay_reconciliation") {
        return Promise.resolve(report);
      }
      return Promise.resolve(null);
    });
  });

  it("keeps the established query key shapes", () => {
    expect(reconciliationKeys.all).toEqual(["reconciliation"]);
    expect(reconciliationKeys.report(7)).toEqual(["reconciliation", 7]);
  });

  it("calls relay_reconciliation with the relay id and exposes the report", async () => {
    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useReconciliationQuery(7), { wrapper });

    expect(result.current.loading).toBe(true);

    await waitFor(() => {
      expect(result.current.report).toEqual(report);
    });

    expect(invokeMock).toHaveBeenCalledWith("relay_reconciliation", {
      relayId: 7,
    });
    expect(result.current.error).toBeNull();
  });

  it("keeps the last good report when a refetch fails", async () => {
    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useReconciliationQuery(7), { wrapper });

    await waitFor(() => {
      expect(result.current.report).toEqual(report);
    });

    invokeMock.mockRejectedValue(new Error("数据库被锁"));
    await act(async () => {
      await result.current.refetch();
    });

    expect(result.current.report).toEqual(report);
  });

  it("exposes the error message when the first fetch fails", async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === "relay_reconciliation") {
        return Promise.reject(new Error("找不到 id 为 7 的中转站"));
      }
      return Promise.resolve(null);
    });

    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useReconciliationQuery(7), { wrapper });

    // retry: 1 + retryDelay 1500ms ⇒ 错误要 ~1.6s 后才透出，waitFor 默认 1s 不够。
    await waitFor(
      () => {
        expect(result.current.error).toContain("找不到 id 为 7 的中转站");
      },
      { timeout: 4000 },
    );
    expect(result.current.report).toBeUndefined();
  });
});
