import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { exit } from "@tauri-apps/plugin-process";
import { StartupUpgrade } from "@/components/StartupUpgrade";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ exit: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const review = (extra = {}) => ({
  status: "database_verified",
  checkpointPresent: true,
  checkpointId: "synthetic-checkpoint",
  reviewToken: "synthetic-review",
  canAuthenticate: false,
  canCheckAndBackup: false,
  canStartUpgrade: false,
  ...extra,
});
const appReview = (appType: string, extra = {}) => ({
  appType,
  revision: `synthetic-${appType}-revision`,
  savedMode: "direct",
  hasPendingOperation: false,
  pointerConsistent: true,
  liveStatus: "parsed",
  storedFieldsMatch: true,
  canRecoverOperation: false,
  defaultAction: "keep_files",
  defaultTakeover: false,
  canCompleteApp: false,
  canStartUpgrade: false,
  ...extra,
});
const recoveries = () =>
  vi
    .mocked(invoke)
    .mock.calls.filter(
      ([command]) => command === "recover_startup_upgrade_app",
    );

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(exit).mockReset();
});

function serve(view = review(), overrides: Record<string, object> = {}) {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "get_startup_upgrade_review") return view;
    if (command === "review_startup_upgrade_app") {
      const app = (args as { appType: string }).appType;
      return appReview(app, overrides[app]);
    }
    throw new Error(`Unexpected synthetic command: ${command}`);
  });
}

describe("StartupUpgrade", () => {
  it("uses the original recovery form after publication leaves a generation intent", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_startup_upgrade_review")
        return review({ status: "recovery_required" });
      if (command === "get_startup_recovery")
        return {
          token: "synthetic-original-recovery",
          status: "pending",
          canRecover: true,
          restartRequired: false,
        };
      if (command === "recover_startup_operation")
        return {
          token: "synthetic-original-recovery",
          status: "completed",
          canRecover: false,
          restartRequired: true,
        };
      throw new Error("synthetic unexpected command");
    });
    render(<StartupUpgrade />);
    await screen.findByRole("heading", { name: "secrets.recoveryTitle" });
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_startup_recovery"),
    );
    expect(invoke).not.toHaveBeenCalledWith(
      "recover_startup_operation",
      expect.anything(),
    );
    fireEvent.change(screen.getByLabelText("secrets.password"), {
      target: { value: "synthetic-recovery-password" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "secrets.resumeRecovery" }),
    );
    await screen.findByRole("button", { name: "secrets.restart" });
    expect(invoke).toHaveBeenCalledWith("recover_startup_operation", {
      token: "synthetic-original-recovery",
      password: "synthetic-recovery-password",
    });
    expect(invoke).not.toHaveBeenCalledWith("restart_app");
    expect(invoke).not.toHaveBeenCalledWith(
      "publish_startup_upgrade_checkpoint",
      expect.anything(),
    );
    expect(invoke).not.toHaveBeenCalledWith(
      "cancel_startup_upgrade_checkpoint",
      expect.anything(),
    );
  });

  it("publishes only on an explicit checkpoint-bound click and coalesces duplicates", async () => {
    const prepared = review({
      status: "checkpoint_ready",
      canStartUpgrade: true,
    });
    let finish!: (value: ReturnType<typeof review>) => void;
    const response = new Promise<ReturnType<typeof review>>((resolve) => {
      finish = resolve;
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_startup_upgrade_review") return prepared;
      if (command === "publish_startup_upgrade_checkpoint") return response;
      if (command === "review_startup_upgrade_app")
        return appReview((args as { appType: string }).appType);
      throw new Error("synthetic unexpected command");
    });
    render(<StartupUpgrade />);
    const button = await screen.findByRole("button", {
      name: "startupUpgrade.start",
    });
    expect(invoke).toHaveBeenCalledTimes(1);
    fireEvent.click(button);
    fireEvent.click(button);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "publish_startup_upgrade_checkpoint",
        ),
    ).toHaveLength(1);
    expect(invoke).toHaveBeenCalledWith("publish_startup_upgrade_checkpoint", {
      expectedReviewToken: "synthetic-review",
      expectedCheckpointId: "synthetic-checkpoint",
    });
    finish(review({ reviewToken: "synthetic-published-review" }));
    await screen.findByRole("region", { name: "Claude Code" });
    expect(invoke).toHaveBeenCalledWith("review_startup_upgrade_app", {
      expectedReviewToken: "synthetic-published-review",
      appType: "claude",
    });
    expect(
      screen.queryByRole("button", { name: "startupUpgrade.cancel" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "startupUpgrade.complete" }),
    ).toBeDisabled();
  });

  it("queries a lost publication response without repeating publication or cancelling", async () => {
    serve(review({ status: "checkpoint_ready", canStartUpgrade: true }));
    render(<StartupUpgrade />);
    const button = await screen.findByRole("button", {
      name: "startupUpgrade.start",
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "publish_startup_upgrade_checkpoint")
        throw new Error("synthetic lost publication response");
      if (command === "get_startup_upgrade_review")
        return review({ reviewToken: "synthetic-reconciled-review" });
      if (command === "review_startup_upgrade_app")
        return appReview((args as { appType: string }).appType);
      throw new Error("synthetic unexpected command");
    });
    fireEvent.click(button);
    await screen.findByRole("region", { name: "Claude Code" });
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "publish_startup_upgrade_checkpoint",
        ),
    ).toHaveLength(1);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "get_startup_upgrade_review",
        ),
    ).toHaveLength(2);
    expect(invoke).not.toHaveBeenCalledWith(
      "cancel_startup_upgrade_checkpoint",
      expect.anything(),
    );
    expect(
      screen.queryByRole("button", { name: "startupUpgrade.start" }),
    ).not.toBeInTheDocument();
  });

  it("authenticates only after explicit input and clears the submitted password", async () => {
    serve(
      review({
        status: "authentication_required",
        canAuthenticate: true,
        reviewToken: null,
        checkpointId: null,
      }),
    );
    render(<StartupUpgrade />);
    const password = await screen.findByLabelText("secrets.password");
    expect(invoke).toHaveBeenCalledTimes(1);
    fireEvent.change(password, {
      target: { value: "synthetic protection password" },
    });
    vi.mocked(invoke).mockResolvedValueOnce(
      review({ status: "ready_to_check", canCheckAndBackup: true }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "startupUpgrade.authenticate" }),
    );
    await screen.findByRole("button", { name: "startupUpgrade.checkBackup" });
    expect(invoke).toHaveBeenLastCalledWith("authenticate_startup_upgrade", {
      password: "synthetic protection password",
    });
    expect(screen.queryByLabelText("secrets.password")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith(
      "unlock_secret_vault",
      expect.anything(),
    );
  });

  it("queries a lost backup response and retains the checkpoint without repeating creation", async () => {
    serve(
      review({
        status: "ready_to_check",
        checkpointPresent: false,
        checkpointId: null,
        canCheckAndBackup: true,
      }),
    );
    render(<StartupUpgrade />);
    const button = await screen.findByRole("button", {
      name: "startupUpgrade.checkBackup",
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "prepare_startup_upgrade_checkpoint")
        throw new Error("synthetic lost backup response");
      if (command === "get_startup_upgrade_review")
        return review({ status: "checkpoint_ready" });
      throw new Error("Unexpected synthetic command");
    });
    fireEvent.click(button);
    await screen.findByRole("button", { name: "startupUpgrade.cancel" });
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([name]) => name === "prepare_startup_upgrade_checkpoint",
        ),
    ).toEqual([
      [
        "prepare_startup_upgrade_checkpoint",
        { expectedReviewToken: "synthetic-review" },
      ],
    ]);
    expect(invoke).toHaveBeenLastCalledWith("get_startup_upgrade_review");
    expect(
      screen.queryByRole("button", { name: "startupUpgrade.checkBackup" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "startupUpgrade.complete" }),
    ).toBeDisabled();
  });

  it("queries first and never treats DB publication or an empty journal as completion", async () => {
    serve();
    render(<StartupUpgrade />);
    await screen.findByRole("region", { name: "Codex" });
    expect(
      screen.getByRole("button", { name: "startupUpgrade.complete" }),
    ).toBeDisabled();
    expect(recoveries()).toHaveLength(0);
    expect(invoke).not.toHaveBeenCalledWith("restart_app");
    expect(screen.getByText("startupUpgrade.databaseOnly")).toBeInTheDocument();
  });

  it("preserves reliable mode and keeps files without a takeover or Direct choice", async () => {
    serve(review(), {
      codex: {
        savedMode: "proxy",
        hasPendingOperation: true,
        canRecoverOperation: true,
      },
    });
    render(<StartupUpgrade />);
    const card = await screen.findByRole("region", { name: "Codex" });
    expect(
      await within(card).findByText("startupUpgrade.mode.proxy"),
    ).toBeInTheDocument();
    expect(
      within(card).getByText("startupUpgrade.keepFiles"),
    ).toBeInTheDocument();
    expect(within(card).queryByRole("combobox")).not.toBeInTheDocument();
    expect(within(card).queryByRole("checkbox")).not.toBeInTheDocument();
    expect(recoveries()).toHaveLength(0);
  });

  it("recovers only by explicit action with the latest app revision and coalesces repeated clicks", async () => {
    serve(review(), {
      codex: { hasPendingOperation: true, canRecoverOperation: true },
    });
    render(<StartupUpgrade />);
    const card = await screen.findByRole("region", { name: "Codex" });
    const button = within(card).getByRole("button", {
      name: "startupUpgrade.recover",
    });
    await waitFor(() => expect(button).toBeEnabled());
    let finish!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation((command) => {
      if (command === "recover_startup_upgrade_app")
        return new Promise((resolve) => {
          finish = resolve;
        });
      return Promise.reject(new Error("Unexpected synthetic command"));
    });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(recoveries()).toEqual([
      [
        "recover_startup_upgrade_app",
        {
          expectedReviewToken: "synthetic-review",
          appType: "codex",
          expectedAppRevision: "synthetic-codex-revision",
        },
      ],
    ]);
    finish(appReview("codex"));
    await waitFor(() => expect(button).toBeDisabled());
    expect(
      screen.getByRole("button", { name: "startupUpgrade.complete" }),
    ).toBeDisabled();
  });

  it("queries after a lost recovery response without repeating its side effects", async () => {
    serve(review(), {
      codex: { hasPendingOperation: true, canRecoverOperation: true },
    });
    render(<StartupUpgrade />);
    const card = await screen.findByRole("region", { name: "Codex" });
    await waitFor(() =>
      expect(
        within(card).getByRole("button", { name: "startupUpgrade.recover" }),
      ).toBeEnabled(),
    );
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "recover_startup_upgrade_app")
        throw new Error("synthetic lost response");
      if (command === "review_startup_upgrade_app")
        return appReview("codex", {
          revision: "synthetic-new-revision",
          hasPendingOperation: true,
          canRecoverOperation: false,
        });
      throw new Error("Unexpected synthetic command");
    });
    fireEvent.click(
      within(card).getByRole("button", { name: "startupUpgrade.recover" }),
    );
    await within(card).findByRole("alert");
    await waitFor(() =>
      expect(
        within(card).getByRole("button", { name: "startupUpgrade.recover" }),
      ).toBeDisabled(),
    );
    expect(recoveries()).toHaveLength(1);
    expect(invoke).toHaveBeenLastCalledWith("review_startup_upgrade_app", {
      expectedReviewToken: "synthetic-review",
      appType: "codex",
    });
  });

  it("keeps an unresolved app isolated while another app can query and recover", async () => {
    serve(review(), {
      claude: { hasPendingOperation: true, canRecoverOperation: true },
    });
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation((command, args) => {
      if (
        command === "review_startup_upgrade_app" &&
        (args as { appType: string }).appType === "codex"
      )
        return Promise.reject(new Error("synthetic app conflict"));
      return original(command, args);
    });
    render(<StartupUpgrade />);
    const codex = await screen.findByRole("region", { name: "Codex" });
    await within(codex).findByRole("alert");
    const claude = screen.getByRole("region", { name: "Claude Code" });
    await waitFor(() =>
      expect(
        within(claude).getByRole("button", { name: "startupUpgrade.recover" }),
      ).toBeEnabled(),
    );
    expect(
      within(claude).getByRole("button", { name: "startupUpgrade.recheck" }),
    ).toBeEnabled();
  });

  it("cancels only a verified unpublished checkpoint with original identifiers", async () => {
    serve(review({ status: "checkpoint_ready" }));
    render(<StartupUpgrade />);
    const cancel = await screen.findByRole("button", {
      name: "startupUpgrade.cancel",
    });
    vi.mocked(invoke).mockResolvedValueOnce(
      review({
        status: "cancelled",
        checkpointPresent: false,
        checkpointId: null,
      }),
    );
    fireEvent.click(cancel);
    await waitFor(() =>
      expect(invoke).toHaveBeenLastCalledWith(
        "cancel_startup_upgrade_checkpoint",
        {
          expectedReviewToken: "synthetic-review",
          expectedCheckpointId: "synthetic-checkpoint",
        },
      ),
    );
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "startupUpgrade.cancel" }),
      ).not.toBeInTheDocument(),
    );
    expect(invoke).not.toHaveBeenCalledWith("restart_app");
  });

  it("leaves published facts intact on later/exit without invoking cancellation or restart", async () => {
    serve();
    vi.mocked(exit).mockResolvedValue();
    render(<StartupUpgrade />);
    fireEvent.click(
      await screen.findByRole("button", { name: "startupUpgrade.later" }),
    );
    await waitFor(() => expect(exit).toHaveBeenCalledWith(0));
    expect(
      vi
        .mocked(invoke)
        .mock.calls.some(([name]) => /cancel|recover|restart/.test(name)),
    ).toBe(false);
    expect(
      screen.getByText("startupUpgrade.publishedExit"),
    ).toBeInTheDocument();
  });

  it("disables stale actions after a query failure and does not show raw errors", async () => {
    vi.mocked(invoke).mockRejectedValue(
      new Error("synthetic private backend detail"),
    );
    render(<StartupUpgrade />);
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", { name: "startupUpgrade.complete" }),
    ).toBeDisabled();
    expect(
      screen.queryByText("synthetic private backend detail"),
    ).not.toBeInTheDocument();
    expect(recoveries()).toHaveLength(0);
  });
});
