import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  createElement as h,
  type PropsWithChildren,
  type ComponentProps,
} from "react";
import { useForm } from "react-hook-form";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  act,
} from "@testing-library/react";
import { CodexFormFields } from "@/components/providers/forms/CodexFormFields";
import { OpenCodeFormFields } from "@/components/providers/forms/OpenCodeFormFields";
import { Form } from "@/components/ui/form";
const mocked = vi.hoisted(() => ({ fetch: vi.fn() }));
vi.mock("@/lib/api/model-fetch", () => ({
  fetchModelsForConfig: mocked.fetch,
  fetchXaiOauthModels: vi.fn(),
  showFetchModelsError: vi.fn(),
}));
const freeRows = [
  {
    id: "glm-4.7-flash",
    ownedBy: "zhipu",
    isFree: true,
    pricing: { prompt: "0", completion: "0" },
  },
  { id: "paid-model", ownedBy: "zhipu", isFree: false },
];
function Shell({ children }: PropsWithChildren) {
  const form = useForm();
  return h(Form, { ...form, children });
}
const codexProps: ComponentProps<typeof CodexFormFields> = {
  codexApiKey: "synthetic-key",
  onApiKeyChange: vi.fn(),
  category: "custom",
  shouldShowApiKeyLink: false,
  websiteUrl: "",
  shouldShowSpeedTest: false,
  codexBaseUrl: "https://open.bigmodel.cn/api/paas/v4",
  onBaseUrlChange: vi.fn(),
  isFullUrl: false,
  onFullUrlChange: vi.fn(),
  isEndpointModalOpen: false,
  onEndpointModalToggle: vi.fn(),
  autoSelect: false,
  onAutoSelectChange: vi.fn(),
  codexModel: "glm-4.7-flash",
  onModelChange: vi.fn(),
  apiFormat: "openai_chat",
  onApiFormatChange: vi.fn(),
  anthropicAuthField: "ANTHROPIC_AUTH_TOKEN",
  onAnthropicAuthFieldChange: vi.fn(),
  impersonateClaudeCode: false,
  onImpersonateClaudeCodeChange: vi.fn(),
  maxOutputTokens: "",
  onMaxOutputTokensChange: vi.fn(),
  promptCacheRouting: "disabled",
  onPromptCacheRoutingChange: vi.fn(),
  catalogModels: [{ model: "glm-4.7-flash" }],
  speedTestEndpoints: [],
  customUserAgent: "",
  onCustomUserAgentChange: vi.fn(),
  localProxyHeadersOverride: "",
  onLocalProxyHeadersOverrideChange: vi.fn(),
  localProxyBodyOverride: "",
  onLocalProxyBodyOverrideChange: vi.fn(),
};
const openCodeProps: ComponentProps<typeof OpenCodeFormFields> = {
  npm: "@ai-sdk/openai-compatible",
  onNpmChange: vi.fn(),
  apiKey: "synthetic-key",
  onApiKeyChange: vi.fn(),
  category: "custom",
  shouldShowApiKeyLink: false,
  websiteUrl: "",
  baseUrl: "https://open.bigmodel.cn/api/paas/v4",
  onBaseUrlChange: vi.fn(),
  headers: {},
  onHeadersChange: vi.fn(),
  models: { "glm-4.7-flash": { name: "Synthetic free" } },
  onModelsChange: vi.fn(),
  extraOptions: {},
  onExtraOptionsChange: vi.fn(),
};
beforeEach(() => {
  mocked.fetch.mockReset();
  mocked.fetch.mockResolvedValue(freeRows);
});
describe("free entry audit", () => {
  it("Codex keeps fetched free metadata for IDs already in the saved catalog", async () => {
    render(h(Shell, null, h(CodexFormFields, codexProps)));
    fireEvent.click(screen.getByTitle("providerForm.fetchModels"));
    await waitFor(() => expect(mocked.fetch).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole("button", { name: "Select model" }));
    await screen.findByRole("button", { name: "providerForm.freeOnly" });
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.freeOnly" }),
    );
    expect(
      screen.queryByRole("option", { name: /glm-4.7-flash/ }),
    ).toBeVisible();
  });
  it("OpenCode retires fetched free facts when the source endpoint changes", async () => {
    const view = render(h(Shell, null, h(OpenCodeFormFields, openCodeProps)));
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );
    await screen.findByRole("button", { name: "Select model" });
    view.rerender(
      h(
        Shell,
        null,
        h(OpenCodeFormFields, {
          ...openCodeProps,
          baseUrl: "https://synthetic-paid.example/v1",
        }),
      ),
    );
    expect(
      screen.queryByRole("button", { name: "Select model" }),
    ).not.toBeInTheDocument();
  });
  it("OpenCode ignores a free-model response from the previous endpoint", async () => {
    let resolve!: (rows: typeof freeRows) => void;
    mocked.fetch.mockReturnValueOnce(
      new Promise((r) => {
        resolve = r;
      }),
    );
    const view = render(h(Shell, null, h(OpenCodeFormFields, openCodeProps)));
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );
    view.rerender(
      h(
        Shell,
        null,
        h(OpenCodeFormFields, {
          ...openCodeProps,
          baseUrl: "https://synthetic-paid.example/v1",
        }),
      ),
    );
    await act(async () => {
      resolve(freeRows);
    });
    expect(
      screen.queryByRole("button", { name: "Select model" }),
    ).not.toBeInTheDocument();
  });
});

function deferredRows() {
  let resolve!: (rows: typeof freeRows) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<typeof freeRows>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
it("OpenCode A to B to A never accepts the first A response", async () => {
  const old = deferredRows();
  mocked.fetch.mockReturnValueOnce(old.promise);
  const view = render(h(Shell, null, h(OpenCodeFormFields, openCodeProps)));
  fireEvent.click(
    screen.getByRole("button", { name: "providerForm.fetchModels" }),
  );
  view.rerender(
    h(
      Shell,
      null,
      h(OpenCodeFormFields, {
        ...openCodeProps,
        baseUrl: "https://b.example/v1",
      }),
    ),
  );
  view.rerender(h(Shell, null, h(OpenCodeFormFields, openCodeProps)));
  await act(async () => old.resolve(freeRows));
  expect(
    screen.queryByRole("button", { name: "Select model" }),
  ).not.toBeInTheDocument();
  fireEvent.click(
    screen.getByRole("button", { name: "providerForm.fetchModels" }),
  );
  await screen.findByRole("button", { name: "Select model" });
  expect(mocked.fetch).toHaveBeenCalledTimes(2);
});
it("OpenCode clearing or changing credentials retires facts and pending work", async () => {
  const old = deferredRows();
  mocked.fetch.mockReturnValueOnce(old.promise);
  const view = render(h(Shell, null, h(OpenCodeFormFields, openCodeProps)));
  fireEvent.click(
    screen.getByRole("button", { name: "providerForm.fetchModels" }),
  );
  view.rerender(
    h(Shell, null, h(OpenCodeFormFields, { ...openCodeProps, apiKey: "" })),
  );
  await act(async () => old.resolve(freeRows));
  expect(
    screen.queryByRole("button", { name: "Select model" }),
  ).not.toBeInTheDocument();
  view.rerender(
    h(
      Shell,
      null,
      h(OpenCodeFormFields, { ...openCodeProps, apiKey: "synthetic-new" }),
    ),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "providerForm.fetchModels" }),
  );
  await screen.findByRole("button", { name: "Select model" });
  view.rerender(
    h(
      Shell,
      null,
      h(OpenCodeFormFields, { ...openCodeProps, apiKey: "synthetic-third" }),
    ),
  );
  expect(
    screen.queryByRole("button", { name: "Select model" }),
  ).not.toBeInTheDocument();
});
it("OpenCode late old failure cannot cancel a new source fetch", async () => {
  const old = deferredRows(),
    fresh = deferredRows();
  mocked.fetch
    .mockReturnValueOnce(old.promise)
    .mockReturnValueOnce(fresh.promise);
  const view = render(h(Shell, null, h(OpenCodeFormFields, openCodeProps)));
  fireEvent.click(
    screen.getByRole("button", { name: "providerForm.fetchModels" }),
  );
  view.rerender(
    h(
      Shell,
      null,
      h(OpenCodeFormFields, {
        ...openCodeProps,
        baseUrl: "https://b.example/v1",
      }),
    ),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "providerForm.fetchModels" }),
  );
  await act(async () => old.reject(new Error("old synthetic failure")));
  expect(
    screen.getByRole("button", { name: "providerForm.fetchModels" }),
  ).toBeDisabled();
  await act(async () => fresh.resolve(freeRows));
  await screen.findByRole("button", { name: "Select model" });
});
it("OpenCode unmount rejects late response and new instance starts empty", async () => {
  const old = deferredRows();
  mocked.fetch.mockReturnValueOnce(old.promise);
  const view = render(h(Shell, null, h(OpenCodeFormFields, openCodeProps)));
  fireEvent.click(
    screen.getByRole("button", { name: "providerForm.fetchModels" }),
  );
  view.unmount();
  render(h(Shell, null, h(OpenCodeFormFields, openCodeProps)));
  await act(async () => old.resolve(freeRows));
  expect(
    screen.queryByRole("button", { name: "Select model" }),
  ).not.toBeInTheDocument();
});
it("OpenCode duplicate click stays one request and a failed request can retry", async () => {
  const first = deferredRows();
  mocked.fetch.mockReturnValueOnce(first.promise);
  const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
  try {
    render(h(Shell, null, h(OpenCodeFormFields, openCodeProps)));
    const button = screen.getByRole("button", {
      name: "providerForm.fetchModels",
    });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(mocked.fetch).toHaveBeenCalledTimes(1);
    await act(async () => first.reject(new Error("synthetic failed request")));
    expect(button).toBeEnabled();
    fireEvent.click(button);
    await screen.findByRole("button", { name: "Select model" });
    expect(mocked.fetch).toHaveBeenCalledTimes(2);
  } finally {
    warning.mockRestore();
  }
});
