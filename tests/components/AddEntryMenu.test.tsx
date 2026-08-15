import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AddEntryMenu } from "@/components/AddEntryMenu";
import { createTestQueryClient } from "../utils/testQueryClient";

const listVendorAccounts = vi.fn();

vi.mock("@/lib/api/vendor", () => ({
  vendorApi: { list: (...args: unknown[]) => listVendorAccounts(...args) },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

function renderMenu(appId: "codex" | "gemini") {
  const onOpenRelayDirectory = vi.fn();
  const onOpenOfficialApi = vi.fn();
  const onOpenManual = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <AddEntryMenu
        appId={appId}
        onOpenRelayDirectory={onOpenRelayDirectory}
        onOpenOfficialApi={onOpenOfficialApi}
        onOpenManual={onOpenManual}
      />
    </QueryClientProvider>,
  );
  return { onOpenRelayDirectory, onOpenOfficialApi, onOpenManual };
}

describe("AddEntryMenu", () => {
  beforeEach(() => {
    listVendorAccounts.mockReset();
  });

  it("offers all three entries on a vendor-supported app and routes each click", async () => {
    const user = userEvent.setup();
    listVendorAccounts.mockResolvedValue({ supported: true, accounts: [] });
    const callbacks = renderMenu("codex");

    await user.click(
      screen.getByRole("button", { name: "loongport.addEntry.title" }),
    );
    await user.click(
      screen.getByRole("menuitem", { name: "loongport.tierList.addSite" }),
    );
    expect(callbacks.onOpenRelayDirectory).toHaveBeenCalledTimes(1);

    await user.click(
      screen.getByRole("button", { name: "loongport.addEntry.title" }),
    );
    await user.click(
      screen.getByRole("menuitem", { name: "loongport.sections.official" }),
    );
    expect(callbacks.onOpenOfficialApi).toHaveBeenCalledTimes(1);

    await user.click(
      screen.getByRole("button", { name: "loongport.addEntry.title" }),
    );
    await user.click(
      screen.getByRole("menuitem", { name: "provider.addNewProvider" }),
    );
    expect(callbacks.onOpenManual).toHaveBeenCalledTimes(1);
  });

  it("hides the official API entry when the backend says the app is unsupported", async () => {
    const user = userEvent.setup();
    listVendorAccounts.mockResolvedValue({ supported: false, accounts: [] });
    renderMenu("gemini");

    await user.click(
      screen.getByRole("button", { name: "loongport.addEntry.title" }),
    );

    expect(
      screen.queryByRole("menuitem", { name: "loongport.sections.official" }),
    ).not.toBeInTheDocument();
    // 其余两项仍在 —— 手动表单对所有 app 都有意义。
    expect(
      screen.getByRole("menuitem", { name: "loongport.tierList.addSite" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("menuitem", { name: "provider.addNewProvider" }),
    ).toBeInTheDocument();
  });
});
