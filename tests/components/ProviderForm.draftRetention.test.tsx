import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { ProviderForm } from "@/components/providers/forms/ProviderForm";
import { PreservedView } from "@/components/ui/PreservedView";
import { createTestQueryClient } from "../utils/testQueryClient";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key, i18n: { language: "en" } }),
}));
vi.mock("@/lib/query", async (original) => ({
  ...(await original<typeof import("@/lib/query")>()),
  useSettingsQuery: () => ({ data: { commonConfigConfirmed: true } }),
}));
vi.mock("@/components/JsonEditor", () => ({
  default: ({
    value,
    onChange,
  }: {
    value: string;
    onChange: (v: string) => void;
  }) => (
    <textarea
      aria-label="configuration JSON"
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  ),
}));
describe("ProviderForm draft retention", () => {
  it.each([
    "claude",
    "codex",
    "gemini",
    "pi",
    "claude-desktop",
    "grokbuild",
    "openclaw",
    "opencode",
    "hermes",
  ] as const)(
    "preserves %s manual draft across inactive lifecycle",
    async (appId) => {
      const user = userEvent.setup();
      const client = createTestQueryClient();
      const view = (active: boolean) => (
        <QueryClientProvider client={client}>
          <PreservedView active={active}>
            <ProviderForm
              appId={appId}
              submitLabel="save"
              onSubmit={vi.fn()}
              onCancel={vi.fn()}
            />
          </PreservedView>
        </QueryClientProvider>
      );
      const { rerender } = render(view(true));
      const name = await screen.findByPlaceholderText(
        "provider.namePlaceholder",
      );
      await user.clear(name);
      await user.type(name, "Draft service");
      rerender(view(false));
      rerender(view(true));
      expect(
        await screen.findByPlaceholderText("provider.namePlaceholder"),
      ).toHaveValue("Draft service");
    },
  );
});
