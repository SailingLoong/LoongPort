import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { RelayDirectoryConnectionPage } from "@/components/relay/onboarding/RelayDirectoryConnectionPage";
import { createTestQueryClient } from "../utils/testQueryClient";
const api = vi.hoisted(() => ({
  importSite: vi.fn().mockResolvedValue({ relayId: 7, siteName: "Example" }),
  refresh: vi.fn().mockResolvedValue({}),
  plazaSeedFromFirstSite: vi.fn().mockResolvedValue(false),
  switchTier: vi.fn().mockResolvedValue({ status: "switched", warnings: [] }),
  listRelays: async (app: string) =>
    app === "codex"
      ? [
          {
            id: 7,
            tiers: [
              { providerId: "p1", appId: "codex", displayName: "Standard" },
            ],
          },
        ]
      : [],
}));
vi.mock("@/lib/api", () => ({
  PLAZA_VISIBLE_DEFAULT: false,
  relayApi: api,
  settingsApi: api,
}));
vi.mock("@/lib/api/relay", () => ({ relayApi: api }));
vi.mock("@/lib/api/serviceOnboarding", () => ({
  serviceOnboardingApi: { status: async () => ({ completed: true }) },
}));
vi.mock("@/hooks/useSettings", () => ({
  useSettings: () => ({
    settings: { plazaVisible: false, crowdMetricsEnabled: false },
  }),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { resolvedLanguage: "en" },
  }),
}));
describe("RelayDirectoryConnectionPage", () => {
  it("confirms configuration after standalone directory login and preserves domain when returning", async () => {
    const user = userEvent.setup();
    const onBack = vi.fn();
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <RelayDirectoryConnectionPage sourceAppId="codex" onBack={onBack} />
      </QueryClientProvider>,
    );
    await user.type(
      screen.getByRole("textbox", { name: "loongport.onboarding.domain" }),
      "panel.example",
    );
    await user.click(
      screen.getByRole("button", { name: "loongport.firstSite.confirm" }),
    );
    await screen.findByRole("combobox", { name: "Codex" });
    expect(onBack).not.toHaveBeenCalled();
    expect(api.switchTier).not.toHaveBeenCalled();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "common.back" }));
    expect(
      screen.getByRole("textbox", { name: "loongport.onboarding.domain" }),
    ).toHaveValue("panel.example");
    await user.click(
      screen.getByRole("button", { name: "loongport.onboarding.resume" }),
    );
    await user.click(
      screen.getByRole("button", { name: "loongport.onboarding.finish" }),
    );
    await waitFor(() => expect(onBack).toHaveBeenCalled());
    expect(api.switchTier).toHaveBeenCalledWith("p1", "codex", undefined);
    expect(
      screen.queryByRole("button", { name: "loongport.onboarding.resume" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("textbox", { name: "loongport.onboarding.domain" }),
    ).toHaveValue("");
  });
});
