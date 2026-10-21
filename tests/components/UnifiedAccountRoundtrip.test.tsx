import { useEffect } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { EditProviderDialog } from "@/components/providers/EditProviderDialog";
import { PreservedView } from "@/components/ui/PreservedView";
import { useClientNavigation } from "@/components/shell/navigation";
import { OPEN_ACCOUNT_MANAGER } from "@/components/relay/accounts/accountPlatforms";
import type { Provider } from "@/types";
const api = vi.hoisted(() => ({ read: vi.fn() }));
vi.mock("@/lib/api/providers", async (load) => {
  const original = await load<typeof import("@/lib/api/providers")>();
  return {
    ...original,
    providersApi: { ...original.providersApi, getEditSettings: api.read },
  };
});
vi.mock("@/components/JsonEditor", () => ({
  default: ({ value, onChange }: any) => (
    <textarea
      aria-label="configuration"
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));
const provider: Provider = {
  id: "synthetic-codex",
  name: "Synthetic provider",
  category: "custom",
  settingsConfig: {
    auth: { OPENAI_API_KEY: "synthetic-key" },
    config:
      'model_provider = "custom"\n[model_providers.custom]\nbase_url = "https://api.example/v1"\nwire_api = "responses"\n',
  },
};
function Shell() {
  const navigation = useClientNavigation(
    () => "providers",
    () => "codex",
  );
  useEffect(() => {
    const open = () =>
      navigation.navigate("services", "codex", undefined, "codex_oauth");
    window.addEventListener(OPEN_ACCOUNT_MANAGER, open);
    return () => window.removeEventListener(OPEN_ACCOUNT_MANAGER, open);
  }, [navigation.navigate]);
  return (
    <>
      {navigation.view === "services" && (
        <button onClick={navigation.back}>Return to editor</button>
      )}
      <PreservedView active={navigation.view !== "services"}>
        <EditProviderDialog
          open
          provider={provider}
          appId={navigation.app}
          onOpenChange={() => {}}
          onSubmit={() => {}}
        />
      </PreservedView>
    </>
  );
}
it("preserves the real provider form and releases its real portal during account management", async () => {
  api.read.mockResolvedValue({
    settingsConfig: provider.settingsConfig,
    modeState: null,
  });
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <Shell />
    </QueryClientProvider>,
  );
  const name = await screen.findByDisplayValue("Synthetic provider");
  fireEvent.change(name, { target: { value: "Unsaved synthetic name" } });
  fireEvent(
    window,
    new CustomEvent(OPEN_ACCOUNT_MANAGER, { detail: "codex_oauth" }),
  );
  await screen.findByRole("button", { name: "Return to editor" });
  await waitFor(() => expect(document.body.style.overflow).not.toBe("hidden"));
  expect(
    screen.queryByDisplayValue("Unsaved synthetic name"),
  ).not.toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Return to editor" }));
  await waitFor(() =>
    expect(screen.getByDisplayValue("Unsaved synthetic name")).toBeVisible(),
  );
  expect(api.read.mock.calls.every(([, app]) => app === "codex")).toBe(true);
});
