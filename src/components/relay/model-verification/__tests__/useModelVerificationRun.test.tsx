import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  start: vi.fn(),
  cancel: vi.fn(),
  onProgress: vi.fn(),
}));

vi.mock("@/lib/api/modelVerification", () => ({ modelVerificationApi: api }));

import { useModelVerificationRun } from "../useModelVerificationRun";

let progressListener: ((event: unknown) => void) | undefined;
let stopProgress = vi.fn();

const target = { providerId: "provider-a", appType: "codex", model: "gpt-5" };

const runningEvent = (overrides: Record<string, unknown> = {}) => ({
  runId: "run-1",
  ...target,
  state: "running",
  completedChecks: 1,
  totalChecks: 3,
  failure: null,
  ...overrides,
});

describe("useModelVerificationRun", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    progressListener = undefined;
    stopProgress = vi.fn();
    api.start.mockResolvedValue({ runId: "run-1", state: "queued" });
    api.cancel.mockResolvedValue(undefined);
    api.onProgress.mockImplementation(
      async (listener: (event: unknown) => void) => {
        progressListener = listener;
        return stopProgress;
      },
    );
  });

  it("tracks only the active run and target", async () => {
    const { result } = renderHook(() =>
      useModelVerificationRun({
        providerId: target.providerId,
        appType: target.appType,
      }),
    );

    await act(async () => {
      await result.current.start(target.model);
    });
    await waitFor(() => expect(progressListener).toBeDefined());

    act(() => progressListener?.(runningEvent({ runId: "old-run" })));
    expect(result.current.progress).toBeNull();

    act(() => progressListener?.(runningEvent({ providerId: "provider-b" })));
    expect(result.current.progress).toBeNull();

    act(() => progressListener?.(runningEvent()));
    expect(result.current.progress?.completedChecks).toBe(1);
  });

  it("keeps terminal events emitted before start resolves", async () => {
    api.start.mockImplementation(async () => {
      progressListener?.(
        runningEvent({
          state: "completed",
          completedChecks: 3,
        }),
      );
      return { runId: "run-1", state: "queued" };
    });

    const { result } = renderHook(() =>
      useModelVerificationRun({
        providerId: target.providerId,
        appType: target.appType,
      }),
    );

    await act(async () => {
      await result.current.start(target.model);
    });

    await waitFor(() =>
      expect(result.current.progress?.state).toBe("completed"),
    );
    expect(result.current.isRunning).toBe(false);
  });

  it("keeps an early terminal failure when start later binds the run ID", async () => {
    api.start.mockImplementation(async () => {
      progressListener?.(
        runningEvent({
          state: "failed",
          completedChecks: 1,
          failure: "authentication",
        }),
      );
      return { runId: "run-1", state: "running" };
    });

    const { result } = renderHook(() =>
      useModelVerificationRun({
        providerId: target.providerId,
        appType: target.appType,
      }),
    );

    await act(async () => {
      await result.current.start(target.model);
    });

    expect(result.current.progress?.state).toBe("failed");
    expect(result.current.failure).toBe("authentication");
    expect(result.current.isRunning).toBe(false);
  });

  it("keeps the active run subscribed while its persistent dialog is closed", async () => {
    const { result, rerender } = renderHook(
      ({ open: _open }) =>
        useModelVerificationRun({
          providerId: target.providerId,
          appType: target.appType,
        }),
      { initialProps: { open: true } },
    );
    await act(async () => {
      await result.current.start(target.model);
    });
    await waitFor(() => expect(progressListener).toBeDefined());
    const priorListener = progressListener;

    rerender({ open: false });
    expect(stopProgress).not.toHaveBeenCalled();

    rerender({ open: true });
    act(() => priorListener?.(runningEvent({ state: "completed" })));

    expect(result.current.progress?.state).toBe("completed");
    expect(api.start).toHaveBeenCalledTimes(1);
  });
});
