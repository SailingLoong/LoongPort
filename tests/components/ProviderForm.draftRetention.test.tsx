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
vi.mock("@/lib/api/applicationRouting", () => ({
  applicationRoutingApi: { get: vi.fn().mockResolvedValue({}) },
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

it("keeps an open Claude snippet readable when the backend cache freezes its write capability", async () => {
  const client = createTestQueryClient();
  client.setQueryData(["applicationRouting", "claude"], {});
  const save = vi
    .spyOn(configApi, "setCommonConfigSnippet")
    .mockResolvedValue(undefined);
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
    expect(screen.getByText("commonConfig.frozenHint")).toBeVisible();
    await userEvent.click(
      screen.getAllByRole("button", { name: "common.cancel" }).at(-1)!,
    );
    await waitFor(() =>
      expect(
        screen.queryByText("commonConfig.frozenHint"),
      ).not.toBeInTheDocument(),
    );
  } finally {
    unmount();
    await client.cancelQueries();
    client.clear();
    save.mockRestore();
  }
});

it("preserves a frozen provider's existing common-config flag when saving ordinary fields", async () => {
  const client = createTestQueryClient();
  const admission = {
    modeState: {
      status: "ready",
      canWrite: true,
      legacyCommonConfigWritable: false,
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
