import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  LeaderboardKind,
  RelayDirectoryItem,
  RelayLeaderboard,
} from "@/lib/api/relay";

const {
  listDirectory,
  importSite,
  importDirectorySite,
  refresh,
  openInBrowser,
  toastError,
  toastSuccess,
  toastWarning,
} = vi.hoisted(() => ({
  listDirectory: vi.fn(),
  importSite: vi.fn(),
  importDirectorySite: vi.fn(),
  refresh: vi.fn(),
  openInBrowser: vi.fn(),
  toastError: vi.fn(),
  toastSuccess: vi.fn(),
  toastWarning: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  relayApi: { listDirectory, importSite, importDirectorySite, refresh },
}));

vi.mock("../../openInBrowser", () => ({ openInBrowser }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) =>
      options ? `${key} ${JSON.stringify(options)}` : key,
    i18n: { resolvedLanguage: "zh" },
  }),
}));

vi.mock("sonner", () => ({
  toast: {
    success: toastSuccess,
    error: toastError,
    warning: toastWarning,
    info: vi.fn(),
  },
}));

const { RelayDirectoryPage } = await import("../RelayDirectoryPage");

function item(index: number): RelayDirectoryItem {
  return {
    siteHost: index === 1 ? "bestapi.store" : `site-${index}.example`,
    veridropHost: index === 1 ? "bestapi.store" : `probe-${index}.example`,
    displayName: index === 1 ? "BestAPI" : `站点 ${index}`,
    rank: index,
    score: index === 1 ? 99 : 90,
    samples: index === 1 ? 20 : 10,
    latestDate: "2026-08-12",
    detailUrl:
      index === 1
        ? "https://veridrop.org/leaderboard/bestapi.store"
        : `https://veridrop.org/leaderboard/site-${index}.example`,
    protocolScores:
      index === 1
        ? [
            {
              protocol: "Claude",
              score: 99,
              samples: 20,
              verdict: "通过",
              reportUrl: "https://veridrop.org/r/claude-best",
            },
            {
              protocol: "OpenAI",
              score: 95,
              samples: 27,
              verdict: "通过",
              reportUrl: null,
            },
          ]
        : [],
    claudeSignatureRate: index === 1 ? 95 : null,
    scenarios: index === 1 ? ["Claude Code / Cursor 编程"] : [],
    issues: index === 1 ? ["token_usage"] : [],
    entryUrl:
      index === 1 ? "https://bestapi.store" : `https://site-${index}.example`,
  };
}

function leaderboard(
  kind: LeaderboardKind,
  overrides: Partial<RelayLeaderboard> = {},
): RelayLeaderboard {
  return {
    kind,
    items: Array.from({ length: 13 }, (_, index) => item(index + 1)),
    syncedAt: 1786633200,
    fromCache: false,
    ...overrides,
  };
}

describe("RelayDirectoryPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listDirectory.mockImplementation((kind: LeaderboardKind) =>
      Promise.resolve(leaderboard(kind)),
    );
    importSite.mockResolvedValue({
      relayId: 7,
      siteOrigin: "https://bestapi.store",
      siteName: "BestAPI",
      backendKind: "sub2api",
    });
    importDirectorySite.mockResolvedValue({
      relayId: 7,
      siteOrigin: "https://bestapi.store",
      siteName: "BestAPI",
      backendKind: "sub2api",
    });
    refresh.mockResolvedValue({
      summary: {
        notice: "updated",
        refreshedAccounts: 1,
        tiers: 0,
        keysCreated: 0,
        otherPlatformTiers: 0,
        mergedProviders: 0,
        failures: [],
      },
      balances: [],
    });
  });

  it("defaults Codex to OpenAI and exposes all four leaderboard tabs", async () => {
    render(<RelayDirectoryPage sourceAppId="codex" onBack={() => {}} />);

    await waitFor(() => expect(listDirectory).toHaveBeenCalledWith("openai"));
    for (const tab of ["overall", "claude", "openai", "gemini"]) {
      expect(
        screen.getByRole("tab", { name: `loongport.directory.tabs.${tab}` }),
      ).toBeInTheDocument();
    }
  });

  it("does not show the previous leaderboard while a new tab is loading", async () => {
    const user = userEvent.setup();
    let resolveOpenAi: ((value: RelayLeaderboard) => void) | undefined;
    listDirectory.mockImplementation((kind: LeaderboardKind) => {
      if (kind === "claude") {
        return Promise.resolve(leaderboard("claude", { items: [item(1)] }));
      }
      return new Promise<RelayLeaderboard>((resolve) => {
        resolveOpenAi = resolve;
      });
    });

    render(<RelayDirectoryPage sourceAppId="claude" onBack={() => {}} />);
    expect(await screen.findByText("BestAPI")).toBeInTheDocument();

    await user.click(
      screen.getByRole("tab", {
        name: "loongport.directory.tabs.openai",
      }),
    );

    await waitFor(() => expect(listDirectory).toHaveBeenCalledWith("openai"));
    await waitFor(() =>
      expect(screen.queryByText("BestAPI")).not.toBeInTheDocument(),
    );
    expect(screen.getByText("loongport.directory.loading")).toBeInTheDocument();

    resolveOpenAi?.(leaderboard("openai", { items: [] }));
    expect(
      await screen.findByText("loongport.directory.empty"),
    ).toBeInTheDocument();
  });

  it("shows the useful VeriDrop evidence and opens full history", async () => {
    render(<RelayDirectoryPage sourceAppId="claude" onBack={() => {}} />);

    expect(await screen.findByText("BestAPI")).toBeInTheDocument();
    const row = screen.getByText("BestAPI").closest("article");
    expect(row).not.toBeNull();
    const bestApi = within(row!);
    expect(bestApi.getByText("bestapi.store")).toBeInTheDocument();
    expect(bestApi.getByText("99")).toBeInTheDocument();
    expect(bestApi.getByText("Claude Code / Cursor 编程")).toBeInTheDocument();
    expect(bestApi.getByText("token_usage")).toBeInTheDocument();
    expect(bestApi.getByText(/95%/)).toBeInTheDocument();
    expect(row).toContainHTML("lucide-lock-keyhole");
    expect(row).not.toHaveTextContent("🔐");
    expect(bestApi.getByText("OpenAI 95")).toBeInTheDocument();
    expect(
      bestApi.getByText("loongport.directory.actions.authenticate"),
    ).toBeInTheDocument();
    expect(
      bestApi.getByText("loongport.directory.actions.autoAddHint"),
    ).toBeInTheDocument();

    fireEvent.click(
      bestApi.getByRole("button", {
        name: "loongport.directory.actions.history",
      }),
    );
    expect(openInBrowser).toHaveBeenCalledWith(
      "https://veridrop.org/leaderboard/bestapi.store",
    );
  });

  it("labels a managed detail-page completion without inventing a rank", async () => {
    listDirectory.mockResolvedValue(
      leaderboard("gemini", {
        items: [{ ...item(1), rank: null }],
      }),
    );

    render(<RelayDirectoryPage sourceAppId="gemini" onBack={() => {}} />);

    const row = (await screen.findByText("BestAPI")).closest("article");
    expect(
      within(row!).getByText("loongport.directory.meta.supplementalRank"),
    ).toBeInTheDocument();
    expect(row).not.toHaveTextContent("#0");
  });

  it("searches and paginates twelve rows per page", async () => {
    render(<RelayDirectoryPage sourceAppId="claude" onBack={() => {}} />);
    await screen.findByText("BestAPI");

    expect(screen.queryByText("站点 13")).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", {
        name: "loongport.directory.pagination.next",
      }),
    );
    expect(screen.getByText("站点 13")).toBeInTheDocument();

    fireEvent.change(
      screen.getByPlaceholderText("loongport.directory.searchPlaceholder"),
      { target: { value: "bestapi" } },
    );
    expect(screen.getByText("BestAPI")).toBeInTheDocument();
    expect(screen.queryByText("站点 13")).not.toBeInTheDocument();
  });

  it("marks cached data with its timestamp and lets the user retry live", async () => {
    listDirectory.mockResolvedValue(
      leaderboard("claude", { fromCache: true, items: [item(1)] }),
    );
    render(<RelayDirectoryPage sourceAppId="claude" onBack={() => {}} />);

    expect(
      await screen.findByText(/loongport\.directory\.source\.cached/),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", {
        name: "loongport.directory.actions.refresh",
      }),
    );
    await waitFor(() => expect(listDirectory).toHaveBeenCalledTimes(2));
  });

  it("waits for authentication and backend refresh before returning", async () => {
    const onBack = vi.fn();
    const onAuthenticated = vi.fn();
    render(
      <RelayDirectoryPage
        sourceAppId="claude"
        onBack={onBack}
        onAuthenticated={onAuthenticated}
      />,
    );
    await screen.findByText("BestAPI");

    const row = screen.getByText("BestAPI").closest("article");
    fireEvent.click(
      within(row!).getByText("loongport.directory.actions.authenticate"),
    );

    await waitFor(() =>
      expect(importDirectorySite).toHaveBeenCalledWith("https://bestapi.store"),
    );
    expect(refresh).toHaveBeenCalledWith(7, "claude");
    await waitFor(() => expect(onAuthenticated).toHaveBeenCalled());
    expect(onBack).toHaveBeenCalled();
  });

  it("stays open when registration or login is cancelled", async () => {
    importDirectorySite.mockRejectedValue({
      kind: "cancelled",
      message: "注册或登录尚未完成",
    });
    const onBack = vi.fn();
    render(<RelayDirectoryPage sourceAppId="claude" onBack={onBack} />);
    await screen.findByText("BestAPI");

    const row = screen.getByText("BestAPI").closest("article");
    fireEvent.click(
      within(row!).getByText("loongport.directory.actions.authenticate"),
    );

    await waitFor(() => expect(importDirectorySite).toHaveBeenCalled());
    expect(refresh).not.toHaveBeenCalled();
    expect(onBack).not.toHaveBeenCalled();
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(toastError).not.toHaveBeenCalled();
  });

  it("returns to the relay list when refresh fails after authentication", async () => {
    refresh.mockRejectedValue(new Error("网络不通"));
    const onBack = vi.fn();
    const onAuthenticated = vi.fn();
    render(
      <RelayDirectoryPage
        sourceAppId="claude"
        onBack={onBack}
        onAuthenticated={onAuthenticated}
      />,
    );
    await screen.findByText("BestAPI");

    const row = screen.getByText("BestAPI").closest("article");
    fireEvent.click(
      within(row!).getByText("loongport.directory.actions.authenticate"),
    );

    await waitFor(() => expect(onAuthenticated).toHaveBeenCalled());
    expect(onBack).toHaveBeenCalled();
    expect(toastError).toHaveBeenCalledWith(
      expect.stringContaining("网络不通"),
    );
  });

  it.each([
    ["unsupported_site", "loongport.addSite.unsupportedSite"],
    ["protocol_conflict", "loongport.addSite.protocolConflict"],
  ])("maps %s to an actionable message", async (kind, key) => {
    importDirectorySite.mockRejectedValue({ kind, message: kind });
    render(<RelayDirectoryPage sourceAppId="claude" onBack={() => {}} />);
    await screen.findByText("BestAPI");

    const row = screen.getByText("BestAPI").closest("article");
    fireEvent.click(
      within(row!).getByText("loongport.directory.actions.authenticate"),
    );

    await waitFor(() => expect(toastError).toHaveBeenCalledWith(key));
  });

  it("uses the localized fallback for an unknown object error", async () => {
    importDirectorySite.mockRejectedValue({ code: "unexpected" });
    render(<RelayDirectoryPage sourceAppId="claude" onBack={() => {}} />);
    await screen.findByText("BestAPI");

    const row = screen.getByText("BestAPI").closest("article");
    fireEvent.click(
      within(row!).getByText("loongport.directory.actions.authenticate"),
    );

    await waitFor(() =>
      expect(toastError).toHaveBeenCalledWith("loongport.addSite.importFailed"),
    );
  });

  it("runs a custom site through the same authentication flow", async () => {
    render(<RelayDirectoryPage sourceAppId="codex" onBack={() => {}} />);
    await screen.findByText("BestAPI");

    fireEvent.change(
      screen.getByPlaceholderText("loongport.directory.customSitePlaceholder"),
      { target: { value: "https://790053500.com/keys" } },
    );
    fireEvent.click(
      screen.getByText("loongport.directory.actions.useOtherSite"),
    );

    await waitFor(() =>
      expect(importSite).toHaveBeenCalledWith("https://790053500.com/keys"),
    );
    expect(refresh).toHaveBeenCalledWith(7, "codex");
  });

  it("allows only one authentication operation at a time", async () => {
    let finishImport!: (value: {
      relayId: number;
      siteOrigin: string;
      siteName: string;
      backendKind: string;
    }) => void;
    importDirectorySite.mockReturnValue(
      new Promise((resolve) => {
        finishImport = resolve;
      }),
    );

    render(<RelayDirectoryPage sourceAppId="claude" onBack={() => {}} />);
    await screen.findByText("BestAPI");

    const customInput = screen.getByPlaceholderText(
      "loongport.directory.customSitePlaceholder",
    );
    fireEvent.change(customInput, {
      target: { value: "https://790053500.com/keys" },
    });

    const firstRow = screen.getByText("BestAPI").closest("article");
    const secondRow = screen.getByText("站点 2").closest("article");
    fireEvent.click(
      within(firstRow!).getByText("loongport.directory.actions.authenticate"),
    );
    fireEvent.click(
      within(secondRow!).getByText("loongport.directory.actions.authenticate"),
    );
    fireEvent.keyDown(customInput, { key: "Enter" });

    expect(importDirectorySite).toHaveBeenCalledTimes(1);
    expect(importSite).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(
        within(secondRow!).getByRole("button", {
          name: "loongport.directory.actions.authenticate",
        }),
      ).toBeDisabled(),
    );
    expect(customInput).toBeDisabled();

    await act(async () => {
      finishImport({
        relayId: 7,
        siteOrigin: "https://bestapi.store",
        siteName: "BestAPI",
        backendKind: "sub2api",
      });
    });
  });
});
