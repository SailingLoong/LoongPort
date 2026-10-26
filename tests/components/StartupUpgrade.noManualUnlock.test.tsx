import { render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi, afterEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { StartupUpgrade } from "@/components/StartupUpgrade";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => vi.resetAllMocks());
it.each(["restart_required", "recovery_required", "locked"])(
  "does not reintroduce manual unlock from upgrade status %s",
  async (status) => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_startup_upgrade_review")
        return {
          status,
          canAuthenticate: true,
          checkpointPresent: false,
          canCheckAndBackup: false,
          canStartUpgrade: false,
        };
      return { status: "pending", canRecover: false };
    });
    render(<StartupUpgrade embedded />);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_startup_upgrade_review"),
    );
    await waitFor(() =>
      expect(
        screen.queryByText("startupUpgrade.loading"),
      ).not.toBeInTheDocument(),
    );
    expect
      .soft(screen.queryByLabelText("secrets.password"))
      .not.toBeInTheDocument();
    expect
      .soft(screen.queryByText("secrets.unlockTitle"))
      .not.toBeInTheDocument();
    expect
      .soft(screen.queryByText("startupUpgrade.authenticate"))
      .not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith(
      "unlock_secret_vault",
      expect.anything(),
    );
  },
);
