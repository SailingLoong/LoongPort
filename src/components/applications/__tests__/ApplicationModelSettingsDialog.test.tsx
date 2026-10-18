import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApplicationModelSettingsDialog } from "../ApplicationModelSettingsDialog";
import { applicationModelSettingsApi } from "@/lib/api/applicationModelSettings";
import type {
  ApplicationModelSettings,
  ReasoningCapabilities,
} from "@/lib/api/applicationModelSettings";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("@/lib/api/applicationModelSettings", async (importOriginal) => ({
  ...(await importOriginal<
    typeof import("@/lib/api/applicationModelSettings")
  >()),
  applicationModelSettingsApi: {
    get: vi.fn(),
    preview: vi.fn(),
    confirm: vi.fn(),
    query: vi.fn(),
  },
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
const request = {
  scope: "providerModelSettings" as const,
  id: "33333333-3333-4333-8333-333333333333",
  providerId: "relay-7",
  draftDigest: "a".repeat(64),
  revision: "b".repeat(64),
};
const capability = (
  mode: ReasoningCapabilities["mode"],
  levels: string[] = [],
): ReasoningCapabilities => ({
  mode,
  levels,
  defaultLevel: null,
  source: "appDeclaration",
  conflicts: [],
  verified: false,
  transport: {
    kind: "explicitTemplate",
    supportsThinking: true,
    supportsEffort: true,
    thinkingParam: "thinking.type",
    effortParam: "reasoning_effort",
    effortValueMode: "passthrough",
  },
  controls: {
    on: mode === "toggle",
    off: mode === "toggle",
    effort: levels.length > 0,
  },
});
const state = (
  capabilities = capability("unknown"),
): ApplicationModelSettings => ({
  app: "codex",
  providerId: "relay-7",
  revision: "b".repeat(64),
  models: [
    { model: "alias-a", capabilities },
    { model: "alias-b", capabilities: capability("required", ["low", "high"]) },
  ],
  selectedModel: "alias-a",
  reasoningIntent: { kind: "serviceDefault" },
  overrides: { model: true, reasoning: true },
  canRestoreModel: true,
  canRestoreReasoning: true,
  sourceChanged: false,
  editable: true,
  conflicts: [],
  restartRequired: true,
  manualConfig: false,
});
const props = {
  appId: "codex" as const,
  providerId: "relay-7",
  name: "Relay tier",
  open: true,
  onOpenChange: vi.fn(),
  onSaved: vi.fn(),
  onAdvanced: vi.fn(),
};
const openReasoning = async () =>
  userEvent.click(
    await screen.findByRole("combobox", {
      name: "applications.modelSettings.reasoning",
    }),
  );
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(applicationModelSettingsApi.get).mockResolvedValue(state());
  vi.mocked(applicationModelSettingsApi.preview).mockImplementation(
    async (_app, _id, _patch, requestId) => ({
      app: "codex",
      request: { ...request, id: requestId },
      status: "ready",
      action: "saveOnly",
      fields: ["models"],
      files: [],
      preserves: [],
    }),
  );
  vi.mocked(applicationModelSettingsApi.confirm).mockImplementation(
    async (_app, _id, _patch, value) => ({
      app: "codex",
      request: value,
      status: "completed",
    }),
  );
});

describe("application model settings", () => {
  it("keeps unknown and explicit empty capabilities unknown without a fabricated strength", async () => {
    render(<ApplicationModelSettingsDialog {...props} />);
    await openReasoning();
    expect(
      screen.getByRole("option", {
        name: "applications.modelSettings.serviceDefault",
      }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "high" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "applications.modelSettings.off" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText("applications.modelSettings.unknown"),
    ).toBeInTheDocument();
  });
  it("offers a toggle-only declaration without fabricated strengths", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue(
      state(capability("toggle")),
    );
    render(<ApplicationModelSettingsDialog {...props} />);
    await openReasoning();
    expect(
      screen.getByRole("option", { name: "applications.modelSettings.on" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("option", { name: "applications.modelSettings.off" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "high" }),
    ).not.toBeInTheDocument();
  });
  it("never offers off for required reasoning and does not preselect service default strength", async () => {
    const value = state(capability("required", ["low", "high"]));
    value.models[0].capabilities.defaultLevel = "high";
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue(value);
    render(<ApplicationModelSettingsDialog {...props} />);
    expect(
      await screen.findByRole("combobox", {
        name: "applications.modelSettings.reasoning",
      }),
    ).toHaveTextContent("applications.modelSettings.serviceDefault");
    await openReasoning();
    expect(screen.getByRole("option", { name: "low" })).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "applications.modelSettings.off" }),
    ).not.toBeInTheDocument();
  });
  it("does not infer toggle support from supported reasoning", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue(
      state(capability("supported", ["high"])),
    );
    render(<ApplicationModelSettingsDialog {...props} />);
    await openReasoning();
    expect(screen.getByRole("option", { name: "high" })).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "applications.modelSettings.on" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "applications.modelSettings.off" }),
    ).not.toBeInTheDocument();
  });
  it("restores only the named field and requires preview confirmation", async () => {
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.restoreReasoning",
      }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    await screen.findByRole("button", {
      name: "applications.modelSettings.confirm",
    });
    expect(applicationModelSettingsApi.preview).toHaveBeenCalledWith(
      "codex",
      "relay-7",
      { reasoning: { action: "restore" } },
      expect.any(String),
    );
    expect(applicationModelSettingsApi.confirm).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getByRole("button", {
        name: "applications.modelSettings.confirm",
      }),
    );
    await waitFor(() => expect(props.onSaved).toHaveBeenCalledOnce());
    expect(applicationModelSettingsApi.confirm).toHaveBeenCalledWith(
      "codex",
      "relay-7",
      { reasoning: { action: "restore" } },
      expect.objectContaining({ providerId: "relay-7" }),
    );
  });
  it("does not silently replace a preserved explicit effort when selecting another model", async () => {
    const value = state(capability("toggle", ["medium"]));
    value.reasoningIntent = { kind: "effort", level: "medium" };
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue(value);
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("combobox", {
        name: "applications.modelSettings.model",
      }),
    );
    await userEvent.click(screen.getByRole("option", { name: "alias-b" }));
    expect(
      screen.getByRole("combobox", {
        name: "applications.modelSettings.reasoning",
      }),
    ).toHaveTextContent("medium");
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    await waitFor(() =>
      expect(applicationModelSettingsApi.preview).toHaveBeenCalledWith(
        "codex",
        "relay-7",
        { model: { action: "set", value: "alias-b" } },
        expect.any(String),
      ),
    );
    expect(applicationModelSettingsApi.confirm).not.toHaveBeenCalled();
  });
  it("preserves manual configuration and opens the existing advanced editor", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      manualConfig: true,
      editable: false,
    });
    render(<ApplicationModelSettingsDialog {...props} />);
    await screen.findByText("applications.modelSettings.manualConfig");
    expect(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    ).toBeDisabled();
    await userEvent.click(
      screen.getByRole("button", {
        name: "applications.modelSettings.advanced",
      }),
    );
    expect(props.onAdvanced).toHaveBeenCalledOnce();
  });
  it("blocks confirmation when the backend reports a hard conflict", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      conflicts: ["endpoint conflicts with declaration"],
    });
    render(<ApplicationModelSettingsDialog {...props} />);
    await screen.findByText("endpoint conflicts with declaration");
    expect(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    ).toBeDisabled();
  });
  it("queries the original request after a lost reply, without resubmitting a write", async () => {
    vi.mocked(applicationModelSettingsApi.confirm).mockRejectedValue(
      new Error("reply lost"),
    );
    vi.mocked(applicationModelSettingsApi.query).mockImplementation(
      async (_app, value) => ({
        app: "codex",
        request: value,
        status: "completed",
      }),
    );
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.restoreModel",
      }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.confirm",
      }),
    );
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.recheck",
      }),
    );
    await waitFor(() => expect(props.onSaved).toHaveBeenCalledOnce());
    expect(applicationModelSettingsApi.confirm).toHaveBeenCalledOnce();
    expect(applicationModelSettingsApi.query).toHaveBeenCalledOnce();
  });
  it("saves toggle-only on as typed intent without an invented effort", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue(
      state(capability("toggle")),
    );
    render(<ApplicationModelSettingsDialog {...props} />);
    await openReasoning();
    await userEvent.click(
      screen.getByRole("option", { name: "applications.modelSettings.on" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    await waitFor(() =>
      expect(applicationModelSettingsApi.preview).toHaveBeenCalledWith(
        "codex",
        "relay-7",
        { reasoning: { action: "set", value: { kind: "on" } } },
        expect.any(String),
      ),
    );
  });

  it("recovers a durable unresolved request on reopening and queries only that request", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      originalSave: { app: "codex", request, status: "unknown" },
    });
    vi.mocked(applicationModelSettingsApi.query).mockResolvedValue({
      app: "codex",
      request,
      status: "completed",
    });
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.recheck",
      }),
    );
    await waitFor(() => expect(props.onSaved).toHaveBeenCalledOnce());
    expect(applicationModelSettingsApi.query).toHaveBeenCalledWith(
      "codex",
      request,
    );
    expect(applicationModelSettingsApi.preview).not.toHaveBeenCalled();
    expect(applicationModelSettingsApi.confirm).not.toHaveBeenCalled();
  });

  it("ignores an old provider read that arrives after changing scope", async () => {
    let resolveOld!: (value: ApplicationModelSettings) => void;
    vi.mocked(applicationModelSettingsApi.get).mockImplementation((_app, id) =>
      id === "relay-7"
        ? new Promise((resolve) => {
            resolveOld = resolve;
          })
        : Promise.resolve({
            ...state(),
            providerId: id,
            selectedModel: "new-scope-model",
            models: [
              { model: "new-scope-model", capabilities: capability("unknown") },
            ],
          }),
    );
    const rendered = render(<ApplicationModelSettingsDialog {...props} />);
    rendered.rerender(
      <ApplicationModelSettingsDialog {...props} providerId="relay-8" />,
    );
    expect(
      await screen.findByRole("combobox", {
        name: "applications.modelSettings.model",
      }),
    ).toHaveTextContent("new-scope-model");
    resolveOld(state());
    await waitFor(() =>
      expect(
        screen.getByRole("combobox", {
          name: "applications.modelSettings.model",
        }),
      ).toHaveTextContent("new-scope-model"),
    );
  });

  it("renders only backend-encoded controls even when the model declaration says toggle", async () => {
    const resolved = capability("toggle", ["high"]);
    resolved.controls = { on: false, off: true, effort: false };
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue(
      state(resolved),
    );
    render(<ApplicationModelSettingsDialog {...props} />);
    await openReasoning();
    expect(
      screen.getByRole("option", { name: "applications.modelSettings.off" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "applications.modelSettings.on" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "high" }),
    ).not.toBeInTheDocument();
  });

  it("submits an unknown alias declaration and a backend-listed endpoint template without inventing strength", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      templateOptions: [{ id: "test-template", label: "Compatible endpoint" }],
      selectedTemplateId: null,
    });
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.customModel",
      }),
    );
    const input = screen.getByRole("textbox", {
      name: "applications.modelSettings.customModel",
    });
    await userEvent.clear(input);
    await userEvent.type(input, "private-alias");
    await userEvent.click(
      screen.getByRole("button", {
        name: "applications.modelSettings.declarationTitle",
      }),
    );
    await userEvent.click(
      screen.getByRole("combobox", {
        name: "applications.modelSettings.declarationMode",
      }),
    );
    await userEvent.click(
      screen.getByRole("option", { name: "applications.modelSettings.toggle" }),
    );
    await userEvent.click(
      screen.getByRole("combobox", {
        name: "applications.modelSettings.endpointTemplate",
      }),
    );
    await userEvent.click(
      screen.getByRole("option", { name: "Compatible endpoint" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    await waitFor(() =>
      expect(applicationModelSettingsApi.preview).toHaveBeenCalledWith(
        "codex",
        "relay-7",
        {
          model: { action: "set", value: "private-alias" },
          declaration: {
            model: "private-alias",
            change: {
              action: "set",
              value: { reasoningMode: "toggle", reasoningLevels: [] },
            },
          },
          transport: { action: "set", value: { templateId: "test-template" } },
        },
        expect.any(String),
      ),
    );
  });

  it("restores only the current model declaration when native source inheritance is available", async () => {
    const value = state();
    value.models[0].declaration = {
      reasoningMode: "unknown",
      reasoningLevels: [],
    };
    value.models[0].declarationOverride = true;
    value.models[0].canRestoreDeclaration = true;
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue(value);
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.declarationTitle",
      }),
    );
    await userEvent.click(
      screen.getByRole("button", {
        name: "applications.modelSettings.restoreDeclaration",
      }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    await waitFor(() =>
      expect(applicationModelSettingsApi.preview).toHaveBeenCalledWith(
        "codex",
        "relay-7",
        { declaration: { model: "alias-a", change: { action: "restore" } } },
        expect.any(String),
      ),
    );
  });

  it("does not permit restoring an original manual declaration without a source binding", async () => {
    const value = state();
    value.models[0].declaration = {
      reasoningMode: "unknown",
      reasoningLevels: [],
    };
    value.models[0].canRestoreDeclaration = false;
    value.models[0].restoreUnavailableReason =
      "No inherited source declaration";
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue(value);
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.declarationTitle",
      }),
    );
    expect(
      screen.getByRole("button", {
        name: "applications.modelSettings.restoreDeclaration",
      }),
    ).toBeDisabled();
    expect(
      screen.getByText("No inherited source declaration"),
    ).toBeInTheDocument();
  });
  it("rejects a full-provider scope preview instead of confirming it as a field patch", async () => {
    vi.mocked(applicationModelSettingsApi.preview).mockImplementation(
      async (_app, _id, _patch, requestId) =>
        ({
          app: "codex",
          request: { ...request, id: requestId, scope: "provider" },
          status: "ready",
          action: "saveOnly",
          fields: ["models"],
          files: [],
          preserves: [],
        }) as any,
    );
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.restoreModel",
      }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    await screen.findByText("applications.modelSettings.previewFailed");
    expect(
      screen.queryByRole("button", {
        name: "applications.modelSettings.confirm",
      }),
    ).not.toBeInTheDocument();
  });

  it("rejects an original receipt from a different save scope", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      originalSave: {
        app: "codex",
        request: { ...request, scope: "provider" },
        status: "unknown",
      },
    } as any);
    render(<ApplicationModelSettingsDialog {...props} />);
    await screen.findByText("applications.modelSettings.loadFailed");
    expect(
      screen.queryByRole("button", {
        name: "applications.modelSettings.recheck",
      }),
    ).not.toBeInTheDocument();
  });

  it("allows reviewing a repair to an existing conflict but never confirms a conflicting native preview", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      conflicts: ["existing conflict"],
    });
    vi.mocked(applicationModelSettingsApi.preview).mockImplementation(
      async (_app, _id, _patch, requestId) => ({
        app: "codex",
        request: { ...request, id: requestId },
        status: "blocked",
        code: "invalidFormat",
        action: "saveOnly",
        fields: ["models"],
        files: [],
        preserves: [],
        conflicts: ["endpoint rejects this declaration"],
      }),
    );
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.restoreReasoning",
      }),
    );
    expect(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    ).not.toBeDisabled();
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    await screen.findByText("endpoint rejects this declaration");
    expect(
      screen.queryByRole("button", {
        name: "applications.modelSettings.confirm",
      }),
    ).not.toBeInTheDocument();
    expect(applicationModelSettingsApi.confirm).not.toHaveBeenCalled();
  });

  it("does not offer model or thinking restore without native source-bound inheritance", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      canRestoreModel: false,
      canRestoreReasoning: false,
      restoreUnavailableReason:
        "Refresh the account to recover source defaults.",
    });
    render(<ApplicationModelSettingsDialog {...props} />);
    expect(
      await screen.findByRole("button", {
        name: "applications.modelSettings.restoreModel",
      }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", {
        name: "applications.modelSettings.restoreReasoning",
      }),
    ).toBeDisabled();
    expect(
      screen.getAllByText("Refresh the account to recover source defaults.")
        .length,
    ).toBeGreaterThan(0);
  });

  it.each([
    "notRecorded",
    "discarded",
    "abandoned",
    "stale",
    "blocked",
    "conflict",
  ] as const)(
    "allows a fresh preview after the known terminal result %s",
    async (status) => {
      vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
        ...state(),
        originalSave: { app: "codex", request, status },
      });
      render(<ApplicationModelSettingsDialog {...props} />);
      expect(
        await screen.findByText(`provider.preview.result.${status}`),
      ).toBeInTheDocument();
      expect(
        screen.queryByText("applications.modelSettings.unresolved"),
      ).not.toBeInTheDocument();
      await userEvent.click(
        screen.getByRole("button", { name: "provider.preview.backToEdit" }),
      );
      await userEvent.click(
        screen.getByRole("button", {
          name: "applications.modelSettings.restoreModel",
        }),
      );
      await userEvent.click(
        screen.getByRole("button", {
          name: "applications.modelSettings.review",
        }),
      );
      await waitFor(() =>
        expect(applicationModelSettingsApi.preview).toHaveBeenCalledOnce(),
      );
      expect(applicationModelSettingsApi.confirm).not.toHaveBeenCalled();
    },
  );

  it("does not accept a query result with the same identity from a full-provider scope", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      originalSave: { app: "codex", request, status: "unknown" },
    });
    vi.mocked(applicationModelSettingsApi.query).mockResolvedValue({
      app: "codex",
      request: { ...request, scope: "provider" },
      status: "completed",
    } as any);
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.recheck",
      }),
    );
    expect(
      await screen.findByText("applications.modelSettings.unresolved"),
    ).toBeInTheDocument();
    expect(props.onSaved).not.toHaveBeenCalled();
    expect(props.onOpenChange).not.toHaveBeenCalled();
    expect(applicationModelSettingsApi.confirm).not.toHaveBeenCalled();
  });

  it("ignores a query completion after the dialog has unmounted", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      originalSave: { app: "codex", request, status: "unknown" },
    });
    let complete!: (value: any) => void;
    vi.mocked(applicationModelSettingsApi.query).mockImplementation(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    const view = render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.recheck",
      }),
    );
    view.unmount();
    await act(async () =>
      complete({ app: "codex", request, status: "completed" }),
    );
    expect(props.onSaved).not.toHaveBeenCalled();
    expect(props.onOpenChange).not.toHaveBeenCalled();
    expect(applicationModelSettingsApi.confirm).not.toHaveBeenCalled();
  });
  it("shows the native capability conflict reason without saving", async () => {
    vi.mocked(applicationModelSettingsApi.preview).mockRejectedValue(
      "provider.modelSettings.capabilityConflict",
    );
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.restoreReasoning",
      }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    expect(
      await screen.findByText("applications.modelSettings.capabilityConflict"),
    ).toBeInTheDocument();
    expect(applicationModelSettingsApi.confirm).not.toHaveBeenCalled();
  });
  it("requires source confirmation and explicit Set choices for both model and thinking", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      sourceChanged: true,
      sourceChangeEffects: {
        declarationModels: ["alias-a", "old-owned-model"],
        transport: true,
      },
    } as any);
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("combobox", {
        name: "applications.modelSettings.model",
      }),
    );
    await userEvent.click(screen.getByRole("option", { name: "alias-b" }));
    const review = screen.getByRole("button", {
      name: "applications.modelSettings.review",
    });
    expect(review).toBeDisabled();
    const confirmation = screen.getByRole("checkbox", {
      name: "applications.modelSettings.confirmSourceChange",
    });
    await userEvent.click(confirmation);
    expect(review).toBeDisabled();
    await openReasoning();
    await userEvent.click(
      screen.getByRole("option", {
        name: "applications.modelSettings.serviceDefault",
      }),
    );
    expect(review).toBeEnabled();
    await userEvent.click(confirmation);
    expect(review).toBeDisabled();
    expect(applicationModelSettingsApi.preview).not.toHaveBeenCalled();
  });

  it("shows all native source-reset effects and confirms the narrow source change patch", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      sourceChanged: true,
      sourceChangeEffects: {
        declarationModels: ["alias-a", "old-owned-model"],
        transport: true,
      },
    } as any);
    render(<ApplicationModelSettingsDialog {...props} />);
    const confirmation = await screen.findByRole("checkbox", {
      name: "applications.modelSettings.confirmSourceChange",
    });
    expect(screen.getByText("old-owned-model")).toBeInTheDocument();
    expect(
      screen.getByText(
        "applications.modelSettings.sourceChangeDeclarationFields",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("applications.modelSettings.sourceChangeTransport"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("applications.modelSettings.sourceChangeInherited"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("applications.modelSettings.sourceChangePreserves"),
    ).toBeInTheDocument();
    await userEvent.click(confirmation);
    await userEvent.click(
      screen.getByRole("combobox", {
        name: "applications.modelSettings.model",
      }),
    );
    await userEvent.click(screen.getByRole("option", { name: "alias-a" }));
    await openReasoning();
    await userEvent.click(
      screen.getByRole("option", {
        name: "applications.modelSettings.serviceDefault",
      }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    const expected = {
      confirmSourceChange: true,
      sourceChangeRevision: "b".repeat(64),
      model: { action: "set", value: "alias-a" },
      reasoning: { action: "set", value: { kind: "serviceDefault" } },
    };
    await waitFor(() =>
      expect(applicationModelSettingsApi.preview).toHaveBeenCalledWith(
        "codex",
        "relay-7",
        expected,
        expect.any(String),
      ),
    );
    await userEvent.click(
      await screen.findByRole("button", {
        name: "applications.modelSettings.confirm",
      }),
    );
    await waitFor(() =>
      expect(applicationModelSettingsApi.confirm).toHaveBeenCalledWith(
        "codex",
        "relay-7",
        expected,
        expect.objectContaining({ scope: "providerModelSettings" }),
      ),
    );
  });

  it("does not offer or send source confirmation for an unchanged source", async () => {
    render(<ApplicationModelSettingsDialog {...props} />);
    await screen.findByRole("combobox", {
      name: "applications.modelSettings.model",
    });
    expect(
      screen.queryByRole("checkbox", {
        name: "applications.modelSettings.confirmSourceChange",
      }),
    ).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", {
        name: "applications.modelSettings.restoreReasoning",
      }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    await waitFor(() =>
      expect(applicationModelSettingsApi.preview).toHaveBeenCalledWith(
        "codex",
        "relay-7",
        { reasoning: { action: "restore" } },
        expect.any(String),
      ),
    );
  });

  it("requires a fresh thinking choice after source change with an old unsupported effort", async () => {
    vi.mocked(applicationModelSettingsApi.get).mockResolvedValue({
      ...state(),
      sourceChanged: true,
      reasoningIntent: { kind: "effort", level: "medium" },
      sourceChangeEffects: { declarationModels: [], transport: false },
    });
    render(<ApplicationModelSettingsDialog {...props} />);
    expect(
      await screen.findByRole("combobox", {
        name: "applications.modelSettings.reasoning",
      }),
    ).toHaveTextContent("applications.modelSettings.chooseReasoning");
    await openReasoning();
    expect(screen.getByRole("option", { name: "medium" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(screen.getByRole("option", { name: "medium" })).toHaveAttribute(
      "data-state",
      "unchecked",
    );
    await userEvent.keyboard("{Escape}");
    expect(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    ).toBeDisabled();
  });
  it("reloads source effects and requires fresh choices after a stale source confirmation", async () => {
    const changed = {
      ...state(),
      sourceChanged: true,
      sourceChangeEffects: {
        declarationModels: ["old-owned-model"],
        transport: false,
      },
    };
    vi.mocked(applicationModelSettingsApi.get)
      .mockResolvedValueOnce(changed)
      .mockResolvedValueOnce({
        ...changed,
        revision: "c".repeat(64),
        sourceChangeEffects: {
          declarationModels: ["new-owned-model"],
          transport: true,
        },
      });
    vi.mocked(applicationModelSettingsApi.preview).mockRejectedValue(
      "provider.modelSettings.sourceChanged",
    );
    render(<ApplicationModelSettingsDialog {...props} />);
    await userEvent.click(
      await screen.findByRole("checkbox", {
        name: "applications.modelSettings.confirmSourceChange",
      }),
    );
    await userEvent.click(
      screen.getByRole("combobox", {
        name: "applications.modelSettings.model",
      }),
    );
    await userEvent.click(screen.getByRole("option", { name: "alias-a" }));
    await openReasoning();
    await userEvent.click(
      screen.getByRole("option", {
        name: "applications.modelSettings.serviceDefault",
      }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    );
    await screen.findByText("applications.modelSettings.sourceChanged");
    await userEvent.click(screen.getByRole("button", { name: "common.retry" }));
    await screen.findByText("new-owned-model");
    expect(
      screen.getByRole("checkbox", {
        name: "applications.modelSettings.confirmSourceChange",
      }),
    ).not.toBeChecked();
    expect(
      screen.getByRole("combobox", {
        name: "applications.modelSettings.model",
      }),
    ).toHaveTextContent("applications.modelSettings.chooseModel");
    expect(
      screen.getByRole("combobox", {
        name: "applications.modelSettings.reasoning",
      }),
    ).toHaveTextContent("applications.modelSettings.chooseReasoning");
    expect(
      screen.getByRole("button", { name: "applications.modelSettings.review" }),
    ).toBeDisabled();
    expect(applicationModelSettingsApi.preview).toHaveBeenCalledOnce();
    expect(applicationModelSettingsApi.confirm).not.toHaveBeenCalled();
  });
});
