import { act, screen, within, waitFor } from "@testing-library/react";
import { afterAll, beforeAll, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import i18n from "i18next";
import {
  platformAccountFilterName,
  nativeAccountKeyboardSteps,
  nativeAccountOverviewCounts,
} from "../browser/unified-account-locators.mjs";
import { readAccountPlatformPresence } from "@/components/relay/accounts/useAccountPlatformPresence";

const boundary = vi.hoisted(() => ({ invoke: undefined, root: undefined }));
vi.mock("@tauri-apps/api/mocks", () => ({
  mockIPC: (handler) => {
    boundary.invoke = handler;
  },
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (command, payload) => boundary.invoke(command, payload),
}));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(async () => () => {}),
}));
vi.mock("react-dom/client", async (load) => {
  const actual = await load();
  return {
    ...actual,
    createRoot: (container, options) => {
      boundary.root = actual.createRoot(container, options);
      return boundary.root;
    },
  };
});
beforeAll(async () => {
  window.history.replaceState(null, "", "?scenario=native&lang=en");
  document.body.innerHTML = '<div id="root"></div>';
  await act(async () => {
    await import("../browser/unified-accounts");
  });
});
afterAll(async () => {
  await act(async () => boundary.root?.unmount());
  document.getElementById("root")?.remove();
});

for (const language of ["en", "zh"]) {
  it(`uses real inventory, localized browser locators, keyboard filters and Back to All in ${language}`, async () => {
    const user = userEvent.setup();
    await act(async () => {
      await i18n.changeLanguage(language);
    });
    const accounts = boundary.invoke("list_minimax_code_accounts");
    expect(accounts.length).toBeGreaterThan(0);
    expect(accounts.every((account) => account.region === "cn")).toBe(true);
    const inventory = await readAccountPlatformPresence();
    expect(inventory.unavailable).toEqual([]);
    expect(inventory.platforms).toContain("minimax_code");
    expect(inventory.platforms).not.toContain("minimax_code_global");
    const main = screen.getByRole("main");
    const t = (key) => i18n.t(key);
    const tablist = within(main).getByRole("tablist", {
      name: platformAccountFilterName(t),
    });
    const minimax = await within(tablist).findByRole("tab", {
      name: "MiniMax Code 国内版",
    });
    expect(minimax).toHaveTextContent("CN");
    expect(
      within(tablist).queryByRole("tab", { name: "GitHub Copilot" }),
    ).not.toBeInTheDocument();
    expect(
      within(tablist).getByRole("tab", { name: "OpenCode" }),
    ).toBeInTheDocument();
    await user.click(
      within(tablist).getByRole("tab", {
        name: t("loongport.accounts.allPlatforms"),
      }),
    );
    expect(
      within(main).queryAllByRole("region", { name: "账号详情" }),
    ).toHaveLength(0);
    const second = await within(main).findByRole("button", {
      name: "查看 Synthetic account B",
    });
    await user.click(second);
    expect(
      await within(main).findByRole("heading", { name: "Synthetic account B" }),
    ).toBeInTheDocument();
    expect(
      within(main).queryByRole("heading", { name: "Synthetic account A" }),
    ).not.toBeInTheDocument();
    await user.click(
      within(tablist).getByRole("tab", {
        name: t("loongport.accounts.allPlatforms"),
      }),
    );
    expect(
      within(main).queryAllByRole("region", { name: "账号详情" }),
    ).toHaveLength(0);
    await user.click(minimax);
    expect(
      within(main).getByRole("button", { name: "查看 Synthetic account B" }),
    ).toBeVisible();
    expect(
      within(main).queryByRole("heading", { name: "Synthetic account B" }),
    ).not.toBeInTheDocument();
    await user.click(
      within(main).getByRole("button", { name: "查看 Synthetic account A" }),
    );
    expect(
      within(main).getByRole("heading", { name: "Synthetic account A" }),
    ).toBeInTheDocument();
    await user.click(
      within(tablist).getByRole("tab", {
        name: t("loongport.accounts.allPlatforms"),
      }),
    );
    await user.click(
      within(main).getByRole("button", { name: "查看 Synthetic account B" }),
    );
    expect(
      within(main).getByRole("heading", { name: "Synthetic account B" }),
    ).toBeInTheDocument();
    await user.click(
      within(document.querySelector("[data-layout-header]")).getByRole(
        "button",
        { name: t("common.back") },
      ),
    );
    await waitFor(() =>
      expect(
        within(main).queryAllByRole("region", { name: "账号详情" }),
      ).toHaveLength(nativeAccountOverviewCounts.details),
    );
    await user.click(minimax);
    for (const [key, name] of nativeAccountKeyboardSteps(t)) {
      await user.keyboard(`{${key}}`);
      const selected = within(tablist).getByRole("tab", { name });
      expect(selected).toHaveAttribute("aria-selected", "true");
      await waitFor(() => expect(selected).toHaveFocus());
    }
    await user.click(
      within(tablist).getByRole("tab", {
        name: t("loongport.accounts.allPlatforms"),
      }),
    );
    for (const name of [
      "查看 Synthetic account A",
      "查看 Synthetic account B",
    ]) {
      expect(within(main).queryAllByRole("button", { name })).toHaveLength(
        nativeAccountOverviewCounts.accountButtons,
      );
    }
    expect(
      within(tablist).getByRole("tab", {
        name: t("loongport.accounts.allPlatforms"),
      }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      within(main).getByRole("button", { name: "查看 Synthetic account A" }),
    ).toBeInTheDocument();
    expect(
      within(main).getByRole("button", {
        name: "查看 Synthetic WorkBuddy account",
      }),
    ).toBeInTheDocument();
    expect(window.unifiedLayout.rejectedCommands).toEqual([]);
  });
}
