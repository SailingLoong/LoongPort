import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { http, HttpResponse } from "msw";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { CodexSharedSettings } from "@/components/settings/CodexSharedSettings";
import type {
  SharedSettingsApp,
  SharedSettingsConfig,
  SharedSettingsRequest,
} from "@/lib/api/config";
import { server } from "../msw/server";

// Keep the real CodeMirror, shared-settings component, and API adapter. Only
// native IPC is supplied by the existing MSW transport; this is not native QA.
const rangeDescriptors = ["getClientRects", "getBoundingClientRect"].map(
  (key) =>
    [key, Object.getOwnPropertyDescriptor(Range.prototype, key)] as const,
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
  for (const [key, descriptor] of rangeDescriptors) {
    if (descriptor) Object.defineProperty(Range.prototype, key, descriptor);
    else Reflect.deleteProperty(Range.prototype, key);
  }
});

const cases: {
  app: SharedSettingsApp;
  initial: SharedSettingsConfig;
  edited: SharedSettingsConfig;
}[] = [
  {
    app: "codex",
    initial: "# retained comment\n[history]\nmax_bytes = 42\n",
    edited: "# retained comment\n[history]\nmax_bytes = 9007199254740993\n",
  },
  {
    app: "claude",
    initial: '{ "ui": { "literal.key": 42 } }\n',
    edited: '{ "ui": { "literal.key": 9007199254740993 } }\n',
  },
  {
    app: "gemini",
    initial: {
      env: '# retained comment\nGEMINI_CLI_THEME="light"\n',
      settings: '{ "ui": { "literal.key": 42 } }\n',
    },
    edited: {
      env: '# retained comment\nGEMINI_CLI_THEME="dark"\n',
      settings: '{ "ui": { "literal.key": 9007199254740993 } }\n',
    },
  },
];
function views() {
  return [...document.querySelectorAll<HTMLElement>(".cm-editor")].map(
    (element) => EditorView.findFromDOM(element)!,
  );
}
function texts(value: SharedSettingsConfig) {
  return typeof value === "string" ? [value] : [value.env, value.settings];
}

// Mutating the component to normalize text, route to another app, repeat save,
// or permit writes while the original receipt is unresolved must fail these.
describe("U05 actual editor and IPC adapter recovery", () => {
  it.each(cases)(
    "preserves $app source text and queries the same lost save without replay",
    async ({ app, initial, edited }) => {
      const calls: { command: string; payload: any }[] = [];
      let receipt: SharedSettingsRequest | undefined;
      let resultStatus = "pending";
      let releaseSave!: () => void;
      const saving = new Promise<void>((resolve) => {
        releaseSave = resolve;
      });
      server.use(
        http.post(
          "http://tauri.local/:command",
          async ({ params, request }) => {
            const command = String(params.command);
            const payload = (await request.json()) as any;
            calls.push({ command, payload });
            if (command === `get_${app}_shared_settings`)
              return HttpResponse.json({
                app,
                status: "ready",
                config: initial,
                originalSave: null,
              });
            if (command === `preview_${app}_shared_settings`) {
              receipt = {
                id: payload.requestId,
                scope: `${app}SharedSettings`,
                draftDigest: "synthetic-draft",
                revision: "synthetic-revision",
              };
              return HttpResponse.json({
                app,
                status: "ready",
                request: receipt,
                changes: [],
                files: [],
              });
            }
            if (command === `save_${app}_shared_settings`) {
              await saving;
              return new HttpResponse("synthetic lost save response", {
                status: 503,
              });
            }
            if (command === `get_${app}_shared_settings_save_result`)
              return HttpResponse.json({
                app,
                request: receipt,
                status: resultStatus,
              });
            return new HttpResponse(
              `Unexpected synthetic command: ${command}`,
              { status: 500 },
            );
          },
        ),
      );
      render(
        <CodexSharedSettings
          app={app}
          onClose={vi.fn()}
          beforeLeave={(proceed) => proceed()}
        />,
      );
      await waitFor(() => {
        expect(views().map((view) => view.state.doc.toString())).toEqual(
          texts(initial),
        );
        expect(
          views().every((view) => !view.state.facet(EditorState.readOnly)),
        ).toBe(true);
      });
      // Each edit is its own React transaction, as separate user input events are.
      for (const [index, value] of texts(edited).entries()) {
        act(() => {
          const view = views()[index];
          view.dispatch({
            changes: { from: 0, to: view.state.doc.length, insert: value },
          });
        });
      }
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.preview" }),
      );
      const save = screen.getByRole("button", {
        name: "codexSharedSettings.save",
      });
      await waitFor(() => expect(save).toBeEnabled());
      expect(
        calls.filter(
          ({ command }) => command === `preview_${app}_shared_settings`,
        ),
      ).toEqual([
        {
          command: `preview_${app}_shared_settings`,
          payload: { config: edited, requestId: receipt!.id },
        },
      ]);
      expect(views().map((view) => view.state.doc.toString())).toEqual(
        texts(edited),
      );
      fireEvent.click(save);
      await waitFor(() =>
        expect(
          calls.filter(
            ({ command }) => command === `save_${app}_shared_settings`,
          ),
        ).toHaveLength(1),
      );
      expect(save).toBeDisabled();
      expect(
        views().every((view) => view.state.facet(EditorState.readOnly)),
      ).toBe(true);
      fireEvent.click(save);
      act(() => releaseSave());
      await screen.findByText("codexSharedSettings.result.pending");
      expect(
        views().every((view) => view.state.facet(EditorState.readOnly)),
      ).toBe(true);
      for (const action of ["preview", "save", "reload", "import.open"]) {
        const button = screen.getByRole("button", {
          name: `codexSharedSettings.${action}`,
        });
        expect(button).toBeDisabled();
        fireEvent.click(button);
      }
      resultStatus = "completed";
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.checkResult" }),
      );
      await screen.findByText("codexSharedSettings.result.completed");
      expect(views().map((view) => view.state.doc.toString())).toEqual(
        texts(edited),
      );
      await waitFor(() => {
        const editors = views();
        expect(editors).toHaveLength(texts(edited).length);
        expect(
          editors.every((view) => !view.state.facet(EditorState.readOnly)),
        ).toBe(true);
      });
      expect(
        screen.getByRole("button", { name: "codexSharedSettings.preview" }),
      ).toBeDisabled();
      expect(calls).toEqual([
        { command: `get_${app}_shared_settings`, payload: {} },
        {
          command: `preview_${app}_shared_settings`,
          payload: { config: edited, requestId: receipt!.id },
        },
        {
          command: `save_${app}_shared_settings`,
          payload: { config: edited, request: receipt },
        },
        {
          command: `get_${app}_shared_settings_save_result`,
          payload: { request: receipt },
        },
        {
          command: `get_${app}_shared_settings_save_result`,
          payload: { request: receipt },
        },
      ]);
      expect(document.body).not.toHaveTextContent(
        "synthetic lost save response",
      );
    },
  );
});
