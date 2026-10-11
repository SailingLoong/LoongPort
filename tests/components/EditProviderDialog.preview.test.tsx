import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EditorView } from "@codemirror/view";
import { toast } from "sonner";
import { forceLinting, forEachDiagnostic } from "@codemirror/lint";
import { QueryClientProvider } from "@tanstack/react-query";
import { http, HttpResponse } from "msw";
import { beforeAll, afterAll, describe, expect, it, vi } from "vitest";
import { EditProviderDialog } from "@/components/providers/EditProviderDialog";
import { useTierEditGuard } from "@/components/relay/useTierEditGuard";
import { useUpdateProviderMutation } from "@/lib/query/mutations";
import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import type { ProviderEditRequest } from "@/lib/api/providers";
import { server } from "../msw/server";
import { createTestQueryClient } from "../utils/testQueryClient";

// jsdom has no layout. Supply only the geometry used by the real CodeMirror.
const rangeRects = Object.getOwnPropertyDescriptor(
  Range.prototype,
  "getClientRects",
);
const rangeBounds = Object.getOwnPropertyDescriptor(
  Range.prototype,
  "getBoundingClientRect",
);
beforeAll(() => {
  Object.defineProperty(Range.prototype, "getClientRects", {
    configurable: true,
    value: () => [],
  });
  Object.defineProperty(Range.prototype, "getBoundingClientRect", {
    configurable: true,
    value: () => new DOMRect(),
  });
});
afterAll(() => {
  for (const [key, descriptor] of [
    ["getClientRects", rangeRects],
    ["getBoundingClientRect", rangeBounds],
  ] as const) {
    if (descriptor) Object.defineProperty(Range.prototype, key, descriptor);
    else Reflect.deleteProperty(Range.prototype, key);
  }
});

const provider: Provider = {
  id: "synthetic-preview-row",
  name: "Synthetic preview row",
  settingsConfig: {
    env: {
      ANTHROPIC_BASE_URL: "https://preview.example.invalid",
      ANTHROPIC_AUTH_TOKEN: "synthetic-preview-secret",
      ANTHROPIC_MODEL: "synthetic-model",
    },
  },
};

// Keep the real form, dialog, mutation and API adapter. Only the native IPC
// transport is supplied by the existing MSW test host.
function Editor({
  close,
  row = provider,
  app = "claude",
}: {
  close: (open: boolean) => void;
  row?: Provider;
  app?: AppId;
}) {
  const mutation = useUpdateProviderMutation(app);
  return (
    <EditProviderDialog
      open
      appId={app}
      provider={row}
      onOpenChange={close}
      onSubmit={async (payload) => {
        return await mutation.mutateAsync(payload);
      }}
    />
  );
}

describe("U02 real provider edit preview", () => {
  it("previews the real form before any provider write or success close", async () => {
    const writes: unknown[] = [];
    const previews: unknown[] = [];
    const close = vi.fn();
    server.use(
      http.post("http://tauri.local/get_settings", () =>
        HttpResponse.json({ commonConfigConfirmed: true }),
      ),
      http.post("http://tauri.local/get_application_routing", () =>
        HttpResponse.json({
          modeState: {
            status: "ready",
            canWrite: true,
            legacyCommonConfigWritable: false,
          },
        }),
      ),
      http.post("http://tauri.local/get_provider_edit_settings", () =>
        HttpResponse.json({
          settingsConfig: provider.settingsConfig,
          modeState: { status: "ready", canWrite: true },
        }),
      ),
      http.post(
        "http://tauri.local/preview_provider_edit",
        async ({ request }) => {
          const input = (await request.json()) as {
            app: string;
            provider: Provider;
            requestId: string;
          };
          previews.push(input);
          return HttpResponse.json({
            app: input.app,
            request: {
              id: input.requestId,
              providerId: input.provider.id,
              draftDigest: "a".repeat(64),
              revision: "b".repeat(64),
            },
            status: "ready",
            action: "saveOnly",
            fields: ["metadata"],
            files: [],
            preserves: ["sharedSettings"],
          });
        },
      ),
      http.post("http://tauri.local/update_provider", async ({ request }) => {
        writes.push(await request.json());
        return HttpResponse.json(true);
      }),
    );
    const client = createTestQueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor close={close} />
      </QueryClientProvider>,
    );
    try {
      await screen.findByDisplayValue(provider.name);
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        ).toBeEnabled(),
      );
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await waitFor(() => {
        expect(
          screen.getByRole("tab", { name: "provider.preview.fields" }),
        ).toBeVisible();
        expect(
          screen.getByRole("tab", { name: "provider.preview.files" }),
        ).toBeVisible();
      });
      await screen.findByRole("button", { name: "provider.preview.saveOnly" });
      expect(previews).toHaveLength(1);
      expect(writes).toEqual([]);
      expect(close).not.toHaveBeenCalled();
    } finally {
      console.info(
        "U02 safe observation",
        JSON.stringify({
          updateCalls: writes.length,
          closeCalls: close.mock.calls.length,
        }),
      );
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });
});

describe("U02 editing source lifetime", () => {
  it("waits for original edit settings before exposing a writable draft", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    server.use(
      http.post("http://tauri.local/get_settings", () =>
        HttpResponse.json({ commonConfigConfirmed: true }),
      ),
      http.post("http://tauri.local/get_application_routing", () =>
        HttpResponse.json({
          modeState: {
            status: "ready",
            canWrite: true,
            legacyCommonConfigWritable: false,
          },
        }),
      ),
      http.post("http://tauri.local/get_provider_edit_settings", async () => {
        await held;
        return HttpResponse.json({
          settingsConfig: provider.settingsConfig,
          modeState: { status: "ready", canWrite: true },
        });
      }),
    );
    const client = createTestQueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor close={vi.fn()} />
      </QueryClientProvider>,
    );
    try {
      expect(screen.queryByDisplayValue(provider.name)).not.toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "common.save" }),
      ).toBeDisabled();
      await act(async () => {
        release();
        await held;
      });
      await screen.findByDisplayValue(provider.name);
    } finally {
      release();
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });

  it("does not silently use database settings after the original live read fails", async () => {
    server.use(
      http.post("http://tauri.local/get_settings", () =>
        HttpResponse.json({ commonConfigConfirmed: true }),
      ),
      http.post("http://tauri.local/get_application_routing", () =>
        HttpResponse.json({
          modeState: {
            status: "ready",
            canWrite: true,
            legacyCommonConfigWritable: false,
          },
        }),
      ),
      http.post(
        "http://tauri.local/get_provider_edit_settings",
        () => new HttpResponse("synthetic source unavailable", { status: 500 }),
      ),
    );
    const client = createTestQueryClient();
    const close = vi.fn();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor close={close} />
      </QueryClientProvider>,
    );
    try {
      await screen.findByText("provider.preview.sourceUnavailable");
      expect(screen.queryByDisplayValue(provider.name)).not.toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "common.save" }),
      ).toBeDisabled();
      expect(close).not.toHaveBeenCalled();
    } finally {
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });
});

function controlledHost(
  row: Provider,
  options: {
    lostResponse?: boolean;
    previewHold?: Promise<void>;
    confirmHold?: Promise<void>;
  } = {},
) {
  const previews: Array<{
    app: AppId;
    provider: Provider;
    requestId: string;
    deleteCredential: boolean;
  }> = [];
  const confirms: Array<{
    app: AppId;
    provider: Provider;
    request: ProviderEditRequest;
    deleteCredential: boolean;
  }> = [];
  const queries: Array<{ app: AppId; request: ProviderEditRequest }> = [];
  const legacyWrites = vi.fn();
  server.use(
    http.post("http://tauri.local/get_settings", () =>
      HttpResponse.json({ commonConfigConfirmed: true }),
    ),
    http.post("http://tauri.local/get_application_routing", () =>
      HttpResponse.json({
        modeState: {
          status: "ready",
          canWrite: true,
          legacyCommonConfigWritable: false,
        },
      }),
    ),
    http.post("http://tauri.local/get_provider_edit_settings", () =>
      HttpResponse.json({
        settingsConfig: row.settingsConfig,
        modeState: { status: "ready", canWrite: true },
      }),
    ),
    http.post(
      "http://tauri.local/preview_provider_edit",
      async ({ request }) => {
        const input = (await request.json()) as (typeof previews)[number];
        previews.push(input);
        await options.previewHold;
        return HttpResponse.json({
          app: input.app,
          request: {
            id: input.requestId,
            providerId: input.provider.id,
            draftDigest: "a".repeat(64),
            revision: "b".repeat(64),
          },
          status: "ready",
          action: "saveOnly",
          fields: ["metadata"],
          files: [],
          preserves: ["sharedSettings"],
        });
      },
    ),
    http.post(
      "http://tauri.local/confirm_provider_edit",
      async ({ request }) => {
        const input = (await request.json()) as (typeof confirms)[number];
        confirms.push(input);
        await options.confirmHold;
        return options.lostResponse
          ? new HttpResponse("synthetic-private-error-fragment", {
              status: 500,
            })
          : HttpResponse.json({
              app: input.app,
              request: input.request,
              status: "completed",
            });
      },
    ),
    http.post("http://tauri.local/query_provider_edit", async ({ request }) => {
      const input = (await request.json()) as (typeof queries)[number];
      queries.push(input);
      return HttpResponse.json({
        app: input.app,
        request: input.request,
        status: "completed",
      });
    }),
    http.post("http://tauri.local/update_provider", () => {
      legacyWrites();
      return HttpResponse.json(true);
    }),
  );
  return { previews, confirms, queries, legacyWrites };
}

const appRows: Array<[AppId, Provider]> = [
  ["claude", provider],
  [
    "codex",
    {
      ...provider,
      category: "custom",
      settingsConfig: {
        auth: { OPENAI_API_KEY: "synthetic-preview-secret" },
        config:
          'model_provider = "synthetic"\nmodel = "synthetic-model"\n[model_providers.synthetic]\nname = "Synthetic"\nbase_url = "https://preview.example.invalid/v1"\nwire_api = "responses"\n',
      },
    },
  ],
  [
    "gemini",
    {
      ...provider,
      category: "custom",
      settingsConfig: {
        env: {
          GEMINI_API_KEY: "synthetic-preview-secret",
          GOOGLE_GEMINI_BASE_URL: "https://preview.example.invalid",
          GEMINI_MODEL: "synthetic-model",
        },
        config: {},
      },
    },
  ],
  [
    "grokbuild",
    {
      ...provider,
      category: "custom",
      settingsConfig: {
        config:
          '[models]\ndefault = "synthetic"\n[model.synthetic]\nmodel = "synthetic-model"\nname = "Synthetic"\nbase_url = "https://preview.example.invalid/v1"\napi_key = "synthetic-preview-secret"\napi_backend = "responses"\ncontext_window = 500000\n',
      },
    },
  ],
];

describe("U02 real four-app form confirmation", () => {
  it.each(appRows)(
    "%s previews then explicitly confirms one original request",
    async (app, row) => {
      const host = controlledHost(row);
      const close = vi.fn();
      const client = createTestQueryClient();
      const view = render(
        <QueryClientProvider client={client}>
          <Editor row={row} app={app} close={close} />
        </QueryClientProvider>,
      );
      try {
        await screen.findByDisplayValue(row.name);
        await userEvent.click(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        );
        const confirm = await screen.findByRole("button", {
          name: "provider.preview.saveOnly",
        });
        expect(host.previews).toHaveLength(1);
        expect(host.confirms).toHaveLength(0);
        expect(close).not.toHaveBeenCalled();
        await userEvent.click(confirm);
        await waitFor(() => expect(close).toHaveBeenCalledWith(false));
        expect(host.confirms).toHaveLength(1);
        expect(host.confirms[0].request.id).toBe(host.previews[0].requestId);
        expect(host.confirms[0].provider).toEqual(host.previews[0].provider);
        expect(host.legacyWrites).not.toHaveBeenCalled();
      } finally {
        view.unmount();
        await client.cancelQueries();
        client.clear();
      }
    },
  );

  it.each(
    appRows.flatMap(([app, row]) => [
      [app, row, false] as const,
      [app, row, true] as const,
    ]),
  )(
    "%s preserves blank credential intent with explicit deletion=%s",
    async (app, row, deleteCredential) => {
      const host = controlledHost(row);
      const close = vi.fn();
      const client = createTestQueryClient();
      const view = render(
        <QueryClientProvider client={client}>
          <Editor row={row} app={app} close={close} />
        </QueryClientProvider>,
      );
      try {
        await screen.findByDisplayValue(row.name);
        const key = await screen.findByDisplayValue("synthetic-preview-secret");
        fireEvent.change(key, { target: { value: "" } });
        if (deleteCredential) {
          await userEvent.click(
            screen.getByRole("checkbox", {
              name: /^provider\.preview\.deleteCredential/,
            }),
          );
        }
        await userEvent.click(
          screen.getByRole("button", {
            name: "provider.preview.preview",
          }),
        );
        await waitFor(() => expect(host.previews.length).toBe(1));
        expect(host.previews[0].deleteCredential).toBe(deleteCredential);
        expect(
          JSON.stringify(host.previews[0].provider.settingsConfig).includes(
            "synthetic-preview-secret",
          ),
        ).toBe(false);
        expect(host.confirms).toHaveLength(0);
        await userEvent.click(
          await screen.findByRole("button", {
            name: "provider.preview.saveOnly",
          }),
        );
        await waitFor(() => expect(close).toHaveBeenCalledWith(false));
        expect(host.confirms[0].deleteCredential).toBe(deleteCredential);
        expect(host.confirms[0].provider).toEqual(host.previews[0].provider);
        expect(host.legacyWrites).not.toHaveBeenCalled();
      } finally {
        view.unmount();
        await client.cancelQueries();
        client.clear();
      }
    },
  );

  it("retires a preview when the real draft changes and requires a fresh explicit preview", async () => {
    const host = controlledHost(provider);
    const close = vi.fn();
    const client = createTestQueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor close={close} />
      </QueryClientProvider>,
    );
    try {
      const name = await screen.findByDisplayValue(provider.name);
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await screen.findByRole("button", { name: "provider.preview.saveOnly" });
      fireEvent.change(name, { target: { value: "New synthetic draft" } });
      await waitFor(() =>
        expect(
          screen.queryByRole("button", { name: "provider.preview.saveOnly" }),
        ).not.toBeInTheDocument(),
      );
      expect(host.confirms).toHaveLength(0);
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await userEvent.click(
        await screen.findByRole("button", {
          name: "provider.preview.saveOnly",
        }),
      );
      await waitFor(() => expect(host.confirms).toHaveLength(1));
      expect(host.previews).toHaveLength(2);
      expect(host.confirms[0].provider.name).toBe("New synthetic draft");
      expect(host.confirms[0].request.id).toBe(host.previews[1].requestId);
    } finally {
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });

  it("queries the submitted request after a lost response and preserves a newer draft", async () => {
    const host = controlledHost(provider, { lostResponse: true });
    const close = vi.fn();
    const client = createTestQueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor close={close} />
      </QueryClientProvider>,
    );
    try {
      const name = await screen.findByDisplayValue(provider.name);
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await userEvent.click(
        await screen.findByRole("button", {
          name: "provider.preview.saveOnly",
        }),
      );
      await screen.findByText("provider.preview.result.unknown");
      expect(host.confirms).toHaveLength(1);
      expect(host.queries).toHaveLength(0);
      expect(close).not.toHaveBeenCalled();
      fireEvent.change(name, { target: { value: "Newer unsaved draft" } });
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.queryOriginal" }),
      );
      await screen.findByText("provider.preview.savedPreviousDraft");
      expect(host.queries).toHaveLength(1);
      expect(host.queries[0].request).toEqual(host.confirms[0].request);
      expect(host.confirms).toHaveLength(1);
      expect(name).toHaveValue("Newer unsaved draft");
      expect(close).not.toHaveBeenCalled();
      expect(
        screen.queryByText("synthetic-private-error-fragment"),
      ).not.toBeInTheDocument();
    } finally {
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });
});

describe("U02 reopened original operation", () => {
  it("reads the original request from its owner and queries without repeating the write", async () => {
    const host = controlledHost(provider);
    const original = {
      id: "6bb14c32-c8ad-444b-a54b-d38be59d195f",
      providerId: provider.id,
      draftDigest: "c".repeat(64),
      revision: "d".repeat(64),
    };
    server.use(
      http.post("http://tauri.local/get_provider_edit_settings", () =>
        HttpResponse.json({
          settingsConfig: provider.settingsConfig,
          modeState: { status: "pending", canWrite: false },
          originalSave: { app: "claude", request: original, status: "unknown" },
        }),
      ),
    );
    const client = createTestQueryClient();
    const close = vi.fn();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor close={close} />
      </QueryClientProvider>,
    );
    try {
      await screen.findByDisplayValue(provider.name);
      const query = await screen.findByRole("button", {
        name: "provider.preview.queryOriginal",
      });
      await userEvent.click(query);
      await waitFor(() => expect(host.queries.length).toBe(1));
      expect(host.queries[0]).toEqual({ app: "claude", request: original });
      await screen.findByText("provider.preview.savedPreviousDraft");
      expect(close).not.toHaveBeenCalled();
      expect(host.confirms).toHaveLength(0);
      expect(host.previews).toHaveLength(0);
      expect(host.legacyWrites).not.toHaveBeenCalled();
    } finally {
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });
});

describe("U02 raw draft preservation", () => {
  it("invalid Grok TOML retires preview and cannot be silently rebuilt into another draft", async () => {
    const row = appRows.find(([app]) => app === "grokbuild")![1];
    const host = controlledHost(row);
    const client = createTestQueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor row={row} app="grokbuild" close={vi.fn()} />
      </QueryClientProvider>,
    );
    try {
      await screen.findByDisplayValue(row.name);
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await screen.findByRole("button", { name: "provider.preview.saveOnly" });
      const element = document.querySelector<HTMLElement>(".cm-editor")!;
      const editor = EditorView.findFromDOM(element)!;
      expect(!!editor).toBe(true);
      act(() =>
        editor.dispatch({
          changes: { from: 0, to: editor.state.doc.length, insert: "[models" },
        }),
      );
      await waitFor(() =>
        expect(
          screen.queryByRole("button", { name: "provider.preview.saveOnly" }),
        ).not.toBeInTheDocument(),
      );
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        ).toBeEnabled(),
      );
      expect(host.previews.length).toBe(1);
      expect(host.confirms).toHaveLength(0);
    } finally {
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });
});

function TierEditor({ onSaved }: { onSaved: () => Promise<void> }) {
  const owner = useTierEditGuard("claude", onSaved);
  return (
    <>
      <button
        onClick={() =>
          owner.requestEdit({
            providerId: provider.id,
            displayName: provider.name,
            isCurrent: false,
            kind: "vendor",
          })
        }
      >
        open-managed-editor
      </button>
      {owner.editDialogs}
    </>
  );
}

describe("U02 original account and session callbacks", () => {
  it.each([false, true])(
    "account entry preserves its warning and result cache owner after lostResponse=%s",
    async (lostResponse) => {
      const host = controlledHost(provider, { lostResponse });
      server.use(
        http.post("http://tauri.local/get_providers", () =>
          HttpResponse.json({ [provider.id]: provider }),
        ),
      );
      const onSaved = vi.fn().mockResolvedValue(undefined);
      const client = createTestQueryClient();
      const view = render(
        <QueryClientProvider client={client}>
          <TierEditor onSaved={onSaved} />
        </QueryClientProvider>,
      );
      try {
        await userEvent.click(
          screen.getByRole("button", { name: "open-managed-editor" }),
        );
        await screen.findByText("loongport.vendor.editConfirmMessage");
        await userEvent.click(
          screen.getByRole("button", {
            name: "loongport.tier.editConfirmButton",
          }),
        );
        await screen.findByDisplayValue(provider.name);
        await userEvent.click(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        );
        expect(onSaved).not.toHaveBeenCalled();
        await userEvent.click(
          await screen.findByRole("button", {
            name: "provider.preview.saveOnly",
          }),
        );
        if (lostResponse) {
          const query = await screen.findByRole("button", {
            name: "provider.preview.queryOriginal",
          });
          expect(onSaved).not.toHaveBeenCalled();
          await userEvent.click(query);
        }
        await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
        await waitFor(() =>
          expect(
            screen.queryByDisplayValue(provider.name),
          ).not.toBeInTheDocument(),
        );
        expect(host.confirms).toHaveLength(1);
        expect(host.queries).toHaveLength(lostResponse ? 1 : 0);
        expect(host.legacyWrites).not.toHaveBeenCalled();
      } finally {
        view.unmount();
        await client.cancelQueries();
        client.clear();
      }
    },
  );

  it("retires a late preview after a newer draft without a second automatic request", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const host = controlledHost(provider, { previewHold: held });
    const client = createTestQueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor close={vi.fn()} />
      </QueryClientProvider>,
    );
    try {
      const name = await screen.findByDisplayValue(provider.name);
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await waitFor(() => expect(host.previews.length).toBe(1));
      fireEvent.change(name, { target: { value: "Later draft" } });
      await act(async () => {
        release();
      });
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        ).toBeEnabled(),
      );
      expect(
        screen.queryByRole("button", { name: "provider.preview.saveOnly" }),
      ).not.toBeInTheDocument();
      expect(host.previews).toHaveLength(1);
      expect(host.confirms).toHaveLength(0);
    } finally {
      release();
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });

  it("a late save response cannot close a different provider session", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const host = controlledHost(provider, { confirmHold: held });
    const client = createTestQueryClient();
    const close = vi.fn();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor close={close} />
      </QueryClientProvider>,
    );
    try {
      await screen.findByDisplayValue(provider.name);
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await userEvent.click(
        await screen.findByRole("button", {
          name: "provider.preview.saveOnly",
        }),
      );
      await waitFor(() => expect(host.confirms.length).toBe(1));
      const next = {
        ...provider,
        id: "another-synthetic-row",
        name: "Another row",
      };
      view.rerender(
        <QueryClientProvider client={client}>
          <Editor row={next} close={close} />
        </QueryClientProvider>,
      );
      await screen.findByDisplayValue(next.name);
      await act(async () => {
        release();
      });
      await waitFor(() => expect(client.isMutating()).toBe(0));
      expect(close).not.toHaveBeenCalled();
      expect(screen.getByDisplayValue(next.name)).toBeInTheDocument();
      expect(host.confirms).toHaveLength(1);
    } finally {
      release();
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });
});

describe("U02 resolver and duplicate submissions", () => {
  it("does not combine an accepted old soft warning with a newer draft", async () => {
    const host = controlledHost(provider);
    const client = createTestQueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor close={vi.fn()} />
      </QueryClientProvider>,
    );
    try {
      const name = await screen.findByDisplayValue(provider.name);
      fireEvent.change(name, { target: { value: "" } });
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      const accept = await screen.findByRole("button", { name: "仍要保存" });
      fireEvent.change(name, { target: { value: "Newer valid draft" } });
      await userEvent.click(accept);
      await waitFor(() =>
        expect(
          screen.queryByRole("button", { name: "仍要保存" }),
        ).not.toBeInTheDocument(),
      );
      expect(host.previews).toHaveLength(0);
      expect(host.confirms).toHaveLength(0);
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await waitFor(() => expect(host.previews.length).toBe(1));
      expect(host.previews[0].provider.name).toBe("Newer valid draft");
    } finally {
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });

  it("two confirmation clicks submit only one original write", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const host = controlledHost(provider, { confirmHold: held });
    const client = createTestQueryClient();
    const close = vi.fn();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor close={close} />
      </QueryClientProvider>,
    );
    try {
      await screen.findByDisplayValue(provider.name);
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      const confirm = await screen.findByRole("button", {
        name: "provider.preview.saveOnly",
      });
      fireEvent.click(confirm);
      fireEvent.click(confirm);
      await waitFor(() => expect(host.confirms.length).toBe(1));
      await act(async () => {
        release();
      });
      await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
      expect(host.confirms).toHaveLength(1);
      expect(host.legacyWrites).not.toHaveBeenCalled();
    } finally {
      release();
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  });
});

describe("U02 review: raw source and numeric draft binding", () => {
  it.each(["codex-auth", "gemini-config", "codex-toml"] as const)(
    "never previews an invalid %s as an older or normalized draft",
    async (kind) => {
      const app = kind === "gemini-config" ? "gemini" : "codex";
      const row = appRows.find(([name]) => name === app)![1];
      const host = controlledHost(row);
      const client = createTestQueryClient();
      const view = render(
        <QueryClientProvider client={client}>
          <Editor row={row} app={app} close={vi.fn()} />
        </QueryClientProvider>,
      );
      try {
        await screen.findByDisplayValue(row.name);
        const editors = [
          ...document.querySelectorAll<HTMLElement>(".cm-editor"),
        ].map((element) => EditorView.findFromDOM(element)!);
        const editor = editors.find((candidate) =>
          kind === "codex-auth"
            ? candidate.state.doc.toString().includes("OPENAI_API_KEY")
            : kind === "codex-toml"
              ? candidate.state.doc.toString().includes("wire_api")
              : candidate.state.doc.toString().trim() === "{}",
        )!;
        expect(!!editor).toBe(true);
        const invalid =
          kind === "codex-toml"
            ? editor.state.doc
                .toString()
                .replace('wire_api = "responses"', 'wire_api = "\\q"')
            : "{";
        act(() =>
          editor.dispatch({
            changes: { from: 0, to: editor.state.doc.length, insert: invalid },
          }),
        );
        await userEvent.click(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        );
        await waitFor(() =>
          expect(
            screen.getByRole("button", { name: "provider.preview.preview" }),
          ).toBeEnabled(),
        );
        expect(host.previews.length).toBe(0);
        expect(host.confirms).toHaveLength(0);
      } finally {
        view.unmount();
        await client.cancelQueries();
        client.clear();
      }
    },
  );

  it.each(["800000", ""])(
    "retires the preview immediately for an auto-compaction raw input of %s",
    async (value) => {
      const old = appRows.find(([app]) => app === "codex")![1];
      const row = {
        ...old,
        settingsConfig: {
          ...old.settingsConfig,
          config:
            "model_context_window = 1050000\nmodel_auto_compact_token_limit = 900000\n" +
            old.settingsConfig.config,
        },
      };
      const host = controlledHost(row);
      const client = createTestQueryClient();
      const view = render(
        <QueryClientProvider client={client}>
          <Editor row={row} app="codex" close={vi.fn()} />
        </QueryClientProvider>,
      );
      try {
        await screen.findByDisplayValue(row.name);
        await userEvent.click(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        );
        await screen.findByRole("button", {
          name: "provider.preview.saveOnly",
        });
        fireEvent.change(
          screen.getByLabelText(/codexConfig.autoCompactLimit/),
          { target: { value } },
        );
        expect(
          !!screen.queryByRole("button", { name: "provider.preview.saveOnly" }),
        ).toBe(false);
        expect(host.confirms).toHaveLength(0);
        await userEvent.click(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        );
        await waitFor(() =>
          expect(
            screen.getByRole("button", { name: "provider.preview.preview" }),
          ).toBeEnabled(),
        );
        if (value) {
          await waitFor(() => expect(host.previews.length).toBe(2));
          expect(host.previews[1].provider.settingsConfig.config).toContain(
            "model_auto_compact_token_limit = 800000",
          );
        } else {
          expect(host.previews.length).toBe(1);
        }
      } finally {
        view.unmount();
        await client.cancelQueries();
        client.clear();
      }
    },
  );
});

it("does not repeat a synthetic credential line in the controlled TOML parser diagnostic", async () => {
  const row = appRows.find(([app]) => app === "codex")![1];
  controlledHost(row);
  const client = createTestQueryClient();
  const view = render(
    <QueryClientProvider client={client}>
      <Editor row={row} app="codex" close={vi.fn()} />
    </QueryClientProvider>,
  );
  try {
    await screen.findByDisplayValue(row.name);
    const editors = [
      ...document.querySelectorAll<HTMLElement>(".cm-editor"),
    ].map((element) => EditorView.findFromDOM(element)!);
    const editor = editors.find((candidate) =>
      candidate.state.doc.toString().includes("wire_api"),
    )!;
    act(() =>
      editor.dispatch({
        changes: {
          from: 0,
          to: editor.state.doc.length,
          insert:
            'experimental_bearer_token = "synthetic-secret-canary"\n[invalid\n',
        },
      }),
    );
    await waitFor(() =>
      expect(
        document.querySelectorAll("p.text-red-500, p.text-destructive").length,
      ).toBeGreaterThan(0),
    );
    const diagnostics = [
      ...document.querySelectorAll("p.text-red-500, p.text-destructive"),
    ]
      .map((element) => element.textContent)
      .join("\n");
    expect(diagnostics.includes("synthetic-secret-canary")).toBe(false);
  } finally {
    view.unmount();
    await client.cancelQueries();
    client.clear();
  }
});

it.each(["codexDefaultModel", "codexBaseUrl"])(
  "keeps an unfinished compact input when the unrelated %s field changes",
  async (field) => {
    const old = appRows.find(([app]) => app === "codex")![1];
    const row = {
      ...old,
      settingsConfig: {
        ...old.settingsConfig,
        config:
          "model_context_window = 1050000\nmodel_auto_compact_token_limit = 900000\n" +
          old.settingsConfig.config,
      },
    };
    const host = controlledHost(row);
    const client = createTestQueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor row={row} app="codex" close={vi.fn()} />
      </QueryClientProvider>,
    );
    try {
      await screen.findByDisplayValue(row.name);
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await screen.findByRole("button", { name: "provider.preview.saveOnly" });
      const compact = screen.getByLabelText(/codexConfig.autoCompactLimit/);
      fireEvent.change(compact, { target: { value: "" } });
      fireEvent.change(document.getElementById(field)!, {
        target: {
          value:
            field === "codexBaseUrl"
              ? "https://changed.example.invalid/v1"
              : "changed-model",
        },
      });
      expect(
        (
          screen.getByLabelText(
            /codexConfig.autoCompactLimit/,
          ) as HTMLInputElement
        ).value,
      ).toBe("");
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        ).toBeEnabled(),
      );
      expect(host.previews.length).toBe(1);
      expect(host.confirms).toHaveLength(0);
    } finally {
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  },
);

it.each(["claude", "codex", "gemini"] as const)(
  "keeps a synthetic secret out of %s JSON linter diagnostics",
  async (app) => {
    const row = appRows.find(([name]) => name === app)![1];
    controlledHost(row);
    const errors = vi.spyOn(toast, "error");
    const client = createTestQueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Editor row={row} app={app} close={vi.fn()} />
      </QueryClientProvider>,
    );
    try {
      await screen.findByDisplayValue(row.name);
      const editors = [
        ...document.querySelectorAll<HTMLElement>(".cm-editor"),
      ].map((element) => EditorView.findFromDOM(element)!);
      const editor = editors.find((candidate) =>
        app === "claude"
          ? candidate.state.doc.toString().includes("ANTHROPIC_AUTH_TOKEN")
          : app === "codex"
            ? candidate.state.doc.toString().includes("OPENAI_API_KEY")
            : candidate.state.doc.toString().trim() === "{}",
      )!;
      act(() => {
        editor.dispatch({
          changes: {
            from: 0,
            to: editor.state.doc.length,
            insert: "CANARY123",
          },
        });
        forceLinting(editor);
      });
      let diagnostics: string[] = [];
      await waitFor(() => {
        diagnostics = [];
        forEachDiagnostic(editor.state, (diagnostic) =>
          diagnostics.push(diagnostic.message),
        );
        expect(diagnostics.length).toBeGreaterThan(0);
      });
      const linterLeaked = diagnostics.some((message) =>
        message.includes("CANARY123"),
      );
      await userEvent.click(screen.getByRole("button", { name: "格式化" }));
      const formatterLeaked = JSON.stringify(errors.mock.calls).includes(
        "CANARY123",
      );
      let resolverLeaked = false;
      if (app === "claude") {
        await userEvent.click(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        );
        await waitFor(() =>
          expect(document.querySelector("p.text-destructive")).not.toBeNull(),
        );
        resolverLeaked = [
          ...document.querySelectorAll("p.text-destructive"),
        ].some((element) => element.textContent?.includes("CANARY123"));
      }
      expect({ linterLeaked, formatterLeaked, resolverLeaked }).toEqual({
        linterLeaked: false,
        formatterLeaked: false,
        resolverLeaked: false,
      });
    } finally {
      errors.mockRestore();
      view.unmount();
      await client.cancelQueries();
      client.clear();
    }
  },
);

// R5 uses the real U02 editor/mutation/API path; MSW supplies only native facts.
type CatalogDecision = { choice: "keep" | "manage"; revision: string };
const catalogRevision = "c".repeat(64);
const externalPointer = "/synthetic/external-models.json";
const managedPointer = "/synthetic/loongport-models.json";
function catalogHost(
  options: {
    ownership?: "unclaimedExternal" | "configuredExternal" | "managed" | "none";
    canManage?: boolean;
    canRestore?: boolean;
    manageResponse?:
      | "blocked"
      | "error"
      | "missingCatalog"
      | "wrongChoice"
      | "staleWithoutCatalog"
      | "wrongRevision";
    manageHold?: Promise<void>;
    confirmStatus?: "completed" | "stale" | "unknown";
  } = {},
) {
  const row = appRows.find(([app]) => app === "codex")![1];
  const original = controlledHost(row);
  const previews: Array<{
    app: AppId;
    provider: Provider;
    requestId: string;
    catalogDecision?: CatalogDecision;
  }> = [];
  const confirms: Array<{
    app: AppId;
    provider: Provider;
    request: ProviderEditRequest;
    catalogDecision?: CatalogDecision;
  }> = [];
  server.use(
    http.post(
      "http://tauri.local/preview_provider_edit",
      async ({ request }) => {
        const input = (await request.json()) as (typeof previews)[number];
        previews.push(input);
        const managing = input.catalogDecision?.choice === "manage";
        if (managing) await options.manageHold;
        if (managing && options.manageResponse === "error") {
          return new HttpResponse("synthetic failed re-preview", {
            status: 500,
          });
        }
        return HttpResponse.json({
          app: input.app,
          request: {
            id: input.requestId,
            providerId: input.provider.id,
            draftDigest: "a".repeat(64),
            revision: managing ? "d".repeat(64) : "b".repeat(64),
          },
          status:
            managing &&
            ["blocked", "staleWithoutCatalog"].includes(
              options.manageResponse ?? "",
            )
              ? "blocked"
              : "ready",
          code:
            managing &&
            ["blocked", "staleWithoutCatalog"].includes(
              options.manageResponse ?? "",
            )
              ? "sourceChanged"
              : undefined,
          action: "saveAndApply",
          fields: ["models"],
          files: [
            { role: "codexConfig", change: managing ? "write" : "unchanged" },
            ...(managing ? [{ role: "codexCatalog", change: "write" }] : []),
          ],
          preserves: ["externalCatalog"],
          catalog:
            managing &&
            ["missingCatalog", "staleWithoutCatalog"].includes(
              options.manageResponse ?? "",
            )
              ? undefined
              : {
                  ownership: options.ownership ?? "unclaimedExternal",
                  canManage: options.canManage ?? true,
                  ...(options.canRestore !== undefined
                    ? { canRestore: options.canRestore }
                    : {}),
                  choice:
                    managing && options.manageResponse !== "wrongChoice"
                      ? "manage"
                      : "keep",
                  revision:
                    managing && options.manageResponse === "wrongRevision"
                      ? "e".repeat(64)
                      : catalogRevision,
                  beforePointer: externalPointer,
                  afterPointer: managing ? managedPointer : externalPointer,
                },
        });
      },
    ),
    http.post(
      "http://tauri.local/confirm_provider_edit",
      async ({ request }) => {
        const input = (await request.json()) as (typeof confirms)[number];
        confirms.push(input);
        return HttpResponse.json({
          app: input.app,
          request: input.request,
          status: options.confirmStatus ?? "completed",
        });
      },
    ),
  );
  const client = createTestQueryClient();
  const close = vi.fn();
  const view = render(
    <QueryClientProvider client={client}>
      <Editor row={row} app="codex" close={close} />
    </QueryClientProvider>,
  );
  return {
    ...original,
    previews,
    confirms,
    row,
    close,
    client,
    view,
    cleanup: async () => {
      view.unmount();
      await client.cancelQueries();
      client.clear();
    },
  };
}
async function catalogPreview() {
  await screen.findByDisplayValue(provider.name);
  await userEvent.click(
    screen.getByRole("button", { name: "provider.preview.preview" }),
  );
  await screen.findByRole("button", { name: "provider.preview.saveAndApply" });
}
async function chooseManagedCatalog() {
  await userEvent.click(
    screen.getByRole("radio", { name: "provider.preview.catalog.manage" }),
  );
  await userEvent.click(
    screen.getByRole("button", {
      name: "provider.preview.catalog.confirmChoice",
    }),
  );
}

describe("R5 catalog choice in the real editor", () => {
  it("keeps the external catalog on final confirmation without adding a decision", async () => {
    const host = catalogHost();
    try {
      await catalogPreview();
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.saveAndApply" }),
      );
      await waitFor(() => expect(host.confirms).toHaveLength(1));
      expect(host.confirms[0]).not.toHaveProperty("catalogDecision");
      expect(host.confirms[0].request.id).toBe(host.previews[0].requestId);
      expect(host.legacyWrites).not.toHaveBeenCalled();
    } finally {
      await host.cleanup();
    }
  });

  it("returns to keep through a new preview with the same catalog revision", async () => {
    const host = catalogHost();
    try {
      await catalogPreview();
      await chooseManagedCatalog();
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.saveAndApply" }),
        ).toBeEnabled(),
      );
      await userEvent.click(
        screen.getByRole("radio", { name: "provider.preview.catalog.keep" }),
      );
      expect(
        screen.getByRole("button", { name: "provider.preview.saveAndApply" }),
      ).toBeDisabled();
      await userEvent.click(
        screen.getByRole("button", {
          name: "provider.preview.catalog.confirmChoice",
        }),
      );
      await waitFor(() => expect(host.previews).toHaveLength(3));
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.saveAndApply" }),
        ).toBeEnabled(),
      );
      expect(host.previews[2].catalogDecision).toEqual({
        choice: "keep",
        revision: catalogRevision,
      });
      expect(host.confirms).toHaveLength(0);
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.saveAndApply" }),
      );
      await waitFor(() => expect(host.confirms).toHaveLength(1));
      expect(host.confirms[0].catalogDecision).toEqual(
        host.previews[2].catalogDecision,
      );
      expect(host.confirms[0].request.id).toBe(host.previews[2].requestId);
    } finally {
      await host.cleanup();
    }
  });

  it("shows the native source-changed blocker even when it returns no catalog", async () => {
    const host = catalogHost({ manageResponse: "staleWithoutCatalog" });
    try {
      await catalogPreview();
      await chooseManagedCatalog();
      await screen.findByText("provider.preview.sourceChanged");
      const confirm = screen.queryByRole("button", {
        name: "provider.preview.saveAndApply",
      });
      expect(confirm === null || confirm.hasAttribute("disabled")).toBe(true);
      expect(host.confirms).toHaveLength(0);
    } finally {
      await host.cleanup();
    }
  });

  it("defaults to keeping the external catalog and previews management before one final confirmed write", async () => {
    const host = catalogHost();
    try {
      await screen.findByDisplayValue(host.row.name);
      expect(host.previews).toHaveLength(0);
      expect(
        screen.queryByRole("radio", {
          name: "provider.preview.catalog.manage",
        }),
      ).not.toBeInTheDocument();
      await catalogPreview();
      expect(host.previews[0]).not.toHaveProperty("catalogDecision");
      expect(
        screen.getByRole("radio", { name: "provider.preview.catalog.keep" }),
      ).toBeChecked();
      expect(
        screen.getByText("provider.preview.catalog.keepHint"),
      ).toBeVisible();
      await userEvent.click(
        screen.getByRole("radio", { name: "provider.preview.catalog.manage" }),
      );
      expect(
        screen.getByRole("button", { name: "provider.preview.saveAndApply" }),
      ).toBeDisabled();
      expect(host.previews).toHaveLength(1);
      expect(host.confirms).toHaveLength(0);
      await userEvent.click(
        screen.getByRole("button", {
          name: "provider.preview.catalog.confirmChoice",
        }),
      );
      await waitFor(() => expect(host.previews).toHaveLength(2));
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.saveAndApply" }),
        ).toBeEnabled(),
      );
      expect(host.previews[1].catalogDecision).toEqual({
        choice: "manage",
        revision: catalogRevision,
      });
      expect(host.previews[1].provider).toEqual(host.previews[0].provider);
      expect(host.confirms).toHaveLength(0);
      await userEvent.click(
        screen.getByRole("tab", { name: "provider.preview.files" }),
      );
      expect(screen.getByText(externalPointer)).toBeVisible();
      expect(screen.getByText(managedPointer)).toBeVisible();
      expect(
        screen.getByText(/provider\.preview\.file\.codexCatalog/),
      ).toBeVisible();
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.saveAndApply" }),
      );
      await waitFor(() => expect(host.close).toHaveBeenCalledWith(false));
      expect(host.confirms).toHaveLength(1);
      expect(host.confirms[0].catalogDecision).toEqual(
        host.previews[1].catalogDecision,
      );
      expect(host.confirms[0].provider).toEqual(host.previews[1].provider);
      expect(host.confirms[0].request).toEqual({
        id: host.previews[1].requestId,
        providerId: host.row.id,
        draftDigest: "a".repeat(64),
        revision: "d".repeat(64),
      });
      expect(host.legacyWrites).not.toHaveBeenCalled();
    } finally {
      await host.cleanup();
    }
  });

  it.each([
    "blocked",
    "error",
    "missingCatalog",
    "wrongChoice",
    "wrongRevision",
  ] as const)(
    "retires the old ready preview when management returns %s",
    async (manageResponse) => {
      const host = catalogHost({ manageResponse });
      try {
        await catalogPreview();
        await chooseManagedCatalog();
        await waitFor(() => expect(host.previews).toHaveLength(2));
        await waitFor(() =>
          expect(
            screen.getByRole("button", { name: "provider.preview.preview" }),
          ).toBeEnabled(),
        );
        const confirm = screen.queryByRole("button", {
          name: "provider.preview.saveAndApply",
        });
        expect(confirm === null || confirm.hasAttribute("disabled")).toBe(true);
        expect(host.confirms).toHaveLength(0);
        expect(host.close).not.toHaveBeenCalled();
        await userEvent.click(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        );
        await waitFor(() => expect(host.previews).toHaveLength(3));
        expect(host.previews[2]).not.toHaveProperty("catalogDecision");
        await waitFor(() =>
          expect(
            screen.getByRole("radio", {
              name: "provider.preview.catalog.keep",
            }),
          ).toBeChecked(),
        );
      } finally {
        await host.cleanup();
      }
    },
  );

  it.each(["cancel", "reread", "draft", "stale"] as const)(
    "invalidates the selected catalog decision after %s",
    async (reset) => {
      const host = catalogHost({
        confirmStatus: reset === "stale" ? "stale" : "completed",
      });
      try {
        await catalogPreview();
        await chooseManagedCatalog();
        await waitFor(() =>
          expect(
            screen.getByRole("button", {
              name: "provider.preview.saveAndApply",
            }),
          ).toBeEnabled(),
        );
        if (reset === "cancel")
          await userEvent.click(
            screen.getByRole("button", {
              name: "provider.preview.catalog.cancelChoice",
            }),
          );
        if (reset === "draft")
          fireEvent.change(screen.getByDisplayValue(host.row.name), {
            target: { value: "Changed catalog draft" },
          });
        if (reset === "stale") {
          await userEvent.click(
            screen.getByRole("button", {
              name: "provider.preview.saveAndApply",
            }),
          );
          await screen.findByText("provider.preview.result.stale");
          await userEvent.click(
            screen.getByRole("button", { name: "provider.preview.backToEdit" }),
          );
        }
        await userEvent.click(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        );
        await waitFor(() => expect(host.previews).toHaveLength(3));
        expect(host.previews[2]).not.toHaveProperty("catalogDecision");
        await waitFor(() =>
          expect(
            screen.getByRole("radio", {
              name: "provider.preview.catalog.keep",
            }),
          ).toBeChecked(),
        );
        expect(host.confirms).toHaveLength(reset === "stale" ? 1 : 0);
      } finally {
        await host.cleanup();
      }
    },
  );

  it("only queries the original request after an unknown management result", async () => {
    const host = catalogHost({ confirmStatus: "unknown" });
    try {
      await catalogPreview();
      await chooseManagedCatalog();
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.saveAndApply" }),
        ).toBeEnabled(),
      );
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.saveAndApply" }),
      );
      await screen.findByText("provider.preview.result.unknown");
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.queryOriginal" }),
      );
      await waitFor(() => expect(host.queries).toHaveLength(1));
      expect(host.queries[0]).toEqual({
        app: "codex",
        request: host.confirms[0].request,
      });
      expect(host.confirms).toHaveLength(1);
      expect(host.previews).toHaveLength(2);
      expect(host.legacyWrites).not.toHaveBeenCalled();
    } finally {
      await host.cleanup();
    }
  });

  it.each([
    ["configuredExternal", true],
    ["managed", true],
    ["none", true],
    ["unclaimedExternal", false],
  ] as const)(
    "does not infer catalog takeover for ownership=%s, canManage=%s",
    async (ownership, canManage) => {
      const host = catalogHost({ ownership, canManage });
      try {
        await catalogPreview();
        expect(
          screen.queryByRole("radio", {
            name: "provider.preview.catalog.manage",
          }),
        ).not.toBeInTheDocument();
        expect(host.confirms).toHaveLength(0);
      } finally {
        await host.cleanup();
      }
    },
  );

  it("drops a management preview that returns after the form changes", async () => {
    let release!: () => void;
    const manageHold = new Promise<void>((resolve) => {
      release = resolve;
    });
    const host = catalogHost({ manageHold });
    try {
      await catalogPreview();
      await chooseManagedCatalog();
      await waitFor(() => expect(host.previews).toHaveLength(2));
      fireEvent.change(screen.getByDisplayValue(host.row.name), {
        target: { value: "Newer catalog draft" },
      });
      await act(async () => {
        release();
      });
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        ).toBeEnabled(),
      );
      expect(
        screen.queryByRole("button", { name: "provider.preview.saveAndApply" }),
      ).not.toBeInTheDocument();
      expect(host.confirms).toHaveLength(0);
    } finally {
      release();
      await host.cleanup();
    }
  });
});

function catalogRestoreHost(
  options: {
    canRestore?: boolean;
    previewHold?: Promise<void>;
    confirmHold?: Promise<void>;
    confirmStatus?: string;
    loseResponse?: boolean;
    previewError?: boolean;
    badPreview?: "scope" | "providerId" | "requestId" | "revision" | "app";
    badResult?: boolean;
    afterPointer?: string | null;
  } = {},
) {
  const previews: Array<{ requestId: string }> = [];
  const confirms: Array<{ request: Record<string, unknown> }> = [];
  const queries: Array<{ request: Record<string, unknown> }> = [];
  server.use(
    http.post(
      "http://tauri.local/preview_codex_catalog_restore",
      async ({ request }) => {
        const input = (await request.json()) as (typeof previews)[number];
        previews.push(input);
        await options.previewHold;
        if (options.previewError)
          return new HttpResponse("SYNTHETIC_SECRET_MUST_NOT_RENDER", {
            status: 500,
          });
        return HttpResponse.json({
          app: options.badPreview === "app" ? "claude" : "codex",
          status: "ready",
          request: {
            id:
              options.badPreview === "requestId"
                ? "00000000-0000-0000-0000-000000000000"
                : input.requestId,
            scope:
              options.badPreview === "scope"
                ? "provider"
                : "codexCatalogRestore",
            ...(options.badPreview === "providerId"
              ? { providerId: provider.id }
              : {}),
            draftDigest: "f".repeat(64),
            revision:
              options.badPreview === "revision" ? "invalid" : "e".repeat(64),
          },
          beforePointer: managedPointer,
          afterPointer:
            options.afterPointer === undefined
              ? externalPointer
              : options.afterPointer,
        });
      },
    ),
    http.post(
      "http://tauri.local/confirm_codex_catalog_restore",
      async ({ request }) => {
        const input = (await request.json()) as (typeof confirms)[number];
        confirms.push(input);
        await options.confirmHold;
        if (options.loseResponse)
          return new HttpResponse("response lost", { status: 500 });
        return HttpResponse.json({
          app: "codex",
          request: options.badResult
            ? { ...input.request, scope: "provider" }
            : input.request,
          status: options.confirmStatus ?? "completed",
        });
      },
    ),
    http.post(
      "http://tauri.local/query_codex_catalog_restore",
      async ({ request }) => {
        const input = (await request.json()) as (typeof queries)[number];
        queries.push(input);
        return HttpResponse.json({
          app: "codex",
          request: input.request,
          status: "completed",
        });
      },
    ),
  );
  const host = catalogHost({
    ownership: "managed",
    canManage: false,
    canRestore: options.canRestore ?? true,
  });
  return {
    ...host,
    restorePreviews: previews,
    restoreConfirms: confirms,
    restoreQueries: queries,
  };
}

async function previewCatalogRestore() {
  await catalogPreview();
  await userEvent.click(
    screen.getByRole("button", { name: "provider.preview.catalog.restore" }),
  );
  return await screen.findByRole("dialog", {
    name: "provider.preview.catalog.restoreTitle",
  });
}

describe("R5 catalog restore in the real editor", () => {
  it("previews and confirms only the saved pointer while preserving an unsaved draft", async () => {
    const host = catalogRestoreHost();
    try {
      await screen.findByDisplayValue(provider.name);
      fireEvent.change(screen.getByDisplayValue(provider.name), {
        target: { value: "Unsaved restore draft" },
      });
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      );
      await screen.findByRole("button", {
        name: "provider.preview.catalog.restore",
      });
      await userEvent.click(
        screen.getByRole("button", {
          name: "provider.preview.catalog.restore",
        }),
      );
      const dialog = await screen.findByRole("dialog", {
        name: "provider.preview.catalog.restoreTitle",
      });
      expect(dialog).toHaveTextContent("provider.preview.catalog.restoreHint");
      expect(dialog).toHaveTextContent(managedPointer);
      expect(dialog).toHaveTextContent(externalPointer);
      expect(host.restorePreviews).toHaveLength(1);
      expect(Object.keys(host.restorePreviews[0])).toEqual(["requestId"]);
      expect(host.restoreConfirms).toHaveLength(0);
      await userEvent.click(
        screen.getByRole("button", {
          name: "provider.preview.catalog.confirmRestore",
        }),
      );
      await screen.findByText("provider.preview.catalog.restored");
      expect(host.restoreConfirms).toHaveLength(1);
      expect(Object.keys(host.restoreConfirms[0])).toEqual(["request"]);
      expect(host.restoreConfirms[0].request).toEqual({
        id: host.restorePreviews[0].requestId,
        scope: "codexCatalogRestore",
        draftDigest: "f".repeat(64),
        revision: "e".repeat(64),
      });
      expect(host.confirms).toHaveLength(0);
      expect(host.legacyWrites).not.toHaveBeenCalled();
      expect(host.close).not.toHaveBeenCalled();
      expect(
        screen.getByDisplayValue("Unsaved restore draft"),
      ).toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: "provider.preview.saveAndApply" }),
      ).not.toBeInTheDocument();
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.backToEdit" }),
      );
      expect(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      ).toBeEnabled();
    } finally {
      await host.cleanup();
    }
  });

  it.each(["cancel", "escape", "draft"])(
    "discards restore preview on %s without writing",
    async (action) => {
      const host = catalogRestoreHost();
      try {
        await previewCatalogRestore();
        if (action === "cancel")
          await userEvent.click(
            screen.getByRole("button", { name: "common.cancel" }),
          );
        else if (action === "escape") await userEvent.keyboard("{Escape}");
        else
          fireEvent.change(screen.getByDisplayValue(provider.name), {
            target: { value: "Changed during restore preview" },
          });
        await waitFor(() =>
          expect(
            screen.queryByRole("dialog", {
              name: "provider.preview.catalog.restoreTitle",
            }),
          ).not.toBeInTheDocument(),
        );
        expect(host.restoreConfirms).toHaveLength(0);
        expect(host.confirms).toHaveLength(0);
        expect(host.close).not.toHaveBeenCalled();
        if (action !== "draft") {
          await userEvent.click(
            screen.getByRole("button", {
              name: "provider.preview.catalog.restore",
            }),
          );
          await screen.findByRole("dialog", {
            name: "provider.preview.catalog.restoreTitle",
          });
          expect(host.restorePreviews).toHaveLength(2);
          expect(host.restorePreviews[1].requestId).not.toBe(
            host.restorePreviews[0].requestId,
          );
        }
      } finally {
        await host.cleanup();
      }
    },
  );

  it.each([false, undefined])(
    "does not infer restoration from ownership without canRestore=%s",
    async (canRestore) => {
      const host = catalogHost({
        ownership: "managed",
        canManage: false,
        canRestore,
      });
      try {
        await catalogPreview();
        expect(
          screen.queryByRole("button", {
            name: "provider.preview.catalog.restore",
          }),
        ).not.toBeInTheDocument();
      } finally {
        await host.cleanup();
      }
    },
  );

  it.each(["scope", "providerId", "requestId", "revision", "app"] as const)(
    "rejects a restore preview with mismatched %s",
    async (badPreview) => {
      const host = catalogRestoreHost({ badPreview });
      try {
        await catalogPreview();
        await userEvent.click(
          screen.getByRole("button", {
            name: "provider.preview.catalog.restore",
          }),
        );
        await screen.findByText("provider.preview.catalog.restoreUnavailable");
        expect(
          screen.queryByRole("dialog", {
            name: "provider.preview.catalog.restoreTitle",
          }),
        ).not.toBeInTheDocument();
        expect(host.restoreConfirms).toHaveLength(0);
      } finally {
        await host.cleanup();
      }
    },
  );

  it("hides raw preview errors and never offers confirm after an unavailable source", async () => {
    const host = catalogRestoreHost({ previewError: true });
    try {
      await catalogPreview();
      await userEvent.click(
        screen.getByRole("button", {
          name: "provider.preview.catalog.restore",
        }),
      );
      await screen.findByText("provider.preview.catalog.restoreUnavailable");
      expect(document.body).not.toHaveTextContent(
        "SYNTHETIC_SECRET_MUST_NOT_RENDER",
      );
      expect(host.restoreConfirms).toHaveLength(0);
    } finally {
      await host.cleanup();
    }
  });

  it.each([
    "lostResponse",
    "badResult",
    "pending",
    "verificationRequired",
    "unknown",
  ])(
    "queries the original request after %s without resubmitting restore or the provider",
    async (result) => {
      const host = catalogRestoreHost({
        loseResponse: result === "lostResponse",
        badResult: result === "badResult",
        confirmStatus: result,
      });
      try {
        await previewCatalogRestore();
        await userEvent.click(
          screen.getByRole("button", {
            name: "provider.preview.catalog.confirmRestore",
          }),
        );
        await screen.findByText(
          `provider.preview.result.${["lostResponse", "badResult"].includes(result) ? "unknown" : result}`,
        );
        expect(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        ).toBeDisabled();
        expect(
          screen.queryByRole("button", { name: "provider.preview.backToEdit" }),
        ).not.toBeInTheDocument();
        fireEvent.change(screen.getByDisplayValue(provider.name), {
          target: { value: "Edited while result is unknown" },
        });
        await userEvent.click(
          screen.getByRole("button", {
            name: "provider.preview.queryOriginal",
          }),
        );
        await screen.findByText("provider.preview.catalog.restored");
        expect(host.restoreConfirms).toHaveLength(1);
        expect(host.restoreQueries).toEqual([host.restoreConfirms[0]]);
        expect(
          screen.getByDisplayValue("Edited while result is unknown"),
        ).toBeInTheDocument();
        expect(host.confirms).toHaveLength(0);
        expect(host.close).not.toHaveBeenCalled();
      } finally {
        await host.cleanup();
      }
    },
  );

  it.each(["blocked", "stale", "conflict"])(
    "reports %s without claiming restoration or reusing the provider preview",
    async (confirmStatus) => {
      const host = catalogRestoreHost({ confirmStatus });
      try {
        await previewCatalogRestore();
        await userEvent.click(
          screen.getByRole("button", {
            name: "provider.preview.catalog.confirmRestore",
          }),
        );
        await screen.findByText(`provider.preview.result.${confirmStatus}`);
        expect(
          screen.queryByText("provider.preview.catalog.restored"),
        ).not.toBeInTheDocument();
        await userEvent.click(
          screen.getByRole("button", { name: "provider.preview.backToEdit" }),
        );
        expect(
          screen.queryByRole("button", {
            name: "provider.preview.saveAndApply",
          }),
        ).not.toBeInTheDocument();
        expect(host.restoreConfirms).toHaveLength(1);
      } finally {
        await host.cleanup();
      }
    },
  );

  it("ignores a delayed restore preview after a newer form revision", async () => {
    let release!: () => void;
    const previewHold = new Promise<void>((resolve) => {
      release = resolve;
    });
    const host = catalogRestoreHost({ previewHold });
    try {
      await catalogPreview();
      await userEvent.click(
        screen.getByRole("button", {
          name: "provider.preview.catalog.restore",
        }),
      );
      await waitFor(() => expect(host.restorePreviews).toHaveLength(1));
      expect(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      ).toBeDisabled();
      fireEvent.change(screen.getByDisplayValue(provider.name), {
        target: { value: "Newer pointer draft" },
      });
      await act(async () => {
        release();
        await previewHold;
      });
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        ).toBeEnabled(),
      );
      expect(
        screen.queryByRole("dialog", {
          name: "provider.preview.catalog.restoreTitle",
        }),
      ).not.toBeInTheDocument();
      expect(host.restoreConfirms).toHaveLength(0);
    } finally {
      release();
      await host.cleanup();
    }
  });

  it("does not revive a delayed restore preview in a replacement editor session", async () => {
    let release!: () => void;
    const previewHold = new Promise<void>((resolve) => {
      release = resolve;
    });
    const host = catalogRestoreHost({ previewHold });
    try {
      await catalogPreview();
      await userEvent.click(
        screen.getByRole("button", {
          name: "provider.preview.catalog.restore",
        }),
      );
      await waitFor(() => expect(host.restorePreviews).toHaveLength(1));
      host.view.rerender(
        <QueryClientProvider client={host.client}>
          <Editor
            row={{
              ...host.row,
              id: "replacement-editor-row",
              name: "Replacement editor",
            }}
            app="codex"
            close={host.close}
          />
        </QueryClientProvider>,
      );
      await screen.findByDisplayValue("Replacement editor");
      await act(async () => {
        release();
        await previewHold;
      });
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "provider.preview.preview" }),
        ).toBeEnabled(),
      );
      expect(
        screen.queryByRole("dialog", {
          name: "provider.preview.catalog.restoreTitle",
        }),
      ).not.toBeInTheDocument();
      expect(host.restoreConfirms).toHaveLength(0);
    } finally {
      release();
      await host.cleanup();
    }
  });

  it("coalesces repeated confirmation and excludes provider save while restoration is pending", async () => {
    let release!: () => void;
    const confirmHold = new Promise<void>((resolve) => {
      release = resolve;
    });
    const host = catalogRestoreHost({ confirmHold, afterPointer: null });
    try {
      const dialog = await previewCatalogRestore();
      expect(dialog).toHaveTextContent("provider.preview.catalog.noPointer");
      const confirm = screen.getByRole("button", {
        name: "provider.preview.catalog.confirmRestore",
      });
      fireEvent.click(confirm);
      fireEvent.click(confirm);
      await waitFor(() => expect(host.restoreConfirms).toHaveLength(1));
      expect(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      ).toBeDisabled();
      fireEvent.submit(document.getElementById("provider-form")!);
      expect(host.confirms).toHaveLength(0);
      await act(async () => {
        release();
        await confirmHold;
      });
      await screen.findByText("provider.preview.catalog.restored");
      expect(host.restoreConfirms).toHaveLength(1);
    } finally {
      release();
      await host.cleanup();
    }
  });
});

describe("R5 reopened catalog restore receipt", () => {
  it("queries the original restore after close and reopen without repeating either write", async () => {
    const host = catalogRestoreHost({ loseResponse: true });
    try {
      await previewCatalogRestore();
      await userEvent.click(
        screen.getByRole("button", {
          name: "provider.preview.catalog.confirmRestore",
        }),
      );
      await screen.findByText("provider.preview.result.unknown");
      expect(host.restoreConfirms).toHaveLength(1);
      const original = host.restoreConfirms[0].request;
      host.view.rerender(
        <QueryClientProvider client={host.client}>{null}</QueryClientProvider>,
      );
      server.use(
        http.post("http://tauri.local/get_provider_edit_settings", () =>
          HttpResponse.json({
            settingsConfig: host.row.settingsConfig,
            modeState: { status: "unknown", canWrite: false },
            originalSave: null,
            originalCatalogRestore: {
              app: "codex",
              request: original,
              status: "unknown",
            },
          }),
        ),
      );
      host.view.rerender(
        <QueryClientProvider client={host.client}>
          <Editor row={host.row} app="codex" close={host.close} />
        </QueryClientProvider>,
      );
      await screen.findByDisplayValue(provider.name);
      const query = await screen.findByRole("button", {
        name: "provider.preview.queryOriginal",
      });
      expect(
        screen.getByRole("button", { name: "provider.preview.preview" }),
      ).toBeDisabled();
      expect(
        screen.queryByRole("button", {
          name: "provider.preview.catalog.confirmRestore",
        }),
      ).not.toBeInTheDocument();
      fireEvent.change(screen.getByDisplayValue(provider.name), {
        target: { value: "New draft after reopening" },
      });
      await userEvent.click(query);
      await screen.findByText("provider.preview.catalog.restored");
      expect(host.restoreQueries).toEqual([{ request: original }]);
      expect(host.restoreConfirms).toHaveLength(1);
      expect(host.restorePreviews).toHaveLength(1);
      expect(host.confirms).toHaveLength(0);
      expect(host.queries).toHaveLength(0);
      expect(host.legacyWrites).not.toHaveBeenCalled();
      expect(host.close).not.toHaveBeenCalled();
      expect(
        screen.getByDisplayValue("New draft after reopening"),
      ).toBeInTheDocument();
    } finally {
      await host.cleanup();
    }
  });

  it("keeps a backend-selected provider receipt on the original provider query route", async () => {
    const host = catalogRestoreHost();
    const original = {
      id: "00000000-0000-0000-0000-000000000000",
      providerId: host.row.id,
      draftDigest: "a".repeat(64),
      revision: "b".repeat(64),
    };
    try {
      host.view.rerender(
        <QueryClientProvider client={host.client}>{null}</QueryClientProvider>,
      );
      server.use(
        http.post("http://tauri.local/get_provider_edit_settings", () =>
          HttpResponse.json({
            settingsConfig: host.row.settingsConfig,
            modeState: { status: "unknown", canWrite: false },
            originalSave: {
              app: "codex",
              request: original,
              status: "unknown",
            },
            originalCatalogRestore: null,
          }),
        ),
      );
      host.view.rerender(
        <QueryClientProvider client={host.client}>
          <Editor row={host.row} app="codex" close={host.close} />
        </QueryClientProvider>,
      );
      await screen.findByRole("button", {
        name: "provider.preview.queryOriginal",
      });
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.queryOriginal" }),
      );
      await screen.findByText("provider.preview.savedPreviousDraft");
      expect(host.queries).toEqual([{ app: "codex", request: original }]);
      expect(host.restoreQueries).toHaveLength(0);
      expect(host.restoreConfirms).toHaveLength(0);
      expect(host.confirms).toHaveLength(0);
      expect(host.close).not.toHaveBeenCalled();
    } finally {
      await host.cleanup();
    }
  });

  it.each(["app", "scope", "id", "providerId", "digest", "revision"] as const)(
    "rejects an invalid reopened restore %s without a query or write",
    async (field) => {
      const host = catalogRestoreHost();
      try {
        host.view.rerender(
          <QueryClientProvider client={host.client}>
            {null}
          </QueryClientProvider>,
        );
        server.use(
          http.post("http://tauri.local/get_provider_edit_settings", () =>
            HttpResponse.json({
              settingsConfig: host.row.settingsConfig,
              modeState: { status: "unknown", canWrite: false },
              originalSave: null,
              originalCatalogRestore: {
                app: field === "app" ? "claude" : "codex",
                request: {
                  id:
                    field === "id"
                      ? "not-a-uuid"
                      : "00000000-0000-0000-0000-000000000000",
                  scope: field === "scope" ? "provider" : "codexCatalogRestore",
                  ...(field === "providerId"
                    ? { providerId: host.row.id }
                    : {}),
                  draftDigest: field === "digest" ? "bad" : "a".repeat(64),
                  revision: field === "revision" ? "bad" : "b".repeat(64),
                },
                status: "unknown",
              },
            }),
          ),
        );
        host.view.rerender(
          <QueryClientProvider client={host.client}>
            <Editor row={host.row} app="codex" close={host.close} />
          </QueryClientProvider>,
        );
        await screen.findByText("provider.preview.sourceUnavailable");
        expect(
          screen.queryByRole("button", {
            name: "provider.preview.queryOriginal",
          }),
        ).not.toBeInTheDocument();
        expect(host.restoreQueries).toHaveLength(0);
        expect(host.restoreConfirms).toHaveLength(0);
        expect(host.confirms).toHaveLength(0);
      } finally {
        await host.cleanup();
      }
    },
  );
});
