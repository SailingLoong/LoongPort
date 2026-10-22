import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditProviderDialog } from "@/components/providers/EditProviderDialog";
import { providersApi } from "@/lib/api/providers";
import { useUpdateProviderMutation } from "@/lib/query/mutations";
import type { Provider } from "@/types";

vi.mock("@/lib/query/queries", () => ({
  useProvidersQuery: () => ({ data: { providers: {} } }),
}));
vi.mock("@/components/JsonEditor", () => ({
  default: ({ id, value, onChange, readOnly }: any) => (
    <textarea
      id={id}
      value={value}
      readOnly={readOnly}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const listProvider: Provider = {
  id: "native-provider",
  name: "Stale list name",
  settingsConfig: { name: "Stale list name" },
  meta: { mcodeConfigRevision: "old-list-revision", custom_endpoints: {} },
};
const nativeConfig = {
  name: "Current native name",
  kind: "custom",
  enabled: true,
  api: "openai-completions",
  options: {
    baseURL: "https://native.example.invalid/v1",
    apiKey: "synthetic-key",
    futureOption: true,
  },
  models: { "native-model": { name: "Native model" } },
};
function show() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  client.setQueryData(["providers", "mcode"], {
    providers: { [listProvider.id]: listProvider },
  });
  const close = vi.fn();
  function Editor() {
    const mutation = useUpdateProviderMutation("mcode");
    return (
      <EditProviderDialog
        open
        appId="mcode"
        provider={listProvider}
        onOpenChange={close}
        onSubmit={mutation.mutateAsync}
      />
    );
  }
  render(
    <QueryClientProvider client={client}>
      <Editor />
    </QueryClientProvider>,
  );
  return { client, close };
}
afterEach(() => vi.restoreAllMocks());

describe("MiniMax Code edit snapshot boundary", () => {
  it("uses one edit-read name, config and revision and preserves the draft on a rejected write", async () => {
    const read = vi.spyOn(providersApi, "getEditSettings").mockResolvedValue({
      settingsConfig: nativeConfig,
      modeState: null,
      mcodeConfigRevision: "fresh-edit-revision",
    });
    const update = vi
      .spyOn(providersApi, "update")
      .mockRejectedValue(new Error("sourceChanged"));
    const { close, client } = show();
    const name = await screen.findByDisplayValue("Current native name");
    expect(read).toHaveBeenCalledExactlyOnceWith(listProvider.id, "mcode");
    expect(screen.getByLabelText("provider.configJson")).toHaveValue(
      JSON.stringify(nativeConfig, null, 2),
    );
    fireEvent.change(name, { target: { value: "My retained draft" } });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(update).toHaveBeenCalledWith(
      expect.objectContaining({
        id: listProvider.id,
        name: "My retained draft",
        settingsConfig: { ...nativeConfig, name: "My retained draft" },
        meta: {
          ...listProvider.meta,
          mcodeConfigRevision: "fresh-edit-revision",
        },
      }),
      "mcode",
      listProvider.id,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("sourceChanged");
    expect(name).toHaveValue("My retained draft");
    expect(close).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
    expect(client.getQueryData(["providers", "mcode"])).toEqual({
      providers: { [listProvider.id]: listProvider },
    });
  });

  it.each([undefined, "", "   "])(
    "refuses a revisionless edit read (%s), never falling back to the list revision",
    async (revision) => {
      const read = vi.spyOn(providersApi, "getEditSettings").mockResolvedValue({
        settingsConfig: nativeConfig,
        modeState: null,
        mcodeConfigRevision: revision,
      });
      const update = vi.spyOn(providersApi, "update").mockResolvedValue(true);
      show();
      expect(
        await screen.findByText("provider.preview.sourceUnavailable"),
      ).toBeInTheDocument();
      expect(screen.queryByLabelText("provider.name")).not.toBeInTheDocument();
      const save = screen.getByRole("button", { name: "common.save" });
      expect(save).toBeDisabled();
      fireEvent.click(save);
      expect(update).not.toHaveBeenCalled();
      expect(read).toHaveBeenCalledTimes(1);
      expect(
        screen.getByRole("button", { name: "provider.preview.reread" }),
      ).toBeEnabled();
    },
  );

  it("uses the list display name only when the edit snapshot has no string name", async () => {
    vi.spyOn(providersApi, "getEditSettings").mockResolvedValue({
      settingsConfig: { ...nativeConfig, name: undefined },
      modeState: null,
      mcodeConfigRevision: "fresh-edit-revision",
    });
    const update = vi.spyOn(providersApi, "update").mockResolvedValue(true);
    show();
    expect(
      await screen.findByDisplayValue(listProvider.name),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(update.mock.calls[0][0].meta?.mcodeConfigRevision).toBe(
      "fresh-edit-revision",
    );
  });
});
