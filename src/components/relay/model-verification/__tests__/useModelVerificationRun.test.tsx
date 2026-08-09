import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  start: vi.fn(),
  cancel: vi.fn(),
  listResults: vi.fn(),
  onProgress: vi.fn(),
  onChanged: vi.fn(),
}));

vi.mock("@/lib/api/modelVerification", () => ({ modelVerificationApi: api }));

import { useModelVerificationRun } from "../useModelVerificationRun";

let progressListener: ((event: unknown) => void) | undefined;
let changedListener: ((event: unknown) => void) | undefined;
let stopProgress = vi.fn();
let stopChanged = vi.fn();

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
    changedListener = undefined;
    stopProgress = vi.fn();
    stopChanged = vi.fn();
    api.start.mockResolvedValue({ runId: "run-1", state: "queued" });
    api.cancel.mockResolvedValue(undefined);
    api.listResults.mockResolvedValue([]);
    api.onProgress.mockImplementation(
      async (listener: (event: unknown) => void) => {
        progressListener = listener;
        return stopProgress;
      },
    );
    api.onChanged.mockImplementation(
      async (listener: (event: unknown) => void) => {
        changedListener = listener;
        return stopChanged;
      },
    );
  });

  it("tracks only the active run and target, then fetches persisted reports after a matching change", async () => {
    const { result } = renderHook(() =>
      useModelVerificationRun({
        open: true,
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

    api.listResults.mockResolvedValueOnce([
      { target, verdict: "trusted", facts: [] },
    ]);
    await act(async () => {
      await changedListener?.({ providerId: "provider-a", appType: "codex" });
    });
    await waitFor(() =>
      expect(api.listResults).toHaveBeenCalledWith(["provider-a"]),
    );
    await waitFor(() => expect(result.current.report?.verdict).toBe("trusted"));
  });

  it("keeps terminal events emitted before start resolves", async () => {
    api.listResults.mockResolvedValueOnce([
      { target, verdict: "trusted", facts: [] },
    ]);
    api.start.mockImplementation(async () => {
      progressListener?.(
        runningEvent({
          state: "completed",
          completedChecks: 3,
        }),
      );
      changedListener?.({
        providerId: target.providerId,
        appType: target.appType,
      });
      return { runId: "run-1", state: "queued" };
    });

    const { result } = renderHook(() =>
      useModelVerificationRun({
        open: true,
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
    await waitFor(() => expect(result.current.report?.verdict).toBe("trusted"));
  });

  it("unsubscribes when closed and ignores a prior dialog instance completion", async () => {
    const { result, rerender } = renderHook(
      ({ open }) =>
        useModelVerificationRun({
          open,
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
    expect(stopProgress).toHaveBeenCalled();
    expect(stopChanged).toHaveBeenCalled();
    expect(result.current.progress).toBeNull();

    rerender({ open: true });
    await act(async () => {
      await result.current.start(target.model);
    });
    act(() => priorListener?.(runningEvent({ state: "completed" })));

    expect(result.current.progress).toBeNull();
    expect(api.listResults).not.toHaveBeenCalled();
  });
});
