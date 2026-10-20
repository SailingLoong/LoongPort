import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { PreservedView } from "@/components/ui/PreservedView";
import { AddHubPage } from "@/components/relay/AddHubPage";
import { createTestQueryClient } from "../utils/testQueryClient";
import { vendorApi } from "@/lib/api/vendor";
vi.mock("@/lib/query", async (original) => ({
  ...(await original<typeof import("@/lib/query")>()),
  useSettingsQuery: () => ({ data: { commonConfigConfirmed: true } }),
}));
vi.mock("@/components/relay/directory/RelayDirectoryPage", () => ({
  RelayDirectoryPage: () => null,
}));
vi.mock("@/components/JsonEditor", () => ({ default: () => <textarea /> }));
vi.mock("@/lib/api/vendor", async (original) => ({
  ...(await original<typeof import("@/lib/api/vendor")>()),
  vendorApi: { openLogin: vi.fn(async () => null) },
}));
beforeEach(() => vi.clearAllMocks());
function setup(app: "codex" | "pi" | "claude-desktop" | "grokbuild" = "codex") {
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <AddHubPage sourceAppId={app} onBack={vi.fn()} onAddProvider={vi.fn()} />
    </QueryClientProvider>,
  );
  return userEvent.setup();
}
it("Qoder account entry explains missing native account integration rather than opening custom API", async () => {
  const user = setup();
  await user.click(screen.getByRole("button", { name: "Qoder" }));
  expect(
    await screen.findByText(/不会以自定义 API 或 SDK 配置代替账号接入/),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("textbox", { name: /名称|name/i }),
  ).not.toBeInTheDocument();
});
it("OpenRouter entry loads its real preset in the actual provider form", async () => {
  const user = setup();
  await user.click(
    screen.getByRole("button", { name: "OpenRouter · API Key" }),
  );
  await waitFor(() =>
    expect(screen.getByDisplayValue("OpenRouter")).toBeInTheDocument(),
  );
});
it("OpenCode entry limits the real vendor page and retains the consumer app", async () => {
  const user = setup();
  await user.click(screen.getByRole("button", { name: "OpenCode" }));
  const vendor = await screen.findByRole("button", {
    name: /opencode.*connect/i,
  });
  await user.click(vendor);
  expect(vendorApi.openLogin).toHaveBeenCalledWith("opencode", "codex");
});

it.each(["pi", "claude-desktop", "grokbuild"] as const)(
  "OpenRouter retains the real %s form preset",
  async (app) => {
    const user = setup(app);
    await user.click(
      screen.getByRole("button", { name: "OpenRouter · API Key" }),
    );
    await waitFor(() =>
      expect(screen.getByDisplayValue("OpenRouter")).toBeInTheDocument(),
    );
  },
);
it("ignores a late vendor login after cancelling the source destination", async () => {
  let resolve!: (value: any) => void;
  vi.mocked(vendorApi.openLogin).mockImplementationOnce(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const user = setup();
  await user.click(screen.getByRole("button", { name: "OpenCode" }));
  await user.click(screen.getByRole("button", { name: /opencode.*connect/i }));
  await user.click(screen.getByRole("button", { name: "返回平台列表" }));
  await user.click(screen.getByRole("button", { name: "Qoder" }));
  resolve({ rowId: 42 });
  await waitFor(() =>
    expect(
      screen.getByText(/不会以自定义 API 或 SDK 配置代替账号接入/),
    ).toBeInTheDocument(),
  );
  expect(
    screen.queryByText("loongport.onboarding.resume"),
  ).not.toBeInTheDocument();
});

it("does not let a completed old provider save dismiss a newer source", async () => {
  let resolve!: () => void;
  const save = vi.fn(
    () =>
      new Promise<void>((r) => {
        resolve = r;
      }),
  );
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <AddHubPage sourceAppId="codex" onBack={vi.fn()} onAddProvider={save} />
    </QueryClientProvider>,
  );
  const user = userEvent.setup();
  await user.click(
    screen.getByRole("button", { name: "OpenRouter · API Key" }),
  );
  await screen.findByDisplayValue("OpenRouter");
  await user.type(
    screen.getByLabelText("API Key", { selector: "input" }),
    "synthetic-test-key",
  );
  fireEvent.submit(document.querySelector("form")!);
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  await user.click(screen.getByRole("button", { name: "返回平台列表" }));
  await user.click(screen.getByRole("button", { name: "Qoder" }));
  await act(async () => resolve());
  expect(
    screen.getByText(/不会以自定义 API 或 SDK 配置代替账号接入/),
  ).toBeInTheDocument();
});

it("does not let a late manual save leave a newer account destination", async () => {
  let resolve!: () => void;
  const save = vi.fn(
    () =>
      new Promise<void>((r) => {
        resolve = r;
      }),
  );
  const back = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <AddHubPage
        sourceAppId="codex"
        initialTab="manual"
        onBack={back}
        onAddProvider={save}
      />
    </QueryClientProvider>,
  );
  const user = userEvent.setup();
  await user.type(document.querySelector('input[name="name"]')!, "Manual test");
  await user.type(
    screen.getByLabelText("API Key", { selector: "input" }),
    "synthetic-test-key",
  );
  await user.type(
    document.getElementById("codexBaseUrl")!,
    "https://example.test/v1",
  );
  fireEvent.submit(document.querySelector("form")!);
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  await user.click(screen.getByRole("tab", { name: "账号" }));
  await user.click(screen.getByRole("button", { name: "Qoder" }));
  await act(async () => resolve());
  expect(back).not.toHaveBeenCalled();
  expect(
    screen.getByText(/不会以自定义 API 或 SDK 配置代替账号接入/),
  ).toBeInTheDocument();
});
it("does not erase a newer manual draft when an older save completes", async () => {
  let resolve!: () => void;
  const save = vi.fn(
    () =>
      new Promise<void>((r) => {
        resolve = r;
      }),
  );
  const back = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <AddHubPage
        sourceAppId="codex"
        initialTab="manual"
        onBack={back}
        onAddProvider={save}
      />
    </QueryClientProvider>,
  );
  const user = userEvent.setup();
  await user.type(document.querySelector('input[name="name"]')!, "Old draft");
  await user.type(
    screen.getByLabelText("API Key", { selector: "input" }),
    "synthetic-test-key",
  );
  await user.type(
    document.getElementById("codexBaseUrl")!,
    "https://example.test/v1",
  );
  fireEvent.submit(document.querySelector("form")!);
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  await user.click(screen.getByRole("tab", { name: "账号" }));
  await user.click(
    screen.getByRole("tab", { name: "loongport.addEntry.manual" }),
  );
  const name = document.querySelector('input[name="name"]')!;
  await user.clear(name);
  await user.type(name, "New draft");
  await act(async () => resolve());
  expect(document.querySelector('input[name="name"]')).toHaveValue("New draft");
  expect(back).not.toHaveBeenCalled();
});

it("ignores an old source save across hide/restore while keeping its Back action valid", async () => {
  let resolve!: () => void;
  const save = vi.fn(
    () =>
      new Promise<void>((r) => {
        resolve = r;
      }),
  );
  const client = createTestQueryClient();
  const view = (active: boolean) => (
    <QueryClientProvider client={client}>
      <PreservedView active={active}>
        <AddHubPage sourceAppId="codex" onBack={vi.fn()} onAddProvider={save} />
      </PreservedView>
    </QueryClientProvider>
  );
  const { rerender } = render(view(true));
  const user = userEvent.setup();
  await user.click(
    screen.getByRole("button", { name: "OpenRouter · API Key" }),
  );
  await screen.findByDisplayValue("OpenRouter");
  await user.type(
    screen.getByLabelText("API Key", { selector: "input" }),
    "synthetic-test-key",
  );
  fireEvent.submit(document.querySelector("form")!);
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  rerender(view(false));
  rerender(view(true));
  await act(async () => resolve());
  expect(screen.getByDisplayValue("OpenRouter")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "返回平台列表" }));
  expect(screen.getByRole("button", { name: "Qoder" })).toBeInTheDocument();
});

it("does not reset a manual draft after an internal form Tab round trip", async () => {
  let resolve!: () => void;
  const save = vi.fn(
    () =>
      new Promise<void>((r) => {
        resolve = r;
      }),
  );
  const back = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <AddHubPage
        sourceAppId="codex"
        initialTab="manual"
        onBack={back}
        onAddProvider={save}
      />
    </QueryClientProvider>,
  );
  const user = userEvent.setup();
  await user.type(document.querySelector('input[name="name"]')!, "Old draft");
  await user.type(
    screen.getByLabelText("API Key", { selector: "input" }),
    "synthetic-test-key",
  );
  await user.type(
    document.getElementById("codexBaseUrl")!,
    "https://example.test/v1",
  );
  fireEvent.submit(document.querySelector("form")!);
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  await user.click(screen.getByRole("tab", { name: "provider.tabUniversal" }));
  await user.click(screen.getByRole("tab", { name: /apps.codex/ }));
  const name = document.querySelector('input[name="name"]')!;
  await user.clear(name);
  await user.type(name, "New draft");
  await act(async () => resolve());
  expect(document.querySelector('input[name="name"]')).toHaveValue("New draft");
  expect(back).not.toHaveBeenCalled();
});
