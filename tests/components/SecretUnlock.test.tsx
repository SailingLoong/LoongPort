import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SecretUnlock } from "@/components/SecretUnlock";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

describe("SecretUnlock", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it("routes the original upgrade checkpoint block to a read-only upgrade query", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      status: "checkpoint_requires_verification",
      checkpointPresent: true,
      reviewToken: null,
      canAuthenticate: true,
      canCheckAndBackup: false,
      canStartUpgrade: false,
    });
    render(<SecretUnlock initialError="upgrade.sync_paused" />);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_startup_upgrade_review"),
    );
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(
      screen.queryByRole("button", { name: "secrets.unlock" }),
    ).not.toBeInTheDocument();
  });

  it("queries an interrupted operation and recovers only after an explicit password action", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      token: "opaque-operation-revision",
      status: "pending",
      canRecover: true,
      restartRequired: false,
    });
    const onUnlocked = vi.fn();
    render(
      <SecretUnlock
        initialError="secret.recovery_required"
        onUnlocked={onUnlocked}
      />,
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_startup_recovery"),
    );
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(
      screen.queryByRole("button", { name: "secrets.retrySystemUnlock" }),
    ).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("secrets.password"), {
      target: { value: "new recovery password" },
    });
    vi.mocked(invoke).mockResolvedValueOnce({
      token: "opaque-operation-revision",
      status: "completed",
      canRecover: false,
      restartRequired: true,
    });
    fireEvent.click(
      screen.getByRole("button", { name: "secrets.resumeRecovery" }),
    );
    await screen.findByRole("button", { name: "secrets.restart" });
    expect(invoke).toHaveBeenLastCalledWith("recover_startup_operation", {
      token: "opaque-operation-revision",
      password: "new recovery password",
    });
    expect(onUnlocked).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("secrets.password")).not.toBeInTheDocument();
  });

  it("keeps the recovery view on failure and enters the app only after success", async () => {
    const onUnlocked = vi.fn();
    vi.mocked(invoke).mockRejectedValueOnce("secret.password_rejected");
    render(<SecretUnlock onUnlocked={onUnlocked} />);
    fireEvent.change(screen.getByLabelText("secrets.password"), {
      target: { value: "test protection password" },
    });
    fireEvent.click(screen.getByRole("button", { name: "secrets.unlock" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "secrets.passwordRejected",
    );
    expect(onUnlocked).not.toHaveBeenCalled();

    vi.mocked(invoke).mockResolvedValueOnce(undefined);
    fireEvent.click(screen.getByRole("button", { name: "secrets.unlock" }));
    await waitFor(() => expect(onUnlocked).toHaveBeenCalledOnce());
    expect(invoke).toHaveBeenLastCalledWith("unlock_secret_vault", {
      password: "test protection password",
    });
    expect(screen.getByLabelText("secrets.password")).toHaveValue("");
  });

  it("queries a lost recovery response without repeating the mutation", async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({
        token: "revision",
        status: "pending",
        canRecover: true,
        restartRequired: false,
      })
      .mockRejectedValueOnce("transport response lost")
      .mockResolvedValueOnce({
        token: "revision",
        status: "completed",
        canRecover: false,
        restartRequired: true,
      });
    const onUnlocked = vi.fn();
    render(
      <SecretUnlock
        initialError="secret.recovery_required"
        onUnlocked={onUnlocked}
      />,
    );
    await waitFor(() => expect(invoke).toHaveBeenCalledOnce());
    fireEvent.change(screen.getByLabelText("secrets.password"), {
      target: { value: "recovery password" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "secrets.resumeRecovery" }),
    );
    await screen.findByRole("button", { name: "secrets.restart" });
    expect(screen.getByText("secrets.recoveryCompleted")).toBeInTheDocument();
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
      "get_startup_recovery",
      "recover_startup_operation",
      "get_startup_recovery",
    ]);
    expect(onUnlocked).not.toHaveBeenCalled();
  });

  it("keeps recovery blocked when the response and operation query are unavailable", async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({
        token: "revision",
        status: "pending",
        canRecover: true,
        restartRequired: false,
      })
      .mockRejectedValueOnce("transport response lost")
      .mockRejectedValueOnce("query unavailable");
    render(<SecretUnlock initialError="secret.recovery_required" />);
    await waitFor(() => expect(invoke).toHaveBeenCalledOnce());
    fireEvent.change(screen.getByLabelText("secrets.password"), {
      target: { value: "recovery password" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "secrets.resumeRecovery" }),
    );
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(3));
    fireEvent.change(screen.getByLabelText("secrets.password"), {
      target: { value: "recovery password" },
    });
    expect(
      screen.getByRole("button", { name: "secrets.resumeRecovery" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "secrets.recheckOperation" }),
    ).toBeEnabled();
  });

  it("retries the system store without submitting the password field", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined);
    const onUnlocked = vi.fn();
    render(<SecretUnlock onUnlocked={onUnlocked} />);
    fireEvent.click(
      screen.getByRole("button", { name: "secrets.retrySystemUnlock" }),
    );
    await waitFor(() => expect(onUnlocked).toHaveBeenCalledOnce());
    expect(invoke).toHaveBeenCalledWith("unlock_secret_vault", {
      password: null,
    });
  });
});
