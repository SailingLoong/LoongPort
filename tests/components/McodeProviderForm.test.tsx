import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { McodeProviderForm } from "@/components/providers/forms/McodeProviderForm";

const existing = vi.hoisted(() => ({
  providers: {} as Record<string, unknown>,
}));
vi.mock("@/lib/query/queries", () => ({
  useProvidersQuery: () => ({ data: existing }),
}));
vi.mock("@/components/JsonEditor", () => ({
  default: ({
    id,
    value,
    onChange,
    readOnly,
  }: {
    id: string;
    value: string;
    onChange: (value: string) => void;
    readOnly?: boolean;
  }) => (
    <textarea
      id={id}
      value={value}
      readOnly={readOnly}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));
const config = {
  name: "Native provider",
  kind: "custom",
  enabled: false,
  api: "openai-completions",
  options: {
    baseURL: "https://example.invalid/v1",
    apiKey: "fake-test-key",
    headers: { "X-Test": "keep" },
    futureOption: { keep: true },
  },
  models: {
    "model-a": {
      name: "Model A",
      limit: { context: 128000, output: 8192 },
      futureModel: true,
    },
  },
  futureRoot: { keep: true },
};
function show(overrides: Record<string, unknown> = {}) {
  const onSubmit = vi.fn().mockResolvedValue(undefined);
  render(
    <McodeProviderForm
      appId="mcode"
      submitLabel="Save MiniMax Code"
      onSubmit={onSubmit}
      onCancel={vi.fn()}
      {...overrides}
    />,
  );
  return onSubmit;
}

describe("McodeProviderForm", () => {
  it("adds custom_provider data using a stable provider key, not MiniMax account auth", async () => {
    const onSubmit = show();
    fireEvent.change(screen.getByLabelText(/opencode.providerKey/), {
      target: { value: "my-provider" },
    });
    fireEvent.change(screen.getByLabelText("provider.name"), {
      target: { value: "My provider" },
    });
    fireEvent.change(screen.getByLabelText("provider.configJson"), {
      target: { value: JSON.stringify(config) },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save MiniMax Code" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(onSubmit.mock.calls[0][0].providerKey).toBe("my-provider");
    expect(JSON.parse(onSubmit.mock.calls[0][0].settingsConfig)).toMatchObject({
      ...config,
      name: "My provider",
    });
    expect(screen.queryByText(/OAuth|MCP|登录/)).not.toBeInTheDocument();
  });

  it("locks an existing key and preserves unknown config fields on edit", async () => {
    const onSubmit = show({
      providerId: "my-provider",
      initialData: {
        name: config.name,
        settingsConfig: config,
        meta: { mcodeConfigRevision: "test-revision" },
      },
    });
    expect(screen.getByLabelText(/opencode.providerKey/)).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Save MiniMax Code" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(JSON.parse(onSubmit.mock.calls[0][0].settingsConfig)).toEqual(
      config,
    );
    expect(onSubmit.mock.calls[0][0].meta).toEqual({
      mcodeConfigRevision: "test-revision",
    });
  });

  it("retains invalid JSON and disables save until it is repaired", async () => {
    const onSubmit = show({
      providerId: "my-provider",
      initialData: { name: config.name, settingsConfig: config },
    });
    fireEvent.change(screen.getByLabelText("provider.configJson"), {
      target: { value: "{broken" },
    });
    expect(screen.getByLabelText("provider.configJson")).toHaveValue("{broken");
    expect(
      screen.getByRole("button", { name: "Save MiniMax Code" }),
    ).toBeDisabled();
    expect(onSubmit).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("provider.configJson"), {
      target: { value: JSON.stringify(config) },
    });
    expect(
      screen.getByRole("button", { name: "Save MiniMax Code" }),
    ).toBeEnabled();
  });

  it("keeps the draft after a failed save and prevents edits and duplicate submits while busy", async () => {
    let rejectSave!: (error: Error) => void;
    const onSubmit = vi.fn(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectSave = reject;
        }),
    );
    show({
      onSubmit,
      providerId: "my-provider",
      initialData: { name: config.name, settingsConfig: config },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save MiniMax Code" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(screen.getByLabelText("provider.name")).toBeDisabled();
    expect(screen.getByLabelText("provider.configJson")).toHaveAttribute(
      "readonly",
    );
    expect(
      screen.getByRole("button", { name: "Save MiniMax Code" }),
    ).toBeDisabled();
    await act(async () => rejectSave(new Error("sourceChanged")));
    expect(screen.getByRole("alert")).toHaveTextContent("sourceChanged");
    expect(
      JSON.parse(
        (screen.getByLabelText("provider.configJson") as HTMLTextAreaElement)
          .value,
      ),
    ).toEqual(config);
    expect(
      screen.getByRole("button", { name: "Save MiniMax Code" }),
    ).toBeEnabled();
  });
  it("keeps untouched string option types when another option is edited", async () => {
    const original = {
      ...config,
      options: {
        ...config.options,
        flagText: "false",
        numberText: "123",
        timeout: 100,
      },
    };
    const submit = show({
      providerId: "my-provider",
      initialData: { name: config.name, settingsConfig: original },
    });
    fireEvent.change(screen.getByDisplayValue("100"), {
      target: { value: "200" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save MiniMax Code" }));
    await waitFor(() => expect(submit).toHaveBeenCalledTimes(1));
    expect(JSON.parse(submit.mock.calls[0][0].settingsConfig).options).toEqual({
      ...original.options,
      timeout: 200,
    });
  });

  it.each(["apiKey", "baseURL", "headers"])(
    "prevents a numeric extra option from replacing %s without crashing the form",
    async (key) => {
      const original = {
        ...config,
        options: { ...config.options, timeout: 123 },
      };
      const submit = show({
        providerId: "my-provider",
        initialData: { name: config.name, settingsConfig: original },
      });
      const input = screen.getByDisplayValue("timeout");
      fireEvent.change(input, { target: { value: key } });
      fireEvent.blur(input);
      expect(input).toHaveValue("timeout");
      fireEvent.click(
        screen.getByRole("button", { name: "Save MiniMax Code" }),
      );
      await waitFor(() => expect(submit).toHaveBeenCalledTimes(1));
      expect(
        JSON.parse(submit.mock.calls[0][0].settingsConfig).options,
      ).toEqual(original.options);
    },
  );
  it("preserves both native extra options after a duplicate key rename is rejected", async () => {
    const original = {
      ...config,
      options: { ...config.options, first: "one", second: "two" },
    };
    const submit = show({
      providerId: "my-provider",
      initialData: { name: config.name, settingsConfig: original },
    });
    const input = screen.getByDisplayValue("first");
    fireEvent.change(input, { target: { value: "second" } });
    fireEvent.blur(input);
    expect(input).toHaveValue("first");
    fireEvent.click(screen.getByRole("button", { name: "Save MiniMax Code" }));
    await waitFor(() => expect(submit).toHaveBeenCalledTimes(1));
    expect(JSON.parse(submit.mock.calls[0][0].settingsConfig).options).toEqual(
      original.options,
    );
  });
});
