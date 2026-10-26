import { createInstance } from "i18next";
import zh from "@/i18n/locales/zh.json";
import en from "@/i18n/locales/en.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import ja from "@/i18n/locales/ja.json";
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
const ownershipReview = () => ({
  checkpointId: "synthetic-checkpoint",
  apps: ["claude", "codex", "gemini", "grokbuild"].map((appType) => ({
    appType,
    savedMode: "direct",
    providerResolution: "preserved",
    hasPendingOperation: false,
    storedFieldsMatch: null,
    requiresModeChoice: false,
    requiresProviderChoice: false,
  })),
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
    if (command === "review_startup_upgrade_ownership")
      return ownershipReview();
    if (command === "review_startup_upgrade_app") {
      const app = (args as { appType: string }).appType;
      return appReview(app, overrides[app]);
    }
    throw new Error(`Unexpected synthetic command: ${command}`);
  });
}

describe("StartupUpgrade", () => {
  it.each(["direct", "proxy"])(
    "requires an explicit missing mode and independent route draft for %s",
    async (mode) => {
      serve(review(), {
        claude: {
          savedMode: null,
          canChooseMode: true,
          canChooseProvider: false,
          retainedProviderId: "synthetic-direct",
          keepFilesProviders: [
            { id: "synthetic-direct", name: "Synthetic direct" },
          ],
          modeRouteProviders: [
            { id: "synthetic-direct", name: "Synthetic direct" },
            { id: "synthetic-route", name: "Synthetic route" },
          ],
        },
      });
      render(<StartupUpgrade />);
      const card = await screen.findByRole("region", { name: "Claude Code" });
      const apply = await within(card).findByRole("button", {
        name: "startupUpgrade.keepFiles",
      });
      const direct = within(card).getByRole("button", {
        name: "startupUpgrade.chooseMode.direct",
      });
      const proxy = within(card).getByRole("button", {
        name: "startupUpgrade.chooseMode.proxy",
      });
      expect(direct).toHaveAttribute("aria-pressed", "false");
      expect(proxy).toHaveAttribute("aria-pressed", "false");
      expect(apply).toBeDisabled();
      expect(invoke).not.toHaveBeenCalledWith(
        "select_startup_upgrade_mode",
        expect.anything(),
      );
      fireEvent.click(mode === "direct" ? direct : proxy);
      if (mode === "proxy") {
        expect(apply).toBeDisabled();
        expect(
          within(card).getByRole("button", { name: "Synthetic direct" }),
        ).toHaveAttribute("aria-pressed", "false");
        fireEvent.click(
          within(card).getByRole("button", { name: "Synthetic route" }),
        );
      }
      expect(apply).toBeEnabled();
      const original = vi.mocked(invoke).getMockImplementation()!;
      vi.mocked(invoke).mockImplementation(async (command, args) =>
        command === "select_startup_upgrade_mode"
          ? appReview("claude", {
              savedMode: mode,
              canCompleteApp: true,
              canChooseMode: false,
            })
          : original(command, args),
      );
      fireEvent.click(apply);
      await waitFor(() =>
        expect(invoke).toHaveBeenCalledWith("select_startup_upgrade_mode", {
          expectedReviewToken: "synthetic-review",
          appType: "claude",
          expectedAppRevision: "synthetic-claude-revision",
          choice: {
            mode,
            proxyRoute: mode === "proxy" ? "synthetic-route" : null,
          },
        }),
      );
      await waitFor(() =>
        expect(
          within(card).queryByRole("button", {
            name: "startupUpgrade.chooseMode.direct",
          }),
        ).not.toBeInTheDocument(),
      );
    },
  );

  it("queries a lost mode-choice response without replaying the original intent", async () => {
    serve(review(), {
      claude: {
        savedMode: null,
        canChooseMode: true,
        retainedProviderId: "synthetic-direct",
        keepFilesProviders: [
          { id: "synthetic-direct", name: "Synthetic direct" },
        ],
        modeRouteProviders: [],
      },
    });
    render(<StartupUpgrade />);
    const card = await screen.findByRole("region", { name: "Claude Code" });
    fireEvent.click(
      await within(card).findByRole("button", {
        name: "startupUpgrade.chooseMode.direct",
      }),
    );
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "select_startup_upgrade_mode")
        throw new Error("synthetic lost response");
      if (
        command === "review_startup_upgrade_app" &&
        (args as { appType: string }).appType === "claude"
      )
        return appReview("claude", {
          hasPendingOperation: true,
          canRecoverOperation: true,
          canChooseMode: false,
        });
      return original(command, args);
    });
    fireEvent.click(
      within(card).getByRole("button", { name: "startupUpgrade.keepFiles" }),
    );
    await waitFor(() =>
      expect(
        within(card).getByRole("button", { name: "startupUpgrade.recover" }),
      ).toBeEnabled(),
    );
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "select_startup_upgrade_mode",
        ),
    ).toHaveLength(1);
    expect(recoveries()).toHaveLength(0);
  });

  it("requires an explicit provider draft for conflict and submits only the original keep-files action", async () => {
    serve(review(), {
      claude: {
        directProviderResolution: "conflict",
        retainedProviderId: null,
        keepFilesProviders: [
          { id: "synthetic-native", name: "Synthetic native" },
        ],
        canChooseProvider: true,
        pointerConsistent: false,
      },
    });
    render(<StartupUpgrade />);
    const card = await screen.findByRole("region", { name: "Claude Code" });
    const button = await within(card).findByRole("button", {
      name: "startupUpgrade.keepFiles",
    });
    expect(button).toBeDisabled();
    expect(invoke).not.toHaveBeenCalledWith(
      "select_startup_upgrade_provider",
      expect.anything(),
    );
    fireEvent.click(
      within(card).getByRole("button", { name: "Synthetic native" }),
    );
    expect(button).toBeEnabled();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "select_startup_upgrade_provider")
        return appReview("claude", { canCompleteApp: true });
      if (command === "review_startup_upgrade_app")
        return appReview((args as { appType: string }).appType);
      if (command === "get_startup_upgrade_review") return review();
      throw new Error("synthetic unexpected action");
    });
    fireEvent.click(button);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("select_startup_upgrade_provider", {
        expectedReviewToken: "synthetic-review",
        appType: "claude",
        expectedAppRevision: "synthetic-claude-revision",
        providerId: "synthetic-native",
      }),
    );
    expect(
      within(card).getByText("startupUpgrade.mode.direct"),
    ).toBeInTheDocument();
  });

  it("queries a lost provider-selection response and never replays the choice", async () => {
    let selected = false;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_startup_upgrade_review") return review();
      if (command === "review_startup_upgrade_app") {
        const app = (args as { appType: string }).appType;
        return appReview(
          app,
          app === "claude" && !selected
            ? {
                directProviderResolution: "missing",
                retainedProviderId: null,
                keepFilesProviders: [
                  { id: "synthetic-native", name: "Synthetic native" },
                ],
                canChooseProvider: true,
                pointerConsistent: false,
              }
            : {},
        );
      }
      if (command === "select_startup_upgrade_provider") {
        selected = true;
        throw new Error("synthetic lost response");
      }
      throw new Error("synthetic unexpected action");
    });
    render(<StartupUpgrade />);
    const card = await screen.findByRole("region", { name: "Claude Code" });
    fireEvent.click(
      await within(card).findByRole("button", { name: "Synthetic native" }),
    );
    fireEvent.click(
      within(card).getByRole("button", { name: "startupUpgrade.keepFiles" }),
    );
    await waitFor(() =>
      expect(
        vi
          .mocked(invoke)
          .mock.calls.filter(
            ([command]) => command === "select_startup_upgrade_provider",
          ),
      ).toHaveLength(1),
    );
    await waitFor(() =>
      expect(
        vi
          .mocked(invoke)
          .mock.calls.filter(
            ([command, args]) =>
              command === "review_startup_upgrade_app" &&
              (args as { appType: string }).appType === "claude",
          ).length,
      ).toBeGreaterThan(1),
    );
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "select_startup_upgrade_provider",
        ),
    ).toHaveLength(1);
  });

  it("preserves a generation intent without reintroducing a recovery password form", async () => {
    serve(review({ status: "recovery_required" }));
    render(<StartupUpgrade />);
    await screen.findByText("secrets.runtimeUnavailable");
    expect(screen.queryByLabelText("secrets.password")).not.toBeInTheDocument();
    for (const command of [
      "get_startup_recovery",
      "recover_startup_operation",
      "publish_startup_upgrade_checkpoint",
      "cancel_startup_upgrade_checkpoint",
      "restart_app",
    ]) {
      expect(
        vi.mocked(invoke).mock.calls.some(([name]) => name === command),
      ).toBe(false);
    }
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
      if (command === "review_startup_upgrade_ownership")
        return ownershipReview();
      if (command === "publish_startup_upgrade_checkpoint") return response;
      if (command === "review_startup_upgrade_app")
        return appReview((args as { appType: string }).appType);
      throw new Error("synthetic unexpected command");
    });
    render(<StartupUpgrade />);
    const button = await screen.findByRole("button", {
      name: "startupUpgrade.start",
    });
    await waitFor(() => expect(button).toBeEnabled());
    expect(invoke).toHaveBeenCalledTimes(2);
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
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("review_startup_upgrade_app", {
        expectedReviewToken: "synthetic-published-review",
        appType: "claude",
      }),
    );
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
    await waitFor(() => expect(button).toBeEnabled());
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

  it("does not offer manual password authentication for a retained old checkpoint", async () => {
    serve(
      review({
        status: "authentication_required",
        canAuthenticate: true,
        reviewToken: null,
        checkpointId: null,
      }),
    );
    render(<StartupUpgrade />);
    await screen.findByText("secrets.runtimeUnavailable");
    expect(screen.queryByLabelText("secrets.password")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "startupUpgrade.authenticate" }),
    ).not.toBeInTheDocument();
    expect(vi.mocked(invoke).mock.calls.map(([name]) => name)).toEqual([
      "get_startup_upgrade_review",
    ]);
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

  it("clears stale recovery uncertainty only after original native completion readback", async () => {
    serve(review(), {
      claude: { hasPendingOperation: true, canRecoverOperation: true },
    });
    render(<StartupUpgrade />);
    const card = await screen.findByRole("region", { name: "Claude Code" });
    const recover = within(card).getByRole("button", {
      name: "startupUpgrade.recover",
    });
    await waitFor(() => expect(recover).toBeEnabled());
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "recover_startup_upgrade_app")
        throw new Error("synthetic lost response");
      if (command === "review_startup_upgrade_app")
        return appReview("claude", {
          revision: "synthetic-completed-revision",
          canCompleteApp: true,
        });
      throw new Error("synthetic unexpected action");
    });
    fireEvent.click(recover);
    await waitFor(() =>
      expect(invoke).toHaveBeenLastCalledWith("review_startup_upgrade_app", {
        expectedReviewToken: "synthetic-review",
        appType: "claude",
      }),
    );
    await waitFor(() =>
      expect(
        within(card).getByText("startupUpgrade.journalLabel")
          .nextElementSibling,
      ).toHaveTextContent("startupUpgrade.no"),
    );
    await waitFor(() =>
      expect(within(card).queryByRole("alert")).not.toBeInTheDocument(),
    );
    expect(recover).toBeDisabled();
    expect(recoveries()).toHaveLength(1);
    expect(
      screen.getByRole("button", { name: "startupUpgrade.complete" }),
    ).toBeDisabled();
    expect(invoke).not.toHaveBeenCalledWith("restart_app");
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

describe("explicit runtime handoff", () => {
  it("keeps files until an explicit handoff and returns to the workbench only after success", async () => {
    const onReady = vi.fn();
    serve();
    render(<StartupUpgrade onReady={onReady} />);
    const button = await screen.findByRole("button", {
      name: "startupUpgrade.continue",
    });
    expect(
      vi
        .mocked(invoke)
        .mock.calls.some(([command]) => command === "continue_startup_upgrade"),
    ).toBe(false);
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "continue_startup_upgrade") {
        expect(args).toEqual({ expectedReviewToken: "synthetic-review" });
        return null;
      }
      if (command === "get_startup_upgrade_review") return review();
      if (command === "review_startup_upgrade_app")
        return appReview((args as { appType: string }).appType);
      throw new Error("synthetic unexpected command");
    });
    fireEvent.click(button);
    await waitFor(() => expect(onReady).toHaveBeenCalledTimes(1));
    expect(recoveries()).toHaveLength(0);
    expect(
      screen.getByRole("button", { name: "startupUpgrade.complete" }),
    ).toBeDisabled();
  });

  it("queries a lost handoff response without replaying or entering the workbench", async () => {
    const onReady = vi.fn();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_startup_upgrade_review") return review();
      if (command === "review_startup_upgrade_app")
        return appReview((args as { appType: string }).appType);
      if (command === "continue_startup_upgrade")
        throw new Error("synthetic lost response");
      throw new Error("synthetic unexpected command");
    });
    render(<StartupUpgrade onReady={onReady} />);
    fireEvent.click(
      await screen.findByRole("button", { name: "startupUpgrade.continue" }),
    );
    await screen.findByRole("alert");
    await waitFor(() =>
      expect(
        vi
          .mocked(invoke)
          .mock.calls.filter(([c]) => c === "get_startup_upgrade_review"),
      ).toHaveLength(2),
    );
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(([c]) => c === "continue_startup_upgrade"),
    ).toHaveLength(1);
    expect(onReady).not.toHaveBeenCalled();
    expect(recoveries()).toHaveLength(0);
  });
});

it("does not route runtime preparation failure into the retired unlock form", async () => {
  serve(review({ status: "restart_required", reviewToken: null }));
  render(<StartupUpgrade />);
  await screen.findByText("secrets.runtimeUnavailable");
  expect(
    screen.queryByRole("button", { name: "secrets.restart" }),
  ).not.toBeInTheDocument();
  expect(vi.mocked(invoke).mock.calls.map(([name]) => name)).toEqual([
    "get_startup_upgrade_review",
  ]);
});

describe("retained checkpoint sync explanation", () => {
  it.each([
    ["checkpoint_ready", "synthetic-review"],
    ["database_verified", null],
  ])(
    "explains retained checkpoint without promising a workbench for %s/%s",
    async (status, reviewToken) => {
      serve(review({ status, reviewToken }));
      render(<StartupUpgrade />);
      expect(
        await screen.findByText("startupUpgrade.syncPausedReason"),
      ).toBeVisible();
      expect(
        screen.queryByText("startupUpgrade.localContinueHint"),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: "startupUpgrade.continue" }),
      ).not.toBeInTheDocument();
      expect(
        vi
          .mocked(invoke)
          .mock.calls.every(([command]) =>
            [
              "get_startup_upgrade_review",
              "review_startup_upgrade_ownership",
              "review_startup_upgrade_app",
            ].includes(command),
          ),
      ).toBe(true);
    },
  );

  it("explains authenticated local continuation and keeps repeated rechecks read-only", async () => {
    serve();
    render(<StartupUpgrade />);
    expect(
      await screen.findByText("startupUpgrade.localContinueHint"),
    ).toBeVisible();
    expect(screen.getByText("startupUpgrade.syncPausedReason")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "startupUpgrade.continue" }),
    ).toBeEnabled();
    for (let i = 0; i < 2; i++) {
      fireEvent.click(
        screen.getAllByRole("button", { name: "startupUpgrade.recheck" })[0],
      );
      await screen.findByText("startupUpgrade.localContinueHint");
    }
    expect(
      vi
        .mocked(invoke)
        .mock.calls.every(([command]) =>
          [
            "get_startup_upgrade_review",
            "review_startup_upgrade_ownership",
            "review_startup_upgrade_app",
          ].includes(command),
        ),
    ).toBe(true);
    expect(
      screen.getByRole("button", { name: "startupUpgrade.complete" }),
    ).toBeDisabled();
  });

  it("does not infer paused sync or local availability without a checkpoint", async () => {
    serve(
      review({
        status: "preflight",
        checkpointPresent: false,
        checkpointId: null,
        reviewToken: null,
      }),
    );
    render(<StartupUpgrade />);
    await screen.findByText("startupUpgrade.verificationRequired");
    expect(
      screen.queryByText("startupUpgrade.syncPausedReason"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("startupUpgrade.localContinueHint"),
    ).not.toBeInTheDocument();
  });
});

it.each([
  ["zh", zh],
  ["en", en],
  ["zh-TW", zhTW],
  ["ja", ja],
] as const)(
  "uses existing labels in the %s sync pause explanation",
  async (lng, translation) => {
    const i18n = createInstance();
    await i18n.init({
      lng,
      resources: { [lng]: { translation } },
      interpolation: { escapeValue: false },
    });
    const labels = {
      settings: i18n.t("common.settings"),
      auth: i18n.t("settings.tabAuth"),
      protection: i18n.t("secrets.protectionTitle"),
      review: i18n.t("startupUpgrade.reviewApps"),
    };
    const hint = i18n.t("startupUpgrade.syncReviewHint", labels);
    for (const label of Object.values(labels)) expect(hint).toContain(label);
    expect(hint).not.toMatch(/{{|startupUpgrade\./);
    const continuation = i18n.t("startupUpgrade.localContinueHint", {
      continue: i18n.t("startupUpgrade.continue"),
    });
    expect(continuation).toContain(translation.startupUpgrade.continue);
    expect(continuation).not.toMatch(/{{|startupUpgrade\./);
    expect(i18n.t("startupUpgrade.syncPausedReason")).toBe(
      translation.startupUpgrade.syncPausedReason,
    );
  },
);

describe("local upgrade completion", () => {
  const completed = () =>
    review({
      status: "completed",
      checkpointPresent: false,
      checkpointId: null,
      reviewToken: null,
      canCompleteUpgrade: false,
      archiveRetained: true,
      syncPaused: true,
    });

  it("completes only the reviewed checkpoint and keeps recovery and sync facts distinct", async () => {
    serve(review({ canCompleteUpgrade: true }));
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args) =>
      command === "complete_startup_upgrade"
        ? completed()
        : original(command, args),
    );
    render(<StartupUpgrade embedded />);
    const button = await screen.findByRole("button", {
      name: "startupUpgrade.complete",
    });
    await waitFor(() => expect(button).toBeEnabled());
    expect(invoke).not.toHaveBeenCalledWith(
      "complete_startup_upgrade",
      expect.anything(),
    );
    fireEvent.click(button);
    await screen.findByText("startupUpgrade.completed");
    expect(invoke).toHaveBeenCalledWith("complete_startup_upgrade", {
      expectedReviewToken: "synthetic-review",
      expectedCheckpointId: "synthetic-checkpoint",
    });
    expect(screen.getByText("startupUpgrade.archiveRetained")).toBeVisible();
    expect(
      screen.getByText("startupUpgrade.completedSyncPaused"),
    ).toBeVisible();
    expect(
      screen.queryByText("startupUpgrade.notComplete"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("startupUpgrade.syncPausedReason"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "startupUpgrade.complete" }),
    ).toBeDisabled();
    expect(invoke).not.toHaveBeenCalledWith(
      "confirm_upgrade_sync_resume",
      expect.anything(),
    );
  });

  it("queries a lost completion response without replaying completion", async () => {
    let finished = false;
    serve(review({ canCompleteUpgrade: true }));
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "complete_startup_upgrade") {
        finished = true;
        throw new Error("synthetic lost completion response");
      }
      if (command === "get_startup_upgrade_review" && finished)
        return completed();
      return original(command, args);
    });
    const onReady = vi.fn();
    render(<StartupUpgrade onReady={onReady} />);
    const button = await screen.findByRole("button", {
      name: "startupUpgrade.complete",
    });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    fireEvent.click(button);
    await screen.findByText("startupUpgrade.completed");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "complete_startup_upgrade",
        ),
    ).toHaveLength(1);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "get_startup_upgrade_review",
        ),
    ).toHaveLength(2);
    fireEvent.click(
      screen.getByRole("button", { name: "startupUpgrade.continue" }),
    );
    expect(onReady).toHaveBeenCalledTimes(1);
    expect(invoke).not.toHaveBeenCalledWith(
      "continue_startup_upgrade",
      expect.anything(),
    );
  });

  it("shows backend-proven unused apps without matched native facts", async () => {
    serve(review(), {
      gemini: {
        notUsed: true,
        savedMode: null,
        pointerConsistent: null,
        storedFieldsMatch: null,
      },
    });
    render(<StartupUpgrade />);
    const card = await screen.findByRole("region", { name: "Gemini CLI" });
    expect(
      await within(card).findByText("startupUpgrade.notUsed"),
    ).toBeVisible();
    expect(
      within(card).queryByText("startupUpgrade.fieldsLabel"),
    ).not.toBeInTheDocument();
    expect(
      within(card).queryByText("startupUpgrade.nativeProvider"),
    ).not.toBeInTheDocument();
    expect(
      within(card).queryByRole("button", { name: "startupUpgrade.recover" }),
    ).not.toBeInTheDocument();
  });

  it("does not infer retained recovery or paused sync when completed fields are missing", async () => {
    serve(review({ status: "completed", checkpointPresent: false }));
    render(<StartupUpgrade />);
    await screen.findByText("startupUpgrade.completed");
    expect(
      screen.queryByText("startupUpgrade.archiveRetained"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("startupUpgrade.completedSyncPaused"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "startupUpgrade.complete" }),
    ).toBeDisabled();
  });
});

it("queries an unknown completion response and blocks the stale complete action if readback fails", async () => {
  let attempted = false;
  serve(review({ canCompleteUpgrade: true }));
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "complete_startup_upgrade") {
      attempted = true;
      return undefined;
    }
    if (command === "get_startup_upgrade_review" && attempted)
      throw new Error("synthetic unreadable response");
    return original(command, args);
  });
  render(<StartupUpgrade />);
  const button = await screen.findByRole("button", {
    name: "startupUpgrade.complete",
  });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
  await screen.findByRole("alert");
  expect(
    screen.getByRole("button", { name: "startupUpgrade.complete" }),
  ).toBeDisabled();
  expect(
    vi
      .mocked(invoke)
      .mock.calls.filter(
        ([command]) => command === "get_startup_upgrade_review",
      ),
  ).toHaveLength(2);
  expect(
    vi
      .mocked(invoke)
      .mock.calls.filter(([command]) => command === "complete_startup_upgrade"),
  ).toHaveLength(1);
});

it.each([zh, en, zhTW, ja])(
  "localizes completion and independent saved sync confirmation",
  (translation) => {
    for (const key of [
      "completed",
      "archiveRetained",
      "completedSyncPaused",
      "notUsed",
      "syncResumeReview",
      "syncResumeSavedConfig",
      "syncResumeConfirm",
      "syncResumeConfirmed",
      "syncResumeUnavailable",
    ] as const) {
      expect(translation.startupUpgrade[key]).toBeTruthy();
      expect(translation.startupUpgrade[key]).not.toMatch(
        /startupUpgrade\.|4\.x/,
      );
    }
    expect(
      translation.startupUpgrade.syncResumeChoice.independentConfig,
    ).toBeTruthy();
    expect(
      translation.startupUpgrade.syncResumeChoice.compatibleDevices,
    ).toBeTruthy();
  },
);
