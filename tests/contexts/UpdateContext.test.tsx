import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emitTauriEvent } from "../msw/tauriMocks";

const updaterMocks = vi.hoisted(() => ({
  checkForUpdate: vi.fn(),
}));

vi.mock("@/lib/updater", () => ({
  checkForUpdate: () => updaterMocks.checkForUpdate(),
}));

import { UpdateProvider, useUpdate } from "@/contexts/UpdateContext";

function UpdateState() {
  const update = useUpdate();

  return (
    <>
      <span>{update.updateInfo?.availableVersion ?? "no update"}</span>
      <span>{update.isDismissed ? "dismissed" : "not dismissed"}</span>
      <span>{update.error ?? "no error"}</span>
      <button
        type="button"
        onClick={() => {
          void update.checkUpdate().catch(() => undefined);
        }}
      >
        Check for updates
      </button>
    </>
  );
}

function renderProvider() {
  return render(
    <UpdateProvider>
      <UpdateState />
    </UpdateProvider>,
  );
}

const available = {
  status: "available" as const,
  info: {
    currentVersion: "3.24.0",
    availableVersion: "3.25.0",
  },
};

describe("UpdateProvider", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    updaterMocks.checkForUpdate.mockReset();
    localStorage.clear();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("applies a backend update event without starting a timer", async () => {
    renderProvider();
    await act(async () => {});

    await act(async () => {
      emitTauriEvent("app-update-checked", available);
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
      emitTauriEvent("app-update-checked", available);
      emitTauriEvent("app-update-checked", available);
    });

    expect(screen.getByText("dismissed")).toBeInTheDocument();
  });

  it("migrates a legacy dismissed version before applying an event", async () => {
    localStorage.setItem("dismissedUpdateVersion", "3.25.0");
    renderProvider();
    await act(async () => {});

    await act(async () => {
      emitTauriEvent("app-update-checked", available);
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
      emitTauriEvent("app-update-checked", available);
      emitTauriEvent("app-update-checked", { status: "upToDate" });
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

  it("records a manual check error for the presentation layer", async () => {
    updaterMocks.checkForUpdate.mockRejectedValue(new Error("network offline"));
    renderProvider();
    await act(async () => {});

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await act(async () => {});

    expect(screen.getByText("network offline")).toBeInTheDocument();
  });
});
