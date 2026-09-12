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

vi.mock("../RelayDirectoryPage", () => ({
  RelayDirectoryPage: ({ sourceAppId, onBack, onAuthenticated }: any) => (
    <div data-testid="relay-directory">
      <span data-testid="directory-source-app">{sourceAppId}</span>
      <button onClick={onBack}>directory-back</button>
      <button onClick={onAuthenticated}>directory-authenticated</button>
    </div>
  ),
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

  it.each([["claude"], ["codex"], ["gemini"], ["openclaw"]])(
    "opens the add hub from %s on the relay directory",
    async (appId) => {
      localStorage.setItem(LAST_APP_STORAGE_KEY, appId);
      renderApp();

      // 顶栏大「+」进聚合页，默认落中转站标签
      //（i18n 空资源下 aria-label 就是 key 本身）。
      fireEvent.click(
        await screen.findByRole("button", {
          name: "loongport.addEntry.title",
        }),
      );

      expect(await screen.findByTestId("relay-directory")).toBeInTheDocument();
      expect(screen.getByTestId("directory-source-app")).toHaveTextContent(
        appId,
      );
      expect(screen.queryByTestId("app-switcher")).not.toBeInTheDocument();
      expect(document.querySelector("header")).toHaveAttribute("hidden");
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");

      fireEvent.click(screen.getByText("directory-back"));
      expect(
        await screen.findByRole("button", {
          name: "applications.addService",
        }),
      ).toBeInTheDocument();
    },
  );

  it("opens the overview add-service entry on the relay directory", async () => {
    localStorage.setItem(LAST_APP_STORAGE_KEY, "codex");
    renderApp();
    fireEvent.click(
      await screen.findByRole("button", { name: "applications.addService" }),
    );

    expect(await screen.findByTestId("directory-source-app")).toHaveTextContent(
      "codex",
    );
  });
});
