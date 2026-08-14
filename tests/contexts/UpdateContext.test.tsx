import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const updaterMocks = vi.hoisted(() => ({
  checkForUpdate: vi.fn(),
}));

const eventMocks = vi.hoisted(() => {
  const listeners = new Set<(event: { payload: unknown }) => void>();
  return {
    emit: (payload: unknown) => {
      listeners.forEach((listener) => listener({ payload }));
    },
    listen: vi.fn(),
    reset: () => listeners.clear(),
    register: (handler: (event: { payload: unknown }) => void) => {
      listeners.add(handler);
      return () => listeners.delete(handler);
    },
  };
});

vi.mock("@/lib/updater", () => ({
  checkForUpdate: () => updaterMocks.checkForUpdate(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => eventMocks.listen(...args),
}));

import { UpdateProvider, useUpdate } from "@/contexts/UpdateContext";
import type { UpdateInfo } from "@/lib/updater";

function UpdateState({
  onCheckUpdate,
}: {
  onCheckUpdate?: (result: Promise<boolean>) => void;
}) {
  const update = useUpdate();

  return (
    <>
      <span>{update.updateInfo?.availableVersion ?? "no update"}</span>
      <span>{update.isDismissed ? "dismissed" : "not dismissed"}</span>
      <span>{update.error ?? "no error"}</span>
      <button
        type="button"
        onClick={() => {
          const result = update.checkUpdate();
          onCheckUpdate?.(result);
        }}
      >
        Check for updates
      </button>
    </>
  );
}

function renderProvider(onCheckUpdate?: (result: Promise<boolean>) => void) {
  return render(
    <UpdateProvider>
      <UpdateState onCheckUpdate={onCheckUpdate} />
    </UpdateProvider>,
  );
}

const available = {
  status: "available" as const,
  info: {
    currentVersion: "3.24.0",
    availableVersion: "3.25.0",
    notes: null,
    pubDate: null,
  },
};

it("models absent updater metadata as IPC null values", () => {
  const info: UpdateInfo = {
    currentVersion: "3.24.0",
    availableVersion: "3.25.0",
    notes: null,
    pubDate: null,
  };

  expect(info.notes).toBeNull();
  expect(info.pubDate).toBeNull();
});

describe("UpdateProvider", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    updaterMocks.checkForUpdate.mockReset();
    eventMocks.listen.mockReset();
    eventMocks.listen.mockImplementation(
      (_eventName: string, handler: (event: { payload: unknown }) => void) =>
        Promise.resolve(eventMocks.register(handler)),
    );
    eventMocks.reset();
    localStorage.clear();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("applies a backend update event without starting a timer", async () => {
    renderProvider();
    await act(async () => {});

    await act(async () => {
      eventMocks.emit(available);
    });

    expect(screen.getByText("3.25.0")).toBeInTheDocument();
    expect(vi.getTimerCount()).toBe(0);
    expect(updaterMocks.checkForUpdate).not.toHaveBeenCalled();
  });

  it("keeps a dismissed version dismissed across identical backend events", async () => {
    localStorage.setItem("ccswitch:update:dismissedVersion", "3.25.0");
    renderProvider();
    await act(async () => {});

    await act(async () => {
      eventMocks.emit(available);
      eventMocks.emit(available);
    });

    expect(screen.getByText("dismissed")).toBeInTheDocument();
  });

  it("migrates a legacy dismissed version before applying an event", async () => {
    localStorage.setItem("dismissedUpdateVersion", "3.25.0");
    renderProvider();
    await act(async () => {});

    await act(async () => {
      eventMocks.emit(available);
    });

    expect(screen.getByText("dismissed")).toBeInTheDocument();
    expect(localStorage.getItem("ccswitch:update:dismissedVersion")).toBe(
      "3.25.0",
    );
    expect(localStorage.getItem("dismissedUpdateVersion")).toBeNull();
  });

  it("clears update and dismissed state when the backend reports up to date", async () => {
    localStorage.setItem("ccswitch:update:dismissedVersion", "3.25.0");
    renderProvider();
    await act(async () => {});

    await act(async () => {
      eventMocks.emit(available);
      eventMocks.emit({ status: "upToDate" });
    });

    expect(screen.getByText("no update")).toBeInTheDocument();
    expect(screen.getByText("not dismissed")).toBeInTheDocument();
  });

  it("applies manual results through the same update state", async () => {
    updaterMocks.checkForUpdate.mockResolvedValue(available);
    renderProvider();
    await act(async () => {});

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await act(async () => {});

    expect(screen.getByText("3.25.0")).toBeInTheDocument();
  });

  it("records and rethrows the original manual command rejection", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    updaterMocks.checkForUpdate.mockRejectedValue("network offline");
    let result: Promise<boolean> | undefined;
    renderProvider((checkResult) => {
      result = checkResult;
    });
    await act(async () => {});

    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Check for updates" }),
      );
      await expect(result).rejects.toBe("network offline");
    });

    expect(screen.getByText("network offline")).toBeInTheDocument();
    expect(consoleError).toHaveBeenCalledWith(
      "检查更新失败:",
      "network offline",
    );
  });

  it("handles listener registration rejection", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    const listenerError = new Error("listener registration failed");
    eventMocks.listen.mockReturnValue(Promise.reject(listenerError));

    renderProvider();
    await act(async () => {});

    expect(consoleError).toHaveBeenCalledWith(
      "Failed to listen for app update checks",
      listenerError,
    );
  });

  it("cleans up a listener that registers after unmount", async () => {
    let resolveRegistration: ((cleanup: () => void) => void) | undefined;
    const cleanup = vi.fn();
    eventMocks.listen.mockReturnValue(
      new Promise<(cleanup: () => void) => void>((resolve) => {
        resolveRegistration = resolve;
      }),
    );

    const { unmount } = renderProvider();
    unmount();

    await act(async () => {
      resolveRegistration?.(cleanup);
    });

    expect(cleanup).toHaveBeenCalledOnce();
  });
});
