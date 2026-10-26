import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ServiceSource } from "@/lib/api/serviceSources";
import { PROVIDER_SWITCHED } from "@/lib/api/events";
import { ServiceSourceList } from "../ServiceSourceList";

const api = vi.hoisted(() => ({
  list: vi.fn(),
  listeners: new Map<string, () => unknown>(),
}));
vi.mock("@/lib/api/serviceSources", () => ({ serviceSourcesApi: api }));
vi.mock("@/hooks/useTauriEvent", () => ({
  useTauriEvent: (event: string, callback: () => unknown) =>
    api.listeners.set(event, callback),
}));
vi.mock("@/config/appConfig", () => ({
  getAppDisplayName: (app: string) => app,
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

const source: ServiceSource & { account: { kind: "relay"; id: number } } = {
  sourceId: "shared-plan",
  account: { kind: "relay", id: 7 },
  name: "Standard",
  serviceName: "Example service",
  accountLabel: "Work",
  configurationName: "Standard",
  bindings: [
    {
      appId: "codex",
      providerId: "shared-plan",
      model: "example-model",
      usageState: "current",
      canSelect: true,
    },
    {
      appId: "claude",
      providerId: "shared-plan",
      model: null,
      usageState: "inactive",
      canSelect: false,
    },
  ],
};
let client: QueryClient;
beforeEach(() => {
  vi.clearAllMocks();
  api.listeners.clear();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
});
afterEach(() => client.clear());
function renderList(
  account: { kind: "relay" | "vendor"; id: number } = source.account,
) {
  const onOpenApp = vi.fn();
  const content = (account: { kind: "relay" | "vendor"; id: number }) => (
    <QueryClientProvider client={client}>
      <ServiceSourceList account={account} onOpenApp={onOpenApp} />
    </QueryClientProvider>
  );
  const result = render(content(account));
  return {
    ...result,
    onOpenApp,
    changeAccount: (account: { kind: "relay" | "vendor"; id: number }) =>
      result.rerender(content(account)),
  };
}

describe("shared account sources", () => {
  it("shows one source with two application references and opens the existing workspace", async () => {
    api.list.mockResolvedValue([source]);
    const { onOpenApp } = renderList();
    const article = await screen.findByRole("article");
    expect(screen.getAllByRole("article")).toHaveLength(1);
    expect(within(article).getByText("Standard")).toBeInTheDocument();
    expect(
      within(article).getAllByText("applications.configured"),
    ).toHaveLength(1);
    fireEvent.click(within(article).getByRole("button", { name: /claude/ }));
    expect(onOpenApp).toHaveBeenCalledWith("claude");
    expect(api.list).toHaveBeenCalledTimes(1);
    // Navigation remains available even when the backend disallows selection.
    // This component never submits a switch or copies source data into an App.
  });

  it("shows backend enabled state for an additive application without inferring current selection", async () => {
    api.list.mockResolvedValue([
      {
        ...source,
        bindings: [
          { ...source.bindings[0], appId: "opencode", usageState: "enabled" },
          { ...source.bindings[1], appId: "openclaw", usageState: "inactive" },
        ],
      },
    ]);
    renderList();
    const article = await screen.findByRole("article");
    expect(within(article).getAllByText("applications.enabled")).toHaveLength(
      1,
    );
    expect(
      within(article).queryByText("applications.configured"),
    ).not.toBeInTheDocument();
  });

  it("does not combine accounts or same-named plans", async () => {
    api.list.mockResolvedValue([
      source,
      { ...source, sourceId: "second-plan" },
      {
        ...source,
        sourceId: "other-account",
        account: { kind: "relay", id: 8 },
      },
      {
        ...source,
        sourceId: "other-domain",
        account: { kind: "vendor", id: 7 },
      },
    ]);
    renderList();
    await waitFor(() => expect(screen.getAllByRole("article")).toHaveLength(2));
  });

  it("changes account scope without retaining the previous account's source", async () => {
    api.list.mockResolvedValue([
      source,
      {
        ...source,
        sourceId: "other",
        account: { kind: "relay", id: 8 },
        configurationName: "Personal",
      },
    ]);
    const { changeAccount } = renderList();
    expect(await screen.findByText("Standard")).toBeInTheDocument();
    changeAccount({ kind: "relay", id: 8 });
    expect(await screen.findByText("Personal")).toBeInTheDocument();
    expect(screen.queryByText("Standard")).not.toBeInTheDocument();
  });

  it("reloads after an owner event and hides cached associations if the new read fails", async () => {
    api.list
      .mockResolvedValueOnce([source])
      .mockRejectedValueOnce(new Error("offline"));
    renderList();
    await screen.findByRole("article");
    await act(async () => {
      await api.listeners.get(PROVIDER_SWITCHED)!();
    });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "loongport.sources.loadFailed",
    );
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
    expect(
      screen.queryByText("loongport.sources.empty"),
    ).not.toBeInTheDocument();
    expect(api.list).toHaveBeenCalledTimes(2);
  });

  it("shows an empty state without inventing an application association", async () => {
    api.list.mockResolvedValue([]);
    renderList();
    expect(
      await screen.findByText("loongport.sources.empty"),
    ).toBeInTheDocument();
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
  });

  it("shows a failed read and retries without a successful or empty-state claim", async () => {
    api.list
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce([source]);
    renderList();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "loongport.sources.loadFailed",
    );
    expect(
      screen.queryByText("loongport.sources.empty"),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "common.retry" }));
    expect(await screen.findByRole("article")).toBeInTheDocument();
    expect(api.list).toHaveBeenCalledTimes(2);
  });
});
