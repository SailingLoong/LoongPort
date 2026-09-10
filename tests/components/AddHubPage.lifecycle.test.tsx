import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { AddHubPage } from "@/components/relay/AddHubPage";
import { PreservedView } from "@/components/ui/PreservedView";
import { createTestQueryClient } from "../utils/testQueryClient";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: "en" } }),
}));
vi.mock("@/lib/query", async (original) => ({
  ...(await original<typeof import("@/lib/query")>()),
  useSettingsQuery: () => ({ data: { commonConfigConfirmed: false } }),
}));
vi.mock("@/components/relay/directory/RelayDirectoryPage", () => ({
  RelayDirectoryPage: () => <div>Directory</div>,
}));
vi.mock("@/components/relay/OfficialApiPage", () => ({
  OfficialApiPage: () => <div>Official</div>,
}));
vi.mock("@/components/JsonEditor", () => ({ default: () => <textarea /> }));
describe("AddHubPage lifecycle", () => {
  it("does not show manual-form portals before that tab is selected and suspends them on another page", async () => {
    const user = userEvent.setup();
    const client = createTestQueryClient();
    const view = (active: boolean) => (
      <QueryClientProvider client={client}>
        <PreservedView active={active}>
          <AddHubPage
            sourceAppId="codex"
            onBack={vi.fn()}
            onAddProvider={vi.fn()}
          />
        </PreservedView>
        <button>Another page</button>
      </QueryClientProvider>
    );
    const { rerender } = render(view(true));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("tab", { name: "loongport.addEntry.manual" }),
    );
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    rerender(view(false));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(document.body.style.pointerEvents).not.toBe("none");
  });
  it("changes tabs only for a new explicit destination request", async () => {
    const user = userEvent.setup();
    const client = createTestQueryClient();
    const view = (active: boolean, id: number) => (
      <QueryClientProvider client={client}>
        <PreservedView active={active}>
          <AddHubPage
            sourceAppId="codex"
            onBack={vi.fn()}
            onAddProvider={vi.fn()}
            entryRequest={{ id, tab: "directory" }}
          />
        </PreservedView>
      </QueryClientProvider>
    );
    const { rerender } = render(view(true, 1));
    await user.click(
      screen.getByRole("tab", { name: "loongport.sections.official" }),
    );
    rerender(view(false, 1));
    rerender(view(true, 1));
    expect(
      screen.getByRole("tab", { name: "loongport.sections.official" }),
    ).toHaveAttribute("aria-selected", "true");
    rerender(view(true, 2));
    expect(
      screen.getByRole("tab", { name: "loongport.sections.relay" }),
    ).toHaveAttribute("aria-selected", "true");
  });
});
