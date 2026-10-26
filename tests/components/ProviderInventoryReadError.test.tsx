import { Suspense } from "react";
import {
  render,
  screen,
  waitFor,
  act,
  fireEvent,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import App from "@/App";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
import { providersApi } from "@/lib/api";
import {
  LAST_APP_STORAGE_KEY,
  LAST_VIEW_STORAGE_KEY,
} from "@/config/constants";
vi.mock("@/components/UpdateBadge", () => ({ UpdateBadge: () => null }));
vi.mock("@/components/settings/ThemeSettings", () => ({
  ThemeSettings: () => null,
}));
afterEach(() => vi.restoreAllMocks());
function show() {
  vi.spyOn(applicationRoutingApi, "get").mockResolvedValue({
    autoFailoverEnabled: false,
    routingActive: false,
    model: null,
    modelOptions: [],
    chainIds: [],
    tiers: [],
  });
  localStorage.setItem(LAST_APP_STORAGE_KEY, "opencode");
  localStorage.setItem(LAST_VIEW_STORAGE_KEY, "providers");
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <Suspense fallback={null}>
        <App />
      </Suspense>
    </QueryClientProvider>,
  );
  return client;
}
it("shows a failed account read in the real inventory, not a successful empty state", async () => {
  vi.spyOn(providersApi, "getAll").mockRejectedValue(
    new Error("secret.locked"),
  );
  const client = show();
  await waitFor(() =>
    expect(client.getQueryState(["providers", "opencode"])?.status).toBe(
      "error",
    ),
  );
  fireEvent.click(
    await screen.findByRole("button", {
      name: "applications.manageConfigurations",
    }),
  );
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent("provider.readFailed"),
  );
  expect(screen.queryByText("provider.noProviders")).not.toBeInTheDocument();
});
it("retains the real empty inventory when the read succeeds", async () => {
  vi.spyOn(providersApi, "getAll").mockResolvedValue({});
  show();
  fireEvent.click(
    await screen.findByRole("button", {
      name: "applications.manageConfigurations",
    }),
  );
  expect(await screen.findByText("provider.noProviders")).toBeInTheDocument();
  expect(screen.queryByText("provider.readFailed")).not.toBeInTheDocument();
});
it("keeps cached account rows visible alongside a refetch error", async () => {
  vi.spyOn(providersApi, "getAll")
    .mockResolvedValueOnce({
      saved: {
        id: "saved",
        name: "Retained saved account",
        settingsConfig: {},
      },
    })
    .mockRejectedValue(new Error("secret.locked"));
  const client = show();
  fireEvent.click(
    await screen.findByRole("button", {
      name: "applications.manageConfigurations",
    }),
  );
  expect(await screen.findByText("Retained saved account")).toBeInTheDocument();
  await act(async () => {
    await client.refetchQueries({ queryKey: ["providers", "opencode"] });
  });
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent("provider.readFailed"),
  );
  expect(screen.getByText("Retained saved account")).toBeInTheDocument();
  expect(screen.queryByText("provider.noProviders")).not.toBeInTheDocument();
});
