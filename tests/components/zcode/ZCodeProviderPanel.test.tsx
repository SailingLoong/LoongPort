import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ZCodeProviderPanel } from "@/components/zcode/ZCodeProviderPanel";
import { zcodeApi, type ZCodeConfig } from "@/lib/api/zcode";

vi.mock("@/lib/api/zcode", () => ({
  zcodeApi: { read: vi.fn(), save: vi.fn(), remove: vi.fn() },
}));
const fixture: ZCodeConfig = {
  revision: "revision-one",
  providers: [
    {
      id: "loongport-test",
      name: "Managed example",
      apiType: "openai-responses",
      baseUrl: "https://api.example/v1",
      models: ["example-model"],
      hasApiKey: true,
      managed: true,
    },
    {
      id: "native",
      name: "Native example",
      apiType: "anthropic-messages",
      baseUrl: "https://native.example",
      models: ["native-model"],
      hasApiKey: true,
      managed: false,
    },
  ],
};
function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ZCodeProviderPanel />
    </QueryClientProvider>,
  );
  return client;
}
beforeEach(() => {
  vi.mocked(zcodeApi.read).mockResolvedValue(fixture);
  vi.mocked(zcodeApi.save).mockResolvedValue({
    ...fixture,
    revision: "revision-two",
  });
  vi.mocked(zcodeApi.remove).mockResolvedValue({
    revision: "revision-two",
    providers: [fixture.providers[1]],
  });
});
describe("ZCode native provider panel", () => {
  it("reloads a conflict in the editor, preserves input and requires explicit resolution and save", async () => {
    const client = mount();
    await screen.findByText("Managed example");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "My draft" },
    });
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "fake-private-draft" },
    });
    vi.mocked(zcodeApi.save).mockRejectedValueOnce({
      code: "zcode.configuration_changed",
      message: "conflict",
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const reload = await screen.findByRole("button", {
      name: "Read latest configuration",
    });
    vi.mocked(zcodeApi.read).mockResolvedValueOnce({
      ...fixture,
      revision: "external-revision",
      providers: [{ ...fixture.providers[0], name: "External name" }],
    });
    fireEvent.click(reload);
    await screen.findAllByText("External name");
    expect(screen.getByLabelText("Name")).toHaveValue("My draft");
    expect(screen.queryByText("fake-private-draft")).not.toBeInTheDocument();
    expect(JSON.stringify(client.getQueryData(["zcodeConfig"]))).not.toContain(
      "fake-private-draft",
    );
    expect(zcodeApi.save).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Keep my input" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(zcodeApi.save).toHaveBeenLastCalledWith(
        expect.objectContaining({
          revision: "external-revision",
          name: "My draft",
          apiKey: "fake-private-draft",
        }),
      ),
    );
  });

  it("lets the user adopt external values without exposing or retaining a replacement key", async () => {
    mount();
    await screen.findByText("Managed example");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "fake-private-draft" },
    });
    vi.mocked(zcodeApi.save).mockRejectedValueOnce({
      code: "zcode.configuration_changed",
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    vi.mocked(zcodeApi.read).mockResolvedValueOnce({
      ...fixture,
      revision: "external",
      providers: [{ ...fixture.providers[0], name: "External name" }],
    });
    fireEvent.click(
      await screen.findByRole("button", { name: "Read latest configuration" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Use external values" }),
    );
    expect(screen.getByLabelText("Name")).toHaveValue("External name");
    expect(screen.getByLabelText("API Key")).toHaveValue("");
    expect(zcodeApi.save).toHaveBeenCalledTimes(1);
  });

  it.each([
    { providers: [] },
    { providers: [{ ...fixture.providers[0], managed: false }] },
  ])(
    "does not revive or take over a removed or no longer managed provider",
    async ({ providers }) => {
      mount();
      await screen.findByText("Managed example");
      fireEvent.click(screen.getByRole("button", { name: "Edit" }));
      vi.mocked(zcodeApi.save).mockRejectedValueOnce({
        code: "zcode.configuration_changed",
      });
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      vi.mocked(zcodeApi.read).mockResolvedValueOnce({
        revision: "external",
        providers,
      });
      fireEvent.click(
        await screen.findByRole("button", {
          name: "Read latest configuration",
        }),
      );
      await screen.findByText(
        "This provider was removed or is now managed in ZCode. Your input has not been saved.",
      );
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
      expect(zcodeApi.save).toHaveBeenCalledTimes(1);
    },
  );

  it("keeps a failed reload read-only and protects against another external change", async () => {
    mount();
    await screen.findByText("Managed example");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    vi.mocked(zcodeApi.save).mockRejectedValue({
      code: "zcode.configuration_changed",
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    vi.mocked(zcodeApi.read).mockRejectedValueOnce(
      new Error("Cannot read file"),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Read latest configuration" }),
    );
    await screen.findByText("Cannot read file");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    vi.mocked(zcodeApi.read).mockResolvedValueOnce({
      ...fixture,
      revision: "external",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Read latest configuration" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Keep my input" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("button", { name: "Read latest configuration" });
    expect(zcodeApi.save).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
  it("shows native providers read-only and edits with the original revision without retrieving a key", async () => {
    mount();
    await screen.findByText("Managed example");
    expect(screen.getAllByRole("button", { name: "Edit" })).toHaveLength(1);
    expect(screen.getByText("Native example")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText("API Key")).toHaveValue("");
    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "Updated" },
    });
    fireEvent.change(screen.getByLabelText("Models (one per line)"), {
      target: { value: "first\nsecond" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(zcodeApi.save).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "loongport-test",
          revision: "revision-one",
          name: "Updated",
          apiKey: null,
          models: ["first", "second"],
        }),
      ),
    );
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });
  it("clears cancelled keys and never stores save inputs in the query cache", async () => {
    const client = mount();
    await screen.findByText("Managed example");
    fireEvent.click(screen.getByRole("button", { name: "Add provider" }));
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "fake-secret-cancelled" },
    });
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: "Cancel",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Add provider" }));
    expect(screen.getByLabelText("API Key")).toHaveValue("");
    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "New example" },
    });
    fireEvent.change(screen.getByLabelText("Base URL"), {
      target: { value: "https://api.example/v1" },
    });
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "fake-secret-new" },
    });
    fireEvent.change(screen.getByLabelText("Models (one per line)"), {
      target: { value: "example-model" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(zcodeApi.save).toHaveBeenCalledTimes(1));
    expect(
      JSON.stringify(
        client
          .getQueryCache()
          .getAll()
          .map((query) => query.state.data),
      ),
    ).not.toContain("fake-secret");
    expect(client.getMutationCache().getAll()).toHaveLength(0);
  });
  it("keeps conflict errors visible and prevents repeat submissions", async () => {
    let reject!: (error: Error) => void;
    vi.mocked(zcodeApi.save).mockImplementation(
      () =>
        new Promise((_, fail) => {
          reject = fail;
        }),
    );
    mount();
    await screen.findByText("Managed example");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const save = screen.getByRole("button", { name: "Save" });
    fireEvent.click(save);
    fireEvent.click(save);
    await waitFor(() => expect(zcodeApi.save).toHaveBeenCalledTimes(1));
    reject(new Error("Configuration changed; refresh before saving"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Configuration changed",
    );
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
  it("requires deletion confirmation and uses the displayed revision", async () => {
    mount();
    await screen.findByText("Managed example");
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(zcodeApi.remove).not.toHaveBeenCalled();
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: "Remove",
      }),
    );
    await waitFor(() =>
      expect(zcodeApi.remove).toHaveBeenCalledWith(
        "loongport-test",
        "revision-one",
      ),
    );
    await waitFor(() =>
      expect(screen.queryByText("Managed example")).not.toBeInTheDocument(),
    );
  });
  it("shows a failed removal inside the confirmation dialog", async () => {
    vi.mocked(zcodeApi.remove).mockRejectedValue(
      new Error("Configuration changed; refresh before removing"),
    );
    mount();
    await screen.findByText("Managed example");
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: "Remove",
      }),
    );
    expect(
      await within(screen.getByRole("dialog")).findByText(
        "Configuration changed; refresh before removing",
      ),
    ).toBeInTheDocument();
  });

  it("does not offer writes when the file cannot be safely read", async () => {
    vi.mocked(zcodeApi.read).mockRejectedValue(
      new Error("Unsupported schema; file preserved"),
    );
    mount();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Unsupported schema",
    );
    expect(screen.getByRole("button", { name: "Add provider" })).toBeDisabled();
  });
});
