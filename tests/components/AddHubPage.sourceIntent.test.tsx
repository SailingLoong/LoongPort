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
import { ProviderForm } from "@/components/providers/forms/ProviderForm";
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

it("clears the real manual form after successful completion", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
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
  await waitFor(() =>
    expect(document.querySelector('input[name="name"]')).toHaveValue(""),
  );
  expect(back).toHaveBeenCalledTimes(1);
});

async function setupManualSave(save: (value: any) => Promise<void>) {
  const back = vi.fn();
  const client = createTestQueryClient();
  const view = (active: boolean) => (
    <QueryClientProvider client={client}>
      <PreservedView active={active}>
        <AddHubPage
          sourceAppId="codex"
          initialTab="manual"
          onBack={back}
          onAddProvider={save}
        />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  const user = userEvent.setup();
  await user.type(
    document.querySelector('input[name="name"]')!,
    "Retained draft",
  );
  await user.type(
    screen.getByLabelText("API Key", { selector: "input" }),
    "synthetic-key",
  );
  await user.type(
    document.getElementById("codexBaseUrl")!,
    "https://example.test/v1",
  );
  return {
    back,
    user,
    rendered,
    view,
    submit: () => fireEvent.submit(document.querySelector("form")!),
  };
}

it("shows a failed real save on its original form, keeps input, and retries successfully", async () => {
  const save = vi
    .fn()
    .mockRejectedValueOnce(new Error("Synthetic save failure"))
    .mockResolvedValue(undefined);
  const { back, submit } = await setupManualSave(save);
  submit();
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "notifications.saveFailedGeneric",
  );
  expect(document.querySelector('input[name="name"]')).toHaveValue(
    "Retained draft",
  );
  expect(back).not.toHaveBeenCalled();
  submit();
  await waitFor(() => expect(back).toHaveBeenCalledTimes(1));
  expect(save).toHaveBeenCalledTimes(2);
  expect(document.querySelector('input[name="name"]')).toHaveValue("");
  expect(
    screen.queryByText("notifications.saveFailedGeneric"),
  ).not.toBeInTheDocument();
});

it("does not send duplicate real saves while one submission is pending", async () => {
  let resolve!: () => void;
  const save = vi.fn(
    () =>
      new Promise<void>((done) => {
        resolve = done;
      }),
  );
  const { back, submit } = await setupManualSave(save);
  submit();
  submit();
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  await act(async () => resolve());
  expect(back).toHaveBeenCalledTimes(1);
});

it("does not attach a late save failure to an edited draft", async () => {
  let reject!: (error: Error) => void;
  const save = vi.fn(
    () =>
      new Promise<void>((_, fail) => {
        reject = fail;
      }),
  );
  const { back, submit, user } = await setupManualSave(save);
  submit();
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  const name = document.querySelector('input[name="name"]')!;
  await user.clear(name);
  await user.type(name, "Newer draft");
  await act(async () => reject(new Error("Late synthetic save failure")));
  expect(document.querySelector('input[name="name"]')).toHaveValue(
    "Newer draft",
  );
  expect(
    screen.queryByText("notifications.saveFailedGeneric"),
  ).not.toBeInTheDocument();
  expect(back).not.toHaveBeenCalled();
});

it("does not attach a late failure to a restored form session", async () => {
  let reject!: (error: Error) => void;
  const save = vi.fn(
    () =>
      new Promise<void>((_, fail) => {
        reject = fail;
      }),
  );
  const { back, submit, rendered, view } = await setupManualSave(save);
  submit();
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  await act(async () => reject(new Error("Old session synthetic failure")));
  expect(document.querySelector('input[name="name"]')).toHaveValue(
    "Retained draft",
  );
  expect(
    screen.queryByText("notifications.saveFailedGeneric"),
  ).not.toBeInTheDocument();
  expect(back).not.toHaveBeenCalled();
});

it("does not attach an old save failure after a hook-owned API format change", async () => {
  let reject!: (error: Error) => void;
  const save = vi.fn(
    () =>
      new Promise<void>((_, fail) => {
        reject = fail;
      }),
  );
  const { back, submit, user } = await setupManualSave(save);
  submit();
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  const advanced = screen.getByRole("button", {
    name: /高级选项|Advanced options|providerForm.advancedOptionsToggle/i,
  });
  if (advanced.getAttribute("aria-expanded") !== "true")
    await user.click(advanced);
  fireEvent.keyDown(document.getElementById("codex-upstream-format")!, {
    key: "ArrowDown",
  });
  await user.click(
    await screen.findByRole("option", {
      name: /Chat Completions|codexConfig.upstreamFormatChat/,
    }),
  );
  await act(async () => reject(new Error("Old API format synthetic failure")));
  expect(document.getElementById("codex-upstream-format")).toHaveTextContent(
    /Chat Completions|codexConfig.upstreamFormatChat/,
  );
  expect(
    screen.queryByText("notifications.saveFailedGeneric"),
  ).not.toBeInTheDocument();
  expect(back).not.toHaveBeenCalled();
});

it("releases the real submit gate after resolver validation fails", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  const { back, submit, user } = await setupManualSave(save);
  const website = document.querySelector('input[name="websiteUrl"]')!;
  await user.type(website, "not-a-url");
  submit();
  expect(await screen.findByText("请输入有效的网址")).toBeInTheDocument();
  expect(save).not.toHaveBeenCalled();
  await user.clear(website);
  submit();
  await waitFor(() => expect(back).toHaveBeenCalledTimes(1));
  expect(save).toHaveBeenCalledTimes(1);
});

it("does not release a restored session's pending submit gate when an older save fails", async () => {
  let rejectOld!: (error: Error) => void;
  let resolveNew!: () => void;
  const save = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise<void>((_, fail) => {
          rejectOld = fail;
        }),
    )
    .mockImplementationOnce(
      () =>
        new Promise<void>((done) => {
          resolveNew = done;
        }),
    );
  const { back, submit, rendered, view } = await setupManualSave(save);
  submit();
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  submit();
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  await act(async () => rejectOld(new Error("Old submission failed")));
  await act(async () => submit());
  expect(save).toHaveBeenCalledTimes(2);
  await act(async () => resolveNew());
  expect(save).toHaveBeenCalledTimes(2);
  expect(back).toHaveBeenCalledTimes(1);
});

it("does not attach a late failure after only the Hermes provider key changes", async () => {
  let reject!: (error: Error) => void;
  const save = vi.fn(
    () =>
      new Promise<void>((_, fail) => {
        reject = fail;
      }),
  );
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <ProviderForm
        appId="hermes"
        providerId="original-key"
        submitLabel="save"
        onSubmit={save}
        onCancel={vi.fn()}
        initialData={{
          name: "Synthetic Hermes",
          settingsConfig: {
            base_url: "https://example.test/v1",
            api_key: "synthetic-key",
            api_mode: "chat_completions",
            models: [],
          },
        }}
      />
    </QueryClientProvider>,
  );
  fireEvent.submit(document.querySelector("form")!);
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  const key = document.getElementById("hermes-key")!;
  fireEvent.change(key, { target: { value: "new-key" } });
  await act(async () => reject(new Error("Old Hermes key synthetic failure")));
  expect(key).toHaveValue("new-key");
  expect(
    screen.queryByText("notifications.saveFailedGeneric"),
  ).not.toBeInTheDocument();
});
