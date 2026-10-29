import { Suspense } from "react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { relayApi } from "@/lib/api/relay";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";
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
  RelayDirectoryPage: ({
    sourceAppId,
    onBack,
    onAuthenticated,
    onConnected,
  }: any) => (
    <div data-testid="relay-directory">
      <span data-testid="directory-source-app">{sourceAppId}</span>
      <button onClick={onBack}>directory-back</button>
      <button onClick={onAuthenticated}>directory-authenticated</button>
      <button
        onClick={() =>
          onConnected({ kind: "relay", rowId: 7, name: "Example" })
        }
      >
        connect-account
      </button>
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
  afterEach(() => vi.restoreAllMocks());
  beforeEach(() => {
    localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
    localStorage.setItem(LAST_APP_STORAGE_KEY, "claude");
  });

  it("keeps the primary plaza page free of Back controls", async () => {
    renderApp();
    fireEvent.click(
      await screen.findByRole("button", { name: "client.plaza" }),
    );
    expect(await screen.findByTestId("relay-directory")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "common.back" }),
    ).not.toBeInTheDocument();
  });

  it("returns from configuration to the add flow on Escape without leaving onboarding", async () => {
    // This is the real selected-setup page. Only its read-only native boundary
    // is supplied here; Escape must still be handled before the outer shell.
    const discovery = vi.spyOn(relayApi, "discoverTiers").mockResolvedValue({
      schemaVersion: 1,
      relayId: 7,
      account: {
        identity: "fixture/account7",
        siteOrigin: "https://fixture.example",
        accountLabel: "Example",
      },
      revision: "fixture-revision",
      choices: [],
    });
    const recovery = vi
      .spyOn(relayApi, "getSetupOperation")
      .mockResolvedValue(null);
    const prepare = vi.spyOn(relayApi, "prepareTiers");
    const refresh = vi.spyOn(relayApi, "refresh");
    const dismiss = vi.spyOn(serviceOnboardingApi, "dismiss");
    renderApp();
    fireEvent.click(
      await screen.findByRole("button", { name: "loongport.addEntry.title" }),
    );
    await userEvent.click(
      await screen.findByRole("tab", { name: "loongport.sections.relay" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "connect-account" }),
    );
    expect(
      await screen.findByRole("heading", {
        name: "准备接入配置",
      }),
    ).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "common.back" })).toHaveLength(
      1,
    );
    await waitFor(() => expect(discovery).toHaveBeenCalledWith(7));
    expect(recovery).toHaveBeenCalledWith(7);
    expect(prepare).not.toHaveBeenCalled();
    expect(refresh).not.toHaveBeenCalled();
    expect(dismiss).not.toHaveBeenCalled();
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(await screen.findByTestId("relay-directory")).toBeVisible();
    expect(document.querySelector("header")).toHaveAttribute("hidden");
    expect(screen.getAllByRole("button", { name: "common.back" })).toHaveLength(
      1,
    );
    expect(dismiss).not.toHaveBeenCalled();
    expect(prepare).not.toHaveBeenCalled();
    expect(refresh).not.toHaveBeenCalled();
  });

  it.each([
    ["claude", "loongport.addEntry.title"],
    ["codex", "loongport.addEntry.title"],
    ["gemini", "loongport.addEntry.title"],
    ["openclaw", "applications.addService"],
  ])(
    "opens the add hub from %s and reaches the relay directory",
    async (appId, returnEntryName) => {
      localStorage.setItem(LAST_APP_STORAGE_KEY, appId);
      renderApp();

      // Unified add opens accounts; the original relay flow remains one tab away.
      fireEvent.click(
        await screen.findByRole("button", {
          name: "loongport.addEntry.title",
        }),
      );

      expect(await screen.findByRole("tab", { name: "账号" })).toHaveAttribute(
        "aria-selected",
        "true",
      );
      await userEvent.click(
        await screen.findByRole("tab", { name: "loongport.sections.relay" }),
      );
      expect(await screen.findByTestId("relay-directory")).toBeVisible();
      expect(screen.getByTestId("directory-source-app")).toHaveTextContent(
        appId,
      );
      expect(screen.queryByTestId("app-switcher")).not.toBeInTheDocument();
      expect(document.querySelector("header")).toHaveAttribute("hidden");
      expect(
        screen.getAllByRole("button", { name: "common.back" }),
      ).toHaveLength(1);
      expect(localStorage.getItem(LAST_VIEW_STORAGE_KEY)).toBe("providers");

      fireEvent.click(screen.getByText("directory-back"));
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: returnEntryName }),
        ).toBeVisible(),
      );
      expect(document.querySelector("header")).toBeVisible();
      expect(screen.getByTestId("relay-directory")).not.toBeVisible();
      expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe(appId);
    },
  );

  it.each([
    ["codex", "loongport.addEntry.title"],
    ["openclaw", "applications.addService"],
  ])(
    "reaches the relay directory from the %s add-service entry",
    async (appId, entryName) => {
      localStorage.setItem(LAST_APP_STORAGE_KEY, appId);
      renderApp();
      fireEvent.click(await screen.findByRole("button", { name: entryName }));

      expect(await screen.findByRole("tab", { name: "账号" })).toHaveAttribute(
        "aria-selected",
        "true",
      );
      await userEvent.click(
        await screen.findByRole("tab", { name: "loongport.sections.relay" }),
      );
      expect(
        await screen.findByTestId("directory-source-app"),
      ).toHaveTextContent(appId);
      fireEvent.click(screen.getByRole("button", { name: "common.back" }));
      await waitFor(() =>
        expect(screen.getByRole("button", { name: entryName })).toBeVisible(),
      );
      expect(screen.getByTestId("relay-directory")).not.toBeVisible();
      expect(localStorage.getItem(LAST_APP_STORAGE_KEY)).toBe(appId);
    },
  );
});
