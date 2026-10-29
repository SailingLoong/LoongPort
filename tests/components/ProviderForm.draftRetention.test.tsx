import { applicationRoutingApi } from "@/lib/api/applicationRouting";
import { configApi } from "@/lib/api";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { ProviderForm } from "@/components/providers/forms/ProviderForm";
import { PreservedView } from "@/components/ui/PreservedView";
import { createTestQueryClient } from "../utils/testQueryClient";
vi.mock("@/lib/api/applicationRouting", async (original) => ({
  ...(await original<typeof import("@/lib/api/applicationRouting")>()),
  applicationRoutingApi: {
    get: vi.fn().mockResolvedValue({
      modeState: {
        status: "ready",
        canWrite: true,
        writeProtocol: "legacy",
        blockedReason: null,
      },
    }),
  },
}));
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
async function cleanupSnippetForm(
  unmount: () => void,
  client: ReturnType<typeof createTestQueryClient>,
) {
  unmount();
  await client.cancelQueries();
  client.clear();
}

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
      const { rerender, unmount } = render(view(true));
      try {
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
      } finally {
        unmount();
        await client.cancelQueries();
        client.clear();
      }
    },
  );
});

it("keeps an open Claude snippet readable when the backend cache freezes its write capability", async ({
  onTestFinished,
}) => {
  const client = createTestQueryClient();
  client.setQueryData(["applicationRouting", "claude"], {
    modeState: {
      status: "ready",
      canWrite: true,
      writeProtocol: "legacy",
      blockedReason: null,
    },
  });
  const save = vi
    .spyOn(configApi, "setCommonConfigSnippet")
    .mockResolvedValue(undefined);
  // The runner ends ownership even when this async test body times out.
  onTestFinished(() => save.mockRestore());
  const { unmount } = render(
    <QueryClientProvider client={client}>
      <ProviderForm
        appId="claude"
        submitLabel="save"
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />
    </QueryClientProvider>,
  );
  try {
    await waitFor(() =>
      expect(
        screen.getByRole("checkbox", {
          name: "claudeConfig.writeCommonConfig",
        }),
      ).toBeEnabled(),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "claudeConfig.editCommonConfig" }),
    );
    await act(async () => {
      client.setQueryData(["applicationRouting", "claude"], {
        modeState: {
          status: "ready",
          canWrite: true,
          legacyCommonConfigWritable: false,
          writeProtocol: "native",
          blockedReason: null,
        },
      });
    });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "common.save" }),
      ).toBeDisabled(),
    );
    const editor = screen
      .getAllByRole("textbox", { name: "configuration JSON" })
      .at(-1)!;
    fireEvent.change(editor, { target: { value: '{"safe":"changed"}' } });
    expect(save).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(screen.getByText("commonConfig.frozenHint")).toBeVisible(),
    );
    await userEvent.click(
      screen.getAllByRole("button", { name: "common.cancel" }).at(-1)!,
    );
    await waitFor(() =>
      expect(
        screen.queryByText("commonConfig.frozenHint"),
      ).not.toBeInTheDocument(),
    );
  } finally {
    await cleanupSnippetForm(unmount, client);
  }
});

it("preserves a frozen provider's existing common-config flag when saving ordinary fields", async () => {
  const client = createTestQueryClient();
  const admission = {
    modeState: {
      status: "ready",
      canWrite: true,
      legacyCommonConfigWritable: false,
      writeProtocol: "native",
      blockedReason: null,
    },
  };
  client.setQueryData(["applicationRouting", "claude"], admission);
  vi.mocked(applicationRoutingApi.get).mockResolvedValueOnce(admission as any);
  const onSubmit = vi.fn();
  const { unmount } = render(
    <QueryClientProvider client={client}>
      <ProviderForm
        appId="claude"
        providerId="existing"
        submitLabel="save"
        onSubmit={onSubmit}
        onCancel={vi.fn()}
        initialData={{
          name: "Existing",
          settingsConfig: {
            env: {
              ANTHROPIC_AUTH_TOKEN: "synthetic-key",
              ANTHROPIC_BASE_URL: "https://synthetic.invalid",
            },
          },
          meta: { commonConfigEnabled: true },
        }}
      />
    </QueryClientProvider>,
  );
  try {
    await waitFor(() =>
      expect(
        screen.getByRole("checkbox", {
          name: "claudeConfig.writeCommonConfig",
        }),
      ).toBeDisabled(),
    );
    fireEvent.submit(document.querySelector("form")!);
    await waitFor(() => expect(onSubmit).toHaveBeenCalledOnce());
    expect(onSubmit.mock.calls[0][0].meta.commonConfigEnabled).toBe(true);
  } finally {
    unmount();
    await client.cancelQueries();
    client.clear();
  }
});

it("allows explicit legacy common snippet save and revokes an open editor when the same app loses its protocol", async ({
  onTestFinished,
}) => {
  const client = createTestQueryClient();
  const legacy = {
    modeState: {
      status: "ready",
      canWrite: true,
      writeProtocol: "legacy",
      blockedReason: null,
      mode: null,
      attached: null,
    },
  };
  client.setQueryData(["applicationRouting", "claude"], legacy);
  vi.mocked(applicationRoutingApi.get).mockResolvedValue(legacy as any);
  const save = vi
    .spyOn(configApi, "setCommonConfigSnippet")
    .mockResolvedValue(undefined);
  // The runner ends ownership even when this async test body times out.
  onTestFinished(() => save.mockRestore());
  const { unmount } = render(
    <QueryClientProvider client={client}>
      <ProviderForm
        appId="claude"
        submitLabel="save"
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />
    </QueryClientProvider>,
  );
  try {
    await waitFor(() =>
      expect(
        screen.getByRole("checkbox", {
          name: "claudeConfig.writeCommonConfig",
        }),
      ).toBeEnabled(),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "claudeConfig.editCommonConfig" }),
    );
    fireEvent.change(
      screen.getAllByRole("textbox", { name: "configuration JSON" }).at(-1)!,
      { target: { value: '{"safe":"changed"}' } },
    );
    await userEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    await userEvent.click(
      screen.getByRole("button", { name: "claudeConfig.editCommonConfig" }),
    );
    await act(async () => {
      client.setQueryData(["applicationRouting", "claude"], {});
    });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "common.save" }),
      ).toBeDisabled(),
    );
    expect(save).toHaveBeenCalledOnce();
  } finally {
    await cleanupSnippetForm(unmount, client);
  }
});

it("saves Gemini env after alternating native field and raw editor changes", async () => {
  const client = createTestQueryClient();
  const legacy = {
    modeState: {
      status: "ready",
      canWrite: true,
      writeProtocol: "legacy",
      blockedReason: null,
    },
  };
  client.setQueryData(["applicationRouting", "gemini"], legacy);
  vi.mocked(applicationRoutingApi.get).mockResolvedValue(legacy as any);
  const onSubmit = vi.fn();
  const { unmount } = render(
    <QueryClientProvider client={client}>
      <ProviderForm
        appId="gemini"
        providerId="gemini-draft"
        submitLabel="save"
        onSubmit={onSubmit}
        onCancel={vi.fn()}
        initialData={{
          name: "Gemini draft",
          category: "third_party",
          settingsConfig: {
            env: {
              GEMINI_API_KEY: "synthetic-initial",
              GOOGLE_GEMINI_BASE_URL: "https://initial.example",
              GEMINI_MODEL: "initial-model",
              EXTRA: "keep",
            },
            config: { custom: true },
          },
          meta: { commonConfigEnabled: false },
        }}
      />
    </QueryClientProvider>,
  );
  try {
    const keyInput = await screen.findByDisplayValue("synthetic-initial");
    fireEvent.change(keyInput, { target: { value: " synthetic-field " } });
    const editors = await screen.findAllByRole("textbox", {
      name: "configuration JSON",
    });
    const raw = [
      "# retained until a field edit",
      "GEMINI_API_KEY=obsolete",
      "GEMINI_API_KEY=synthetic-raw",
      "GOOGLE_GEMINI_BASE_URL=https://raw.example",
      "GEMINI_MODEL=raw-model",
      "EXTRA=keep",
      "unfinished line",
    ].join("\n");
    fireEvent.change(editors[0], { target: { value: raw } });
    expect(editors[0]).toHaveValue(raw);
    await waitFor(() => expect(keyInput).toHaveValue("synthetic-raw"));
    fireEvent.change(screen.getByDisplayValue("raw-model"), {
      target: { value: " final-model " },
    });
    fireEvent.submit(document.querySelector("form")!);
    await waitFor(() => expect(onSubmit).toHaveBeenCalledOnce());
    expect(JSON.parse(onSubmit.mock.calls[0][0].settingsConfig)).toEqual({
      env: {
        GEMINI_API_KEY: "synthetic-raw",
        GOOGLE_GEMINI_BASE_URL: "https://raw.example",
        GEMINI_MODEL: "final-model",
        EXTRA: "keep",
      },
      config: { custom: true },
    });
  } finally {
    unmount();
    await client.cancelQueries();
    client.clear();
  }
});

it("keeps the next real form's save spy when the previous form's query cleanup completes late", async ({
  onTestFinished,
}) => {
  const olderClient = createTestQueryClient();
  const olderSave = vi
    .spyOn(configApi, "setCommonConfigSnippet")
    .mockResolvedValue(undefined);
  onTestFinished(() => olderSave.mockRestore());
  const older = render(
    <QueryClientProvider client={olderClient}>
      <ProviderForm
        appId="claude"
        submitLabel="save"
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />
    </QueryClientProvider>,
  );
  await screen.findByRole("button", { name: "claudeConfig.editCommonConfig" });
  let finishCancellation!: () => void;
  const cancellation = new Promise<void>((resolve) => {
    finishCancellation = resolve;
  });
  const olderCancellation = vi
    .spyOn(olderClient, "cancelQueries")
    .mockReturnValueOnce(cancellation);
  const olderCleanup = cleanupSnippetForm(older.unmount, olderClient);

  const newerClient = createTestQueryClient();
  const newerSave = vi
    .spyOn(configApi, "setCommonConfigSnippet")
    .mockResolvedValue(undefined);
  onTestFinished(() => newerSave.mockRestore());
  const legacy = {
    modeState: {
      status: "ready",
      canWrite: true,
      writeProtocol: "legacy",
      blockedReason: null,
    },
  };
  newerClient.setQueryData(["applicationRouting", "claude"], legacy);
  vi.mocked(applicationRoutingApi.get).mockResolvedValue(legacy as any);
  const newer = render(
    <QueryClientProvider client={newerClient}>
      <ProviderForm
        appId="claude"
        submitLabel="save"
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />
    </QueryClientProvider>,
  );
  try {
    await waitFor(() =>
      expect(
        screen.getByRole("checkbox", {
          name: "claudeConfig.writeCommonConfig",
        }),
      ).toBeEnabled(),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "claudeConfig.editCommonConfig" }),
    );
    fireEvent.change(
      screen.getAllByRole("textbox", { name: "configuration JSON" }).at(-1)!,
      { target: { value: '{"safe":"changed"}' } },
    );
    finishCancellation();
    await olderCleanup;
    expect(configApi.setCommonConfigSnippet).toBe(newerSave);
    await userEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(newerSave).toHaveBeenCalledWith("claude", '{"safe":"changed"}'),
    );
  } finally {
    finishCancellation();
    await olderCleanup;
    olderCancellation.mockRestore();
    olderClient.clear();
    newer.unmount();
    await newerClient.cancelQueries();
    newerClient.clear();
  }
});
