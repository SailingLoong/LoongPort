import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RelayDirectoryPage } from "@/components/relay/directory/RelayDirectoryPage";
import { createTestQueryClient } from "../utils/testQueryClient";
const api = vi.hoisted(() => ({
  listDirectory: vi.fn(),
  importSite: vi.fn(),
  refresh: vi.fn(),
  plazaSeedFromFirstSite: vi.fn(),
  plazaSetVisible: vi.fn(),
  dismiss: vi.fn(),
}));
const settings = vi.hoisted(() => ({
  plazaVisible: false,
  crowdMetricsEnabled: false,
}));
vi.mock("@/lib/api", () => ({
  PLAZA_VISIBLE_DEFAULT: false,
  relayApi: api,
  settingsApi: api,
}));
vi.mock("@/lib/api/serviceOnboarding", () => ({ serviceOnboardingApi: api }));
vi.mock("@/hooks/useSettings", () => ({ useSettings: () => ({ settings }) }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { resolvedLanguage: "en" },
  }),
}));
function setup(firstVisit = false) {
  const onBack = vi.fn();
  const onConnected = vi.fn();
  const onOfficial = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <RelayDirectoryPage
        sourceAppId="codex"
        onBack={onBack}
        onConnected={onConnected}
        firstVisit={firstVisit}
        onOfficial={onOfficial}
      />
    </QueryClientProvider>,
  );
  return { onBack, onConnected, onOfficial };
}
beforeEach(() => {
  vi.clearAllMocks();
  settings.plazaVisible = false;
  api.listDirectory.mockResolvedValue({ items: [], syncedAt: 0 });
  api.plazaSeedFromFirstSite.mockResolvedValue(false);
  api.importSite.mockResolvedValue({ relayId: 7, siteName: "Example" });
  api.refresh.mockResolvedValue({});
  api.dismiss.mockResolvedValue({});
  api.plazaSetVisible.mockResolvedValue(true);
});
describe("RelayDirectoryPage", () => {
  it("retains a separate custom domain form while protected listing is hidden", async () => {
    const user = userEvent.setup();
    const { onConnected } = setup();
    expect(
      screen.queryByRole("textbox", {
        name: "loongport.onboarding.searchDirectory",
      }),
    ).not.toBeInTheDocument();
    expect(api.listDirectory).not.toHaveBeenCalled();
    await user.type(
      screen.getByRole("textbox", { name: "loongport.onboarding.domain" }),
      "panel.example",
    );
    await user.click(
      screen.getByRole("button", { name: "loongport.firstSite.confirm" }),
    );
    await waitFor(() =>
      expect(onConnected).toHaveBeenCalledWith({
        kind: "relay",
        rowId: 7,
        name: "Example",
      }),
    );
    expect(api.plazaSeedFromFirstSite).toHaveBeenCalledWith("panel.example");
    expect(api.importSite).toHaveBeenCalledWith("panel.example");
    expect(api.plazaSetVisible).not.toHaveBeenCalled();
  });
  it("does not reinterpret an unmatched directory search as a connection", async () => {
    settings.plazaVisible = true;
    const user = userEvent.setup();
    setup();
    await user.type(
      screen.getByRole("textbox", { name: "loongport.onboarding.domain" }),
      "panel.example",
    );
    await user.type(
      screen.getByRole("textbox", {
        name: "loongport.onboarding.searchDirectory",
      }),
      "unlisted.example{Enter}",
    );
    expect(api.importSite).not.toHaveBeenCalled();
    expect(
      screen.getByRole("textbox", { name: "loongport.onboarding.domain" }),
    ).toHaveValue("panel.example");
  });
  it("dismisses welcome with Escape without opting into the directory", async () => {
    const user = userEvent.setup();
    const { onBack } = setup(true);
    await user.keyboard("{Escape}");
    expect(api.plazaSetVisible).not.toHaveBeenCalled();
    expect(api.importSite).not.toHaveBeenCalled();
    expect(onBack).toHaveBeenCalled();
  });
  it("offers an official service route without opting into the directory", async () => {
    const user = userEvent.setup();
    const { onOfficial } = setup(true);
    await user.click(
      screen.getByRole("button", { name: "loongport.sections.official" }),
    );
    expect(onOfficial).toHaveBeenCalled();
    expect(api.plazaSetVisible).not.toHaveBeenCalled();
  });
});
