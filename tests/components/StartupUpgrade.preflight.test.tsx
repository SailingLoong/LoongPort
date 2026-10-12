import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { StartupUpgrade } from "@/components/StartupUpgrade";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ exit: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const prepared = {
  status: "checkpoint_ready",
  checkpointPresent: true,
  checkpointId: "synthetic-checkpoint",
  reviewToken: "synthetic-review",
  canAuthenticate: false,
  canCheckAndBackup: false,
  canStartUpgrade: true,
};
const facts = () => ({
  checkpointId: prepared.checkpointId,
  sourceVersions: { upstream: 17, loongport: 24 },
  stagedVersions: { upstream: 20, loongport: 24 },
  canStartUpgrade: false,
  apps: ["claude", "codex", "gemini", "grokbuild"].map((appType) => ({
    appType,
    providerCount: 1,
    savedMode: appType === "claude" ? "proxy" : null,
    modeResolution: appType === "claude" ? "preserved" : "missing",
    providerResolution: appType === "codex" ? "conflict" : "preserved",
    requiresModeChoice: appType !== "claude",
    requiresProviderChoice: appType === "codex",
    hasPendingOperation: false,
    liveStatus: "parsed",
    storedFieldsMatch: appType === "claude" ? false : null,
    defaultAction: "keep_files",
    defaultTakeover: false,
  })),
});

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

describe("original upgrade difference-card preflight", () => {
  it("reads the original checkpoint ownership before enabling explicit publication", async () => {
    let finish!: (value: ReturnType<typeof facts>) => void;
    const pending = new Promise<ReturnType<typeof facts>>((resolve) => {
      finish = resolve;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_startup_upgrade_review") return prepared;
      if (command === "review_startup_upgrade_ownership") return pending;
      throw new Error(`synthetic unexpected action: ${command}`);
    });
    render(<StartupUpgrade />);
    const start = await screen.findByRole("button", {
      name: "startupUpgrade.start",
    });
    expect(start).toBeDisabled();
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("review_startup_upgrade_ownership", {
        expectedReviewToken: prepared.reviewToken,
      }),
    );
    fireEvent.click(start);
    expect(invoke).not.toHaveBeenCalledWith(
      "publish_startup_upgrade_checkpoint",
      expect.anything(),
    );
    finish(facts());
    await screen.findByRole("region", { name: "Claude Code" });
    await waitFor(() => expect(start).toBeEnabled());
    const claude = screen.getByRole("region", { name: "Claude Code" });
    expect(
      within(claude).getByText("startupUpgrade.mode.proxy"),
    ).toBeInTheDocument();
    expect(
      within(claude).getByText("startupUpgrade.resolution.preserved"),
    ).toBeInTheDocument();
    expect(
      within(claude).getByText("startupUpgrade.fieldsLabel").nextElementSibling,
    ).toHaveTextContent("startupUpgrade.no");
    expect(
      within(claude).getByText("startupUpgrade.journalLabel")
        .nextElementSibling,
    ).toHaveTextContent("startupUpgrade.no");
    const codex = screen.getByRole("region", { name: "Codex" });
    expect(
      within(codex).getByText("startupUpgrade.modeLabel").nextElementSibling,
    ).toHaveTextContent("startupUpgrade.unknown");
    expect(
      within(codex).getByText("startupUpgrade.fieldsLabel").nextElementSibling,
    ).toHaveTextContent("startupUpgrade.unknown");
    expect(
      within(codex).getByText("startupUpgrade.resolution.conflict"),
    ).toBeInTheDocument();
    expect(
      within(codex).getByText("startupUpgrade.modeChoiceRequired"),
    ).toBeInTheDocument();
    expect(
      within(codex).getByText("startupUpgrade.providerChoiceRequired"),
    ).toBeInTheDocument();
    expect(
      within(claude).queryByText("startupUpgrade.modeChoiceRequired"),
    ).not.toBeInTheDocument();
    expect(
      within(claude).queryByText("startupUpgrade.providerChoiceRequired"),
    ).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith(
      "recover_startup_upgrade_app",
      expect.anything(),
    );
  });

  it("discards an old ownership response after the original root query changes token", async () => {
    let finish!: (value: ReturnType<typeof facts>) => void;
    let finishFresh!: (value: ReturnType<typeof facts>) => void;
    const pending = new Promise<ReturnType<typeof facts>>((resolve) => {
      finish = resolve;
    });
    const freshResponse = new Promise<ReturnType<typeof facts>>((resolve) => {
      finishFresh = resolve;
    });
    let rootQueries = 0;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_startup_upgrade_review") {
        rootQueries++;
        return rootQueries === 1
          ? prepared
          : { ...prepared, reviewToken: "synthetic-new-review" };
      }
      if (command === "review_startup_upgrade_ownership")
        return (args as { expectedReviewToken: string }).expectedReviewToken ===
          prepared.reviewToken
          ? pending
          : freshResponse;
      throw new Error("synthetic unexpected action");
    });
    render(<StartupUpgrade />);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("review_startup_upgrade_ownership", {
        expectedReviewToken: prepared.reviewToken,
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "startupUpgrade.recheck" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("review_startup_upgrade_ownership", {
        expectedReviewToken: "synthetic-new-review",
      }),
    );
    finish(facts());
    await pending;
    expect(
      screen.getByRole("button", { name: "startupUpgrade.start" }),
    ).toBeDisabled();
    expect(
      screen.queryByRole("region", { name: "Claude Code" }),
    ).not.toBeInTheDocument();
    finishFresh(facts());
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "startupUpgrade.start" }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "startupUpgrade.start" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "publish_startup_upgrade_checkpoint",
        {
          expectedReviewToken: "synthetic-new-review",
          expectedCheckpointId: prepared.checkpointId,
        },
      ),
    );
    expect(invoke).not.toHaveBeenCalledWith(
      "publish_startup_upgrade_checkpoint",
      {
        expectedReviewToken: prepared.reviewToken,
        expectedCheckpointId: prepared.checkpointId,
      },
    );
  });

  it.each(["different-checkpoint", "rejected-query"])(
    "keeps publication blocked after %s without replay",
    async (failure) => {
      vi.mocked(invoke).mockImplementation(async (command) => {
        if (command === "get_startup_upgrade_review") return prepared;
        if (command === "review_startup_upgrade_ownership") {
          if (failure === "rejected-query")
            throw new Error("synthetic private query failure");
          return { ...facts(), checkpointId: "synthetic-different-checkpoint" };
        }
        throw new Error(`synthetic unexpected action: ${command}`);
      });
      render(<StartupUpgrade />);
      await screen.findByRole("alert");
      const start = screen.getByRole("button", {
        name: "startupUpgrade.start",
      });
      expect(start).toBeDisabled();
      fireEvent.click(start);
      expect(invoke).not.toHaveBeenCalledWith(
        "publish_startup_upgrade_checkpoint",
        expect.anything(),
      );
      expect(
        screen.queryByText("synthetic private query failure"),
      ).not.toBeInTheDocument();
      expect(
        vi
          .mocked(invoke)
          .mock.calls.filter(
            ([command]) => command === "review_startup_upgrade_ownership",
          ),
      ).toHaveLength(1);
    },
  );
});
