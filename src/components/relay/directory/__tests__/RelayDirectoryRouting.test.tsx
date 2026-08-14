import { Suspense } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import {
  LAST_APP_STORAGE_KEY,
  LAST_VIEW_STORAGE_KEY,
} from "@/config/constants";

vi.mock("@/components/providers/ProviderList", () => ({
  ProviderList: () => <div data-testid="provider-list" />,
}));

vi.mock("@/components/UpdateBadge", () => ({
  UpdateBadge: () => null,
}));

vi.mock("@/components/settings/CcSwitchImportEntry", () => ({
  CcSwitchImportEntry: () => null,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    close: vi.fn(),
    isMaximized: vi.fn().mockResolvedValue(false),
    minimize: vi.fn(),
    onResized: vi.fn().mockResolvedValue(vi.fn()),
    setDecorations: vi.fn().mockResolvedValue(undefined),
    toggleMaximize: vi.fn(),
  }),
}));

vi.mock("@/components/relay/RelaySection", () => ({
  RelaySection: ({ appId, onOpenDirectory }: any) => (
    <div data-testid="relay-section">
      <span data-testid="relay-source-app">{appId}</span>
      <button onClick={() => onOpenDirectory("add")}>open-directory</button>
      <button onClick={() => onOpenDirectory("firstRun")}>
        open-first-run-directory
      </button>
    </div>
  ),
}));

vi.mock("../RelayDirectoryPage", () => ({
  RelayDirectoryPage: ({
    sourceAppId,
    initialKind,
    onBack,
    onAuthenticated,
  }: any) => {
    const sourceDefault =
      sourceAppId === "claude" || sourceAppId === "claude-desktop"
        ? "claude"
        : sourceAppId === "codex" || sourceAppId === "codex-image"
          ? "openai"
          : sourceAppId === "gemini"
            ? "gemini"
            : "overall";
    return (
      <div data-testid="relay-directory">
        <span data-testid="directory-source-app">{sourceAppId}</span>
        <span data-testid="directory-kind">{initialKind ?? sourceDefault}</span>
        <button onClick={onBack}>directory-back</button>
        <button onClick={onAuthenticated}>directory-authenticated</button>
      </div>
    );
  },
}));

function renderApp() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <Suspense fallback={<div>loading</div>}>
        <App />
      </Suspense>
    </QueryClientProvider>,
  );
}

describe("relay directory routing", () => {
  beforeEach(() => {
    localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
    localStorage.setItem(LAST_APP_STORAGE_KEY, "claude");
  });

  it.each([
    ["claude", "claude"],
    ["codex", "openai"],
    ["gemini", "gemini"],
    ["openclaw", "overall"],
  ])(
    "opens an independent directory from %s with the %s leaderboard",
    async (appId, expectedKind) => {
      localStorage.setItem(LAST_APP_STORAGE_KEY, appId);
      renderApp();

      fireEvent.click(await screen.findByText("open-directory"));

      expect(await screen.findByTestId("relay-directory")).toBeInTheDocument();
      expect(screen.getByTestId("directory-source-app")).toHaveTextContent(
        appId,
      );
      expect(screen.getByTestId("directory-kind")).toHaveTextContent(
        expectedKind,
      );
      expect(screen.queryByTestId("app-switcher")).not.toBeInTheDocument();
      expect(document.querySelector("header")).toHaveAttribute("hidden");
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");

      fireEvent.click(screen.getByText("directory-back"));
      expect(await screen.findByTestId("provider-list")).toBeInTheDocument();
    },
  );

  it("opens the first-run entry on the overall leaderboard", async () => {
    localStorage.setItem(LAST_APP_STORAGE_KEY, "codex");
    renderApp();
    fireEvent.click(await screen.findByText("open-first-run-directory"));

    expect(await screen.findByTestId("directory-source-app")).toHaveTextContent(
      "codex",
    );
    expect(screen.getByTestId("directory-kind")).toHaveTextContent("overall");
  });
});
