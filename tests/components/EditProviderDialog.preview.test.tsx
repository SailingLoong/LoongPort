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
