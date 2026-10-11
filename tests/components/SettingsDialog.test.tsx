import {
  render,
  screen,
  fireEvent,
  waitFor,
  within,
} from "@testing-library/react";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import "@testing-library/jest-dom/vitest";
import {
  createContext,
  useContext,
  useRef,
  useState,
  type ComponentProps,
} from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  SettingsPage,
  type SettingsPageHandle,
} from "@/components/settings/SettingsPage";
import { ConfirmDialog } from "@/components/ConfirmDialog";

const shared = vi.hoisted(() => ({
  get: vi.fn(),
  preview: vi.fn(),
  save: vi.fn(),
  result: vi.fn(),
  getImport: vi.fn(),
  previewImport: vi.fn(),
}));
const claudeShared = vi.hoisted(() => ({
  get: vi.fn(),
  preview: vi.fn(),
  save: vi.fn(),
  result: vi.fn(),
  getImport: vi.fn(),
  previewImport: vi.fn(),
}));
vi.mock("@/lib/api/config", () => ({
  getSharedSettingsApi: (app: string) =>
    app === "claude" ? claudeShared : shared,
  getClaudeSharedSettings: claudeShared.get,
  previewClaudeSharedSettings: claudeShared.preview,
  saveClaudeSharedSettings: claudeShared.save,
  getClaudeSharedSettingsSaveResult: claudeShared.result,
  getClaudeSharedSettingsImport: claudeShared.getImport,
  previewClaudeSharedSettingsImport: claudeShared.previewImport,
  getCodexSharedSettings: shared.get,
  previewCodexSharedSettings: shared.preview,
  saveCodexSharedSettings: shared.save,
  getCodexSharedSettingsSaveResult: shared.result,
  getCodexSharedSettingsImport: shared.getImport,
  previewCodexSharedSettingsImport: shared.previewImport,
}));
vi.mock("@/components/JsonEditor", () => ({
  default: ({
    value,
    onChange,
    readOnly,
    ariaLabel,
    language,
    showFormat,
  }: any) => (
    <textarea
      aria-label={ariaLabel}
      data-language={language}
      data-show-format={String(showFormat)}
      value={value}
      readOnly={readOnly}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));
const sharedRequest = {
  id: "shared-1",
  scope: "codexSharedSettings",
  draftDigest: "digest",
  revision: "revision",
};
const initialSharedConfig = 'approval_policy = "on-request"';
const editedSharedConfig = 'approval_policy = "never"';
const importedSharedConfig =
  'approval_policy = "untrusted"\n[history]\n"max.bytes" = 9007199254740993\n';
const legacyFields = [
  {
    path: ["history", "max.bytes"],
    field: 'history."max.bytes"',
    value: "9007199254740993",
  },
  { path: ["tools", "web_search"], field: "tools.web_search", value: "false" },
];

const toastSuccessMock = vi.fn();
const toastErrorMock = vi.fn();

vi.mock("sonner", () => ({
  toast: {
    success: (...args: unknown[]) => toastSuccessMock(...args),
    error: (...args: unknown[]) => toastErrorMock(...args),
  },
}));

const tMock = vi.fn((key: string) => key);
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("@/hooks/useProxyStatus", () => ({
  useProxyStatus: () => ({
    isRunning: false,
    takeoverStatus: null,
    startProxyServer: vi.fn(),
    stopWithRestore: vi.fn(),
    isPending: false,
  }),
}));

interface SettingsMock {
  settings: any;
  isLoading: boolean;
  isSaving: boolean;
  isPortable: boolean;
  appConfigDir?: string;
  resolvedDirs: Record<string, string>;
  requiresRestart: boolean;
  updateSettings: ReturnType<typeof vi.fn>;
  updateDirectory: ReturnType<typeof vi.fn>;
  updateAppConfigDir: ReturnType<typeof vi.fn>;
  browseDirectory: ReturnType<typeof vi.fn>;
  browseAppConfigDir: ReturnType<typeof vi.fn>;
  resetDirectory: ReturnType<typeof vi.fn>;
  resetAppConfigDir: ReturnType<typeof vi.fn>;
  saveSettings: ReturnType<typeof vi.fn>;
  autoSaveSettings: ReturnType<typeof vi.fn>;
  resetSettings: ReturnType<typeof vi.fn>;
  acknowledgeRestart: ReturnType<typeof vi.fn>;
}

const createSettingsMock = (overrides: Partial<SettingsMock> = {}) => {
  const base: SettingsMock = {
    settings: {
      showInTray: true,
      minimizeToTrayOnClose: true,
      enableClaudePluginIntegration: false,
      language: "zh",
      claudeConfigDir: "/claude",
      codexConfigDir: "/codex",
    },
    isLoading: false,
    isSaving: false,
    isPortable: false,
    appConfigDir: "/app-config",
    resolvedDirs: {
      claude: "/claude",
      codex: "/codex",
    },
    requiresRestart: false,
    updateSettings: vi.fn(),
    updateDirectory: vi.fn(),
    updateAppConfigDir: vi.fn(),
    browseDirectory: vi.fn(),
    browseAppConfigDir: vi.fn(),
    resetDirectory: vi.fn(),
    resetAppConfigDir: vi.fn(),
    saveSettings: vi.fn().mockResolvedValue({ requiresRestart: false }),
    autoSaveSettings: vi.fn().mockResolvedValue({ requiresRestart: false }),
    resetSettings: vi.fn(),
    acknowledgeRestart: vi.fn(),
  };

  return { ...base, ...overrides };
};

interface ImportExportMock {
  selectedFile: string;
  status: string;
  errorMessage: string | null;
  backupId: string | null;
  isImporting: boolean;
  selectImportFile: ReturnType<typeof vi.fn>;
  importConfig: ReturnType<typeof vi.fn>;
  exportConfig: ReturnType<typeof vi.fn>;
  clearSelection: ReturnType<typeof vi.fn>;
  resetStatus: ReturnType<typeof vi.fn>;
}

const createImportExportMock = (overrides: Partial<ImportExportMock> = {}) => {
  const base: ImportExportMock = {
    selectedFile: "",
    status: "idle",
    errorMessage: null,
    backupId: null,
    isImporting: false,
    selectImportFile: vi.fn(),
    importConfig: vi.fn(),
    exportConfig: vi.fn(),
    clearSelection: vi.fn(),
    resetStatus: vi.fn(),
  };

  return { ...base, ...overrides };
};

let settingsMock = createSettingsMock();
let importExportMock = createImportExportMock();
const useImportExportSpy = vi.fn();
let lastUseImportExportOptions: Record<string, unknown> | undefined;

vi.mock("@/hooks/useSettings", () => ({
  useSettings: () => settingsMock,
}));

vi.mock("@/hooks/useImportExport", () => ({
  useImportExport: (options?: Record<string, unknown>) =>
    useImportExportSpy(options),
}));

vi.mock("@/lib/api", () => ({
  settingsApi: {
    restart: vi.fn().mockResolvedValue(true),
  },
  // PlazaSettings 从这里取展示侧默认值；mock 里给真值即可。
  PLAZA_VISIBLE_DEFAULT: true,
}));

const TabsContext = createContext<{
  value: string;
  onValueChange?: (value: string) => void;
}>({
  value: "general",
});

vi.mock("@/components/ui/tabs", () => {
  return {
    Tabs: ({ value, onValueChange, children }: any) => (
      <TabsContext.Provider value={{ value, onValueChange }}>
        <div data-testid="tabs">{children}</div>
      </TabsContext.Provider>
    ),
    TabsList: ({ children }: any) => <div>{children}</div>,
    TabsTrigger: ({ value, children }: any) => {
      const ctx = useContext(TabsContext);
      return (
        <button type="button" onClick={() => ctx.onValueChange?.(value)}>
          {children}
        </button>
      );
    },
    TabsContent: ({ value, children }: any) => {
      const ctx = useContext(TabsContext);
      if (ctx.value !== value) return null;
      return <div data-testid={`tab-${value}`}>{children}</div>;
    },
  };
});

vi.mock("@/components/settings/LanguageSettings", () => ({
  LanguageSettings: ({ value, onChange }: any) => (
    <div>
      <span>language:{value}</span>
      <button onClick={() => onChange("en")}>change-language</button>
    </div>
  ),
}));

vi.mock("@/components/settings/ThemeSettings", () => ({
  ThemeSettings: () => <div>theme-settings</div>,
}));

vi.mock("@/components/settings/WindowSettings", () => ({
  WindowSettings: ({ onChange }: any) => (
    <button onClick={() => onChange({ minimizeToTrayOnClose: false })}>
      window-settings
    </button>
  ),
}));

vi.mock("@/components/settings/DirectorySettings", () => ({
  DirectorySettings: ({
    onBrowseDirectory,
    onResetDirectory,
    onDirectoryChange,
    onBrowseAppConfig,
    onResetAppConfig,
    onAppConfigChange,
  }: any) => (
    <div>
      <button onClick={() => onBrowseDirectory("claude")}>
        browse-directory
      </button>
      <button onClick={() => onResetDirectory("claude")}>
        reset-directory
      </button>
      <button onClick={() => onDirectoryChange("codex", "/new/path")}>
        change-directory
      </button>
      <button onClick={() => onBrowseAppConfig()}>browse-app-config</button>
      <button onClick={() => onResetAppConfig()}>reset-app-config</button>
      <button onClick={() => onAppConfigChange("/app/new")}>
        change-app-config
      </button>
    </div>
  ),
}));

vi.mock("@/components/settings/AboutSection", () => ({
  AboutSection: ({ isPortable }: any) => <div>about:{String(isPortable)}</div>,
}));

vi.mock("@/components/settings/WebdavSyncSection", () => ({
  WebdavSyncSection: ({ config }: any) => (
    <div>webdav-sync-section:{config?.baseUrl ?? "none"}</div>
  ),
}));

let settingsApi: any;

function SharedSettingsGuardHarness(
  props: Partial<ComponentProps<typeof SettingsPage>>,
) {
  const ref = useRef<SettingsPageHandle>(null);
  const [pending, setPending] = useState<(() => void) | null>(null);
  return (
    <>
      <SettingsPage
        ref={ref}
        open
        onOpenChange={vi.fn()}
        beforeLeave={(proceed) => {
          if (ref.current?.submitting) return;
          if (ref.current?.hasPendingChanges) setPending(() => proceed);
          else proceed();
        }}
        {...props}
      />
      <ConfirmDialog
        isOpen={!!pending}
        title="applications.leaveDraftTitle"
        message="applications.leaveDraftDescription"
        cancelText="loongport.tier.editConfirmButton"
        confirmText="applications.discardAndLeave"
        onCancel={() => setPending(null)}
        onConfirm={() => {
          ref.current?.discard();
          const proceed = pending;
          setPending(null);
          proceed?.();
        }}
      />
    </>
  );
}

const renderSettingsPage = (
  props?: Partial<ComponentProps<typeof SettingsPage>>,
) => {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={client}>
      <SharedSettingsGuardHarness {...props} />
    </QueryClientProvider>,
  );
};

describe("SettingsPage Component", () => {
  beforeEach(async () => {
    tMock.mockImplementation((key: string) => key);
    shared.get.mockReset().mockResolvedValue({
      app: "codex",
      status: "ready",
      config: initialSharedConfig,
      originalSave: null,
    });
    shared.preview.mockReset().mockResolvedValue({
      app: "codex",
      status: "ready",
      request: sharedRequest,
      changes: [
        { field: "approval_policy", before: '"on-request"', after: '"never"' },
      ],
      files: [{ role: "codexConfig", change: "write" }],
    });
    shared.getImport.mockReset().mockResolvedValue({
      app: "codex",
      status: "ready",
      fields: legacyFields,
    });
    shared.previewImport.mockReset().mockResolvedValue({
      app: "codex",
      status: "ready",
      request: sharedRequest,
      config: importedSharedConfig,
      changes: [{ field: 'history."max.bytes"', after: "9007199254740993" }],
      files: [{ role: "codexConfig", change: "write" }],
    });
    shared.save.mockReset().mockResolvedValue({
      app: "codex",
      status: "completed",
      request: sharedRequest,
    });
    shared.result.mockReset().mockResolvedValue({
      app: "codex",
      status: "pending",
      request: sharedRequest,
    });
    settingsMock = createSettingsMock();
    importExportMock = createImportExportMock();
    useImportExportSpy.mockReset();
    useImportExportSpy.mockImplementation(
      (options?: Record<string, unknown>) => {
        lastUseImportExportOptions = options;
        return importExportMock;
      },
    );
    lastUseImportExportOptions = undefined;
    toastSuccessMock.mockReset();
    toastErrorMock.mockReset();
    settingsApi = (await import("@/lib/api")).settingsApi;
    settingsApi.restart.mockClear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("opens an independent Codex shared settings editor without saving other settings", async () => {
    renderSettingsPage();
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.title" }),
    );
    expect(
      await screen.findByRole("dialog", { name: "codexSharedSettings.title" }),
    ).toBeInTheDocument();
    expect(settingsMock.saveSettings).not.toHaveBeenCalled();
    expect(settingsMock.autoSaveSettings).not.toHaveBeenCalled();
  });

  const openShared = async () => {
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.title" }),
    );
    return screen.findByRole("textbox", { name: "codexSharedSettings.editor" });
  };
  const previewShared = async () => {
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    fireEvent.change(editor, { target: { value: editedSharedConfig } });
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.preview" }),
    );
    await screen.findByText("approval_policy");
    return editor;
  };

  const openLegacyOptions = async () => {
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.import.open" }),
    );
    return screen.findByRole("checkbox", { name: 'history."max.bytes"' });
  };
  const previewLegacySelection = async () => {
    const option = await openLegacyOptions();
    fireEvent.click(option);
    fireEvent.click(
      screen.getByRole("button", {
        name: "codexSharedSettings.import.preview",
      }),
    );
  };

  it("reads legacy options only after opening them and previews selected original paths before saving", async () => {
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    expect(shared.getImport).not.toHaveBeenCalled();
    const option = await openLegacyOptions();
    expect(option).not.toBeChecked();
    expect(screen.getByText("9007199254740993")).toBeVisible();
    expect(screen.getByText("false")).toBeVisible();
    expect(editor).toHaveValue(initialSharedConfig);
    expect(shared.previewImport).not.toHaveBeenCalled();
    expect(shared.save).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", {
        name: "codexSharedSettings.import.preview",
      }),
    ).toBeDisabled();
    fireEvent.click(option);
    fireEvent.click(
      screen.getByRole("button", {
        name: "codexSharedSettings.import.preview",
      }),
    );
    await waitFor(() => expect(editor).toHaveValue(importedSharedConfig));
    expect(shared.previewImport).toHaveBeenCalledExactlyOnceWith(
      [["history", "max.bytes"]],
      expect.any(String),
    );
    expect(shared.preview).not.toHaveBeenCalled();
    expect(shared.save).not.toHaveBeenCalled();
    expect(screen.getByText("codexSharedSettings.added")).toBeVisible();
    expect(screen.getByText("9007199254740993")).toBeVisible();
    expect(screen.getByText("codexSharedSettings.versionCheck")).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.save" }),
    );
    await screen.findByText("codexSharedSettings.result.completed");
    expect(shared.save).toHaveBeenCalledExactlyOnceWith(
      importedSharedConfig,
      sharedRequest,
    );
    expect(settingsMock.saveSettings).not.toHaveBeenCalled();
    expect(settingsMock.autoSaveSettings).not.toHaveBeenCalled();
  });

  it("keeps unsaved content until the existing guard confirms replacing it with an import preview", async () => {
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    fireEvent.change(editor, { target: { value: editedSharedConfig } });
    await previewLegacySelection();
    await screen.findByText("applications.leaveDraftTitle");
    expect(editor).toHaveValue(editedSharedConfig);
    expect(shared.previewImport).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "loongport.tier.editConfirmButton" }),
    );
    expect(editor).toHaveValue(editedSharedConfig);
    expect(
      screen.getByRole("checkbox", { name: 'history."max.bytes"' }),
    ).toBeChecked();
    fireEvent.click(
      screen.getByRole("button", {
        name: "codexSharedSettings.import.preview",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "applications.discardAndLeave" }),
    );
    await waitFor(() => expect(editor).toHaveValue(importedSharedConfig));
    expect(shared.previewImport).toHaveBeenCalledOnce();
    expect(shared.save).not.toHaveBeenCalled();
  });

  it("cancels legacy selection without changing the existing draft or writing", async () => {
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    fireEvent.change(editor, { target: { value: editedSharedConfig } });
    fireEvent.click(await openLegacyOptions());
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.import.cancel" }),
    );
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(editor).toHaveValue(editedSharedConfig);
    expect(shared.previewImport).not.toHaveBeenCalled();
    expect(shared.save).not.toHaveBeenCalled();
  });

  it.each(["common.cancel", "common.close"])(
    "guards an imported draft on %s without saving",
    async (action) => {
      renderSettingsPage();
      const editor = await openShared();
      await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
      await previewLegacySelection();
      await waitFor(() => expect(editor).toHaveValue(importedSharedConfig));
      fireEvent.click(screen.getByRole("button", { name: action }));
      await screen.findByText("applications.leaveDraftTitle");
      fireEvent.click(
        screen.getByRole("button", {
          name: "loongport.tier.editConfirmButton",
        }),
      );
      expect(editor).toHaveValue(importedSharedConfig);
      fireEvent.click(screen.getByRole("button", { name: action }));
      fireEvent.click(
        screen.getByRole("button", { name: "applications.discardAndLeave" }),
      );
      await waitFor(() =>
        expect(screen.queryByRole("textbox")).not.toBeInTheDocument(),
      );
      expect(shared.save).not.toHaveBeenCalled();
    },
  );

  it("shows when the legacy snippet has no missing ordinary fields", async () => {
    shared.getImport.mockResolvedValue({
      app: "codex",
      status: "ready",
      fields: [],
    });
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.import.open" }),
    );
    await screen.findByText("codexSharedSettings.import.empty");
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(shared.previewImport).not.toHaveBeenCalled();
    expect(shared.save).not.toHaveBeenCalled();
  });

  it.each(["legacyDisabled", "secret-parser-source"])(
    "shows only safe legacy import errors for %s",
    async (code) => {
      shared.getImport.mockResolvedValue({
        app: "codex",
        status: "blocked",
        fields: legacyFields,
        code,
      });
      renderSettingsPage();
      const editor = await openShared();
      await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.import.open" }),
      );
      await screen.findByText(
        `codexSharedSettings.error.${code === "legacyDisabled" ? code : "generic"}`,
      );
      expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
      expect(editor).toHaveValue(initialSharedConfig);
      expect(document.body).not.toHaveTextContent("secret-parser-source");
      expect(shared.previewImport).not.toHaveBeenCalled();
    },
  );

  it("leaves the draft unchanged when the legacy import preview is blocked", async () => {
    shared.previewImport.mockResolvedValue({
      app: "codex",
      status: "blocked",
      config: "",
      request: sharedRequest,
      changes: [],
      files: [],
      code: "sourceChanged",
    });
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    await previewLegacySelection();
    await screen.findByText("codexSharedSettings.error.sourceChanged");
    expect(editor).toHaveValue(initialSharedConfig);
    expect(
      screen.getByRole("button", { name: "codexSharedSettings.save" }),
    ).toBeDisabled();
    expect(shared.save).not.toHaveBeenCalled();
  });

  it("queries an uncertain imported save without starting another import or write", async () => {
    shared.save.mockRejectedValue(new Error("secret-save-source"));
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    await previewLegacySelection();
    await waitFor(() => expect(editor).toHaveValue(importedSharedConfig));
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.save" }),
    );
    await screen.findByText("codexSharedSettings.result.pending");
    expect(shared.result).toHaveBeenCalledExactlyOnceWith(sharedRequest);
    const reopen = screen.getByRole("button", {
      name: "codexSharedSettings.import.open",
    });
    expect(reopen).toBeDisabled();
    fireEvent.click(reopen);
    expect(shared.getImport).toHaveBeenCalledOnce();
    expect(shared.previewImport).toHaveBeenCalledOnce();
    expect(shared.save).toHaveBeenCalledOnce();
  });

  it("allows a later explicitly opened legacy import after a verified save", async () => {
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    await previewLegacySelection();
    await waitFor(() => expect(editor).toHaveValue(importedSharedConfig));
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.save" }),
    );
    await screen.findByText("codexSharedSettings.result.completed");
    expect(shared.getImport).toHaveBeenCalledOnce();
    await openLegacyOptions();
    expect(shared.getImport).toHaveBeenCalledTimes(2);
    expect(shared.save).toHaveBeenCalledOnce();
  });

  it.each(["pending", "verificationRequired", "unknown"])(
    "never starts a legacy import while an original %s save is unresolved",
    async (status) => {
      shared.get.mockResolvedValue({
        app: "codex",
        status: "ready",
        config: initialSharedConfig,
        originalSave: { app: "codex", status, request: sharedRequest },
      });
      renderSettingsPage();
      await openShared();
      await screen.findByText(`codexSharedSettings.result.${status}`);
      const open = screen.getByRole("button", {
        name: "codexSharedSettings.import.open",
      });
      expect(open).toBeDisabled();
      fireEvent.click(open);
      expect(shared.getImport).not.toHaveBeenCalled();
      expect(shared.previewImport).not.toHaveBeenCalled();
    },
  );

  it.each(["getImport", "previewImport"] as const)(
    "keeps a lost %s response read-only and hides its raw error",
    async (command) => {
      shared[command].mockRejectedValue(new Error("secret-parser-source"));
      renderSettingsPage();
      const editor = await openShared();
      await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
      if (command === "getImport") {
        fireEvent.click(
          screen.getByRole("button", {
            name: "codexSharedSettings.import.open",
          }),
        );
      } else {
        await previewLegacySelection();
      }
      await screen.findByText("codexSharedSettings.error.generic");
      expect(editor).toHaveValue(initialSharedConfig);
      expect(
        screen.getByRole("button", { name: "codexSharedSettings.save" }),
      ).toBeDisabled();
      expect(document.body).not.toHaveTextContent("secret-parser-source");
      expect(shared.save).not.toHaveBeenCalled();
      expect(shared.result).not.toHaveBeenCalled();
    },
  );

  it("preserves a newly filled live field returned by the backend instead of restoring the selected legacy value", async () => {
    const latestConfig =
      'approval_policy = "on-request"\n[history]\n"max.bytes" = 42\n';
    shared.previewImport.mockResolvedValue({
      app: "codex",
      status: "ready",
      request: sharedRequest,
      config: latestConfig,
      changes: [],
      files: [{ role: "codexConfig", change: "unchanged" }],
    });
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    await previewLegacySelection();
    await waitFor(() => expect(editor).toHaveValue(latestConfig));
    expect(screen.queryByText("9007199254740993")).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.save" }),
    );
    await screen.findByText("codexSharedSettings.result.completed");
    expect(shared.save).toHaveBeenCalledExactlyOnceWith(
      latestConfig,
      sharedRequest,
    );
  });

  it("requires a fresh field preview and saves only the shared page", async () => {
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    expect(
      screen.getByText("codexSharedSettings.readOnlyFields"),
    ).toBeInTheDocument();
    const save = screen.getByRole("button", {
      name: "codexSharedSettings.save",
    });
    expect(save).toBeDisabled();
    fireEvent.change(editor, { target: { value: editedSharedConfig } });
    expect(save).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.preview" }),
    );
    await screen.findByText("approval_policy");
    expect(
      screen.getByText("codexSharedSettings.versionCheck"),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("codexSharedSettings.result.completed"),
    ).not.toBeInTheDocument();
    fireEvent.click(save);
    await screen.findByText("codexSharedSettings.result.completed");
    expect(shared.save).toHaveBeenCalledExactlyOnceWith(
      editedSharedConfig,
      sharedRequest,
    );
    expect(settingsMock.saveSettings).not.toHaveBeenCalled();
    expect(settingsMock.autoSaveSettings).not.toHaveBeenCalled();
  });

  it("shows the actual before and after values and identifies additions and removals", async () => {
    shared.preview.mockResolvedValue({
      app: "codex",
      status: "ready",
      request: sharedRequest,
      changes: [
        { field: "approval_policy", before: '"on-request"', after: '"never"' },
        { field: "tools.web_search", after: "false" },
        { field: "history.max_bytes", before: "9007199254740993" },
      ],
      files: [{ role: "codexConfig", change: "write" }],
    });
    renderSettingsPage();
    await previewShared();
    const modified = within(screen.getByText("approval_policy").closest("li")!);
    expect(modified.getByText('"on-request"')).toBeVisible();
    expect(modified.getByText('"never"')).toBeVisible();
    expect(modified.getByText("codexSharedSettings.modified")).toBeVisible();
    expect(modified.getByText("codexSharedSettings.before")).toBeVisible();
    expect(modified.getByText("codexSharedSettings.after")).toBeVisible();
    const added = within(screen.getByText("tools.web_search").closest("li")!);
    expect(added.getByText("codexSharedSettings.added")).toBeVisible();
    expect(added.getByText("false")).toBeVisible();
    expect(
      added.queryByText("codexSharedSettings.before"),
    ).not.toBeInTheDocument();
    const removed = within(
      screen.getByText("history.max_bytes").closest("li")!,
    );
    expect(removed.getByText("codexSharedSettings.removed")).toBeVisible();
    expect(removed.getByText("9007199254740993")).toBeVisible();
    expect(
      removed.queryByText("codexSharedSettings.after"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("codexSharedSettings.result.completed"),
    ).not.toBeInTheDocument();
  });

  it("invalidates the preview when the draft changes", async () => {
    renderSettingsPage();
    const editor = await previewShared();
    fireEvent.change(editor, {
      target: { value: 'approval_policy = "untrusted"' },
    });
    expect(
      screen.getByRole("button", { name: "codexSharedSettings.save" }),
    ).toBeDisabled();
    expect(screen.queryByText("approval_policy")).not.toBeInTheDocument();
  });

  it("keeps failed reads non-writable without displaying raw errors", async () => {
    shared.get.mockRejectedValue(new Error("secret-parser-source"));
    renderSettingsPage();
    await openShared();
    await screen.findByText("codexSharedSettings.error.generic");
    expect(screen.getByRole("textbox")).toHaveAttribute("readonly");
    expect(
      screen.getByRole("button", { name: "codexSharedSettings.preview" }),
    ).toBeDisabled();
    expect(document.body).not.toHaveTextContent("secret-parser-source");
  });

  it.each([
    "readOnlyFields",
    "invalidFormat",
    "sourceChanged",
    "ownerUnavailable",
    "vaultLocked",
    "pendingOperation",
  ])("shows a safe %s failure and never enables save", async (code) => {
    shared.preview.mockResolvedValue({
      app: "codex",
      status: "blocked",
      request: sharedRequest,
      changes: [],
      files: [],
      code,
    });
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    fireEvent.change(editor, { target: { value: editedSharedConfig } });
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.preview" }),
    );
    await screen.findByText(`codexSharedSettings.error.${code}`);
    expect(
      screen.getByRole("button", { name: "codexSharedSettings.save" }),
    ).toBeDisabled();
  });

  it("queries the same request after an uncertain save rather than repeating the write", async () => {
    shared.save.mockRejectedValue(new Error("secret-save-source"));
    renderSettingsPage();
    await previewShared();
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.save" }),
    );
    await screen.findByText("codexSharedSettings.result.pending");
    expect(shared.result).toHaveBeenCalledExactlyOnceWith(sharedRequest);
    expect(shared.save).toHaveBeenCalledTimes(1);
    expect(
      screen.getByRole("button", { name: "codexSharedSettings.preview" }),
    ).toBeDisabled();
    shared.result.mockResolvedValue({
      app: "codex",
      status: "completed",
      request: sharedRequest,
    });
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.checkResult" }),
    );
    await screen.findByText("codexSharedSettings.result.completed");
    expect(shared.result).toHaveBeenLastCalledWith(sharedRequest);
    expect(shared.save).toHaveBeenCalledTimes(1);
    expect(document.body).not.toHaveTextContent("secret-save-source");
  });

  it("recovers an original pending request without constructing a replacement", async () => {
    shared.get.mockResolvedValue({
      app: "codex",
      status: "ready",
      config: initialSharedConfig,
      originalSave: {
        app: "codex",
        status: "verificationRequired",
        request: sharedRequest,
      },
    });
    renderSettingsPage();
    await openShared();
    await screen.findByText("codexSharedSettings.result.verificationRequired");
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.checkResult" }),
    );
    await waitFor(() =>
      expect(shared.result).toHaveBeenCalledExactlyOnceWith(sharedRequest),
    );
    expect(shared.preview).not.toHaveBeenCalled();
    expect(shared.save).not.toHaveBeenCalled();
  });

  it("requires a new read after resolving a save from a previous session", async () => {
    shared.get.mockResolvedValue({
      app: "codex",
      status: "ready",
      config: initialSharedConfig,
      originalSave: { app: "codex", status: "pending", request: sharedRequest },
    });
    shared.result.mockResolvedValue({
      app: "codex",
      status: "completed",
      request: sharedRequest,
    });
    renderSettingsPage();
    const editor = await openShared();
    await screen.findByText("codexSharedSettings.result.pending");
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.checkResult" }),
    );
    await screen.findByText("codexSharedSettings.result.completed");
    expect(editor).toHaveAttribute("readonly");
    shared.get.mockResolvedValue({
      app: "codex",
      status: "ready",
      config: editedSharedConfig,
      originalSave: null,
    });
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.reload" }),
    );
    await waitFor(() => expect(editor).toHaveValue(editedSharedConfig));
    expect(editor).not.toHaveAttribute("readonly");
  });

  it("requires keeping or discarding a draft before leaving the shared editor", async () => {
    renderSettingsPage();
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    fireEvent.change(editor, { target: { value: editedSharedConfig } });
    fireEvent.click(screen.getByRole("button", { name: "common.close" }));
    await screen.findByText("applications.leaveDraftTitle");
    fireEvent.click(
      screen.getByRole("button", { name: "loongport.tier.editConfirmButton" }),
    );
    expect(editor).toHaveValue(editedSharedConfig);
    fireEvent.click(screen.getByRole("button", { name: "common.close" }));
    fireEvent.click(
      screen.getByRole("button", { name: "applications.discardAndLeave" }),
    );
    await waitFor(() =>
      expect(screen.queryByRole("textbox")).not.toBeInTheDocument(),
    );
    expect(shared.save).not.toHaveBeenCalled();
  });

  it("routes MCP management through the existing callback after discarding a draft", async () => {
    const onManageCodexMcp = vi.fn();
    renderSettingsPage({ onManageCodexMcp });
    const editor = await openShared();
    await waitFor(() => expect(editor).toHaveValue(initialSharedConfig));
    fireEvent.change(editor, { target: { value: editedSharedConfig } });
    fireEvent.click(
      screen.getByRole("button", { name: "codexSharedSettings.manageMcp" }),
    );
    expect(onManageCodexMcp).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "applications.discardAndLeave" }),
    );
    expect(onManageCodexMcp).toHaveBeenCalledOnce();
  });

  describe("Claude shared settings", () => {
    const request = {
      id: "claude-shared-1",
      scope: "claudeSharedSettings",
      draftDigest: "claude-digest",
      revision: "claude-revision",
    };
    const initial = '{"permissions":{"defaultMode":"default"}}';
    const edited =
      '{"permissions":{"defaultMode":"plan"},"large":9007199254740993}';
    const imported =
      '{"permissions":{"defaultMode":"default"},"ui":{"literal.key":9007199254740993}}';
    beforeEach(() => {
      claudeShared.get.mockReset().mockResolvedValue({
        app: "claude",
        status: "ready",
        config: initial,
        originalSave: null,
      });
      claudeShared.preview.mockReset().mockResolvedValue({
        app: "claude",
        status: "ready",
        request,
        changes: [{ field: "large", after: "9007199254740993" }],
        files: [{ role: "claudeSettings", change: "write" }],
      });
      claudeShared.save
        .mockReset()
        .mockResolvedValue({ app: "claude", request, status: "completed" });
      claudeShared.result
        .mockReset()
        .mockResolvedValue({ app: "claude", request, status: "pending" });
      claudeShared.getImport.mockReset().mockResolvedValue({
        app: "claude",
        status: "ready",
        fields: [
          {
            path: ["ui", "literal.key"],
            field: 'ui."literal.key"',
            value: "9007199254740993",
          },
        ],
      });
      claudeShared.previewImport.mockReset().mockResolvedValue({
        app: "claude",
        status: "ready",
        request,
        config: imported,
        changes: [{ field: 'ui."literal.key"', after: "9007199254740993" }],
        files: [{ role: "claudeSettings", change: "write" }],
      });
    });
    async function openClaude() {
      fireEvent.click(
        screen.getByRole("button", { name: "claudeSharedSettings.title" }),
      );
      const editor = await screen.findByRole("textbox", {
        name: "claudeSharedSettings.editor",
      });
      await waitFor(() => expect(editor).toHaveValue(initial));
      return editor;
    }
    async function selectImport() {
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.import.open" }),
      );
      const option = await screen.findByRole("checkbox", {
        name: 'ui."literal.key"',
      });
      expect(option).not.toBeChecked();
      fireEvent.click(option);
      fireEvent.click(
        screen.getByRole("button", {
          name: "codexSharedSettings.import.preview",
        }),
      );
    }
    async function previewClaude() {
      const editor = await openClaude();
      fireEvent.change(editor, { target: { value: edited } });
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.preview" }),
      );
      return editor;
    }
    it("opens the real Claude settings entry and submits exact JSON only through Claude commands", async () => {
      renderSettingsPage();
      const editor = await previewClaude();
      expect(editor).toHaveAttribute("data-language", "json");
      expect(editor).toHaveAttribute("data-show-format", "false");
      await screen.findByText("9007199254740993");
      expect(claudeShared.preview).toHaveBeenCalledExactlyOnceWith(
        edited,
        expect.any(String),
      );
      expect(claudeShared.save).not.toHaveBeenCalled();
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.save" }),
      );
      await screen.findByText("codexSharedSettings.result.completed");
      expect(claudeShared.save).toHaveBeenCalledExactlyOnceWith(
        edited,
        request,
      );
      expect(editor).toHaveValue(edited);
      expect(shared.get).not.toHaveBeenCalled();
      expect(shared.save).not.toHaveBeenCalled();
      expect(settingsMock.saveSettings).not.toHaveBeenCalled();
      expect(settingsMock.autoSaveSettings).not.toHaveBeenCalled();
    });
    it("imports selected exact missing paths only explicitly and allows another manual import after saving", async () => {
      renderSettingsPage();
      const editor = await openClaude();
      expect(claudeShared.getImport).not.toHaveBeenCalled();
      await selectImport();
      await waitFor(() => expect(editor).toHaveValue(imported));
      expect(claudeShared.previewImport).toHaveBeenCalledExactlyOnceWith(
        [["ui", "literal.key"]],
        expect.any(String),
      );
      expect(claudeShared.preview).not.toHaveBeenCalled();
      expect(claudeShared.save).not.toHaveBeenCalled();
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.save" }),
      );
      await screen.findByText("codexSharedSettings.result.completed");
      expect(claudeShared.save).toHaveBeenCalledExactlyOnceWith(
        imported,
        request,
      );
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.import.open" }),
      );
      await screen.findByRole("checkbox", { name: 'ui."literal.key"' });
      expect(claudeShared.getImport).toHaveBeenCalledTimes(2);
      expect(claudeShared.save).toHaveBeenCalledOnce();
    });
    it("keeps the Claude draft and selected import on guard cancellation before explicit replacement", async () => {
      renderSettingsPage();
      const editor = await openClaude();
      fireEvent.change(editor, { target: { value: edited } });
      await selectImport();
      await screen.findByText("applications.leaveDraftTitle");
      expect(editor).toHaveValue(edited);
      expect(claudeShared.previewImport).not.toHaveBeenCalled();
      fireEvent.click(
        screen.getByRole("button", {
          name: "loongport.tier.editConfirmButton",
        }),
      );
      expect(editor).toHaveValue(edited);
      expect(
        screen.getByRole("checkbox", { name: 'ui."literal.key"' }),
      ).toBeChecked();
      fireEvent.click(
        screen.getByRole("button", {
          name: "codexSharedSettings.import.preview",
        }),
      );
      fireEvent.click(
        screen.getByRole("button", { name: "applications.discardAndLeave" }),
      );
      await waitFor(() => expect(editor).toHaveValue(imported));
      expect(claudeShared.previewImport).toHaveBeenCalledOnce();
      expect(claudeShared.save).not.toHaveBeenCalled();
    });
    it.each(["invalidFormat", "sourceChanged", "secret-parser-source"])(
      "shows a safe Claude %s preview failure and keeps its draft",
      async (code) => {
        claudeShared.preview.mockResolvedValue({
          app: "claude",
          status: "blocked",
          request,
          changes: [],
          files: [],
          code,
        });
        renderSettingsPage();
        const editor = await previewClaude();
        const key =
          code === "invalidFormat"
            ? "claudeSharedSettings.invalidFormat"
            : `codexSharedSettings.error.${code === "sourceChanged" ? code : "generic"}`;
        await screen.findByText(key);
        expect(editor).toHaveValue(edited);
        expect(
          screen.getByRole("button", { name: "codexSharedSettings.save" }),
        ).toBeDisabled();
        expect(document.body).not.toHaveTextContent("secret-parser-source");
        expect(claudeShared.save).not.toHaveBeenCalled();
      },
    );
    it("queries the exact unknown Claude save and blocks repeated writes and imports", async () => {
      claudeShared.save.mockRejectedValue(new Error("secret-save-source"));
      claudeShared.result.mockRejectedValue(new Error("secret-result-source"));
      renderSettingsPage();
      await previewClaude();
      await screen.findByText("9007199254740993");
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.save" }),
      );
      await screen.findByText("codexSharedSettings.result.unknown");
      expect(claudeShared.result).toHaveBeenCalledExactlyOnceWith(request);
      expect(
        screen.getByRole("button", { name: "codexSharedSettings.import.open" }),
      ).toBeDisabled();
      expect(
        screen.getByRole("button", { name: "codexSharedSettings.preview" }),
      ).toBeDisabled();
      expect(
        screen.getByRole("button", { name: "codexSharedSettings.save" }),
      ).toBeDisabled();
      claudeShared.result.mockResolvedValue({
        app: "claude",
        request,
        status: "completed",
      });
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.checkResult" }),
      );
      await screen.findByText("codexSharedSettings.result.completed");
      expect(claudeShared.result).toHaveBeenLastCalledWith(request);
      expect(claudeShared.save).toHaveBeenCalledOnce();
      expect(shared.result).not.toHaveBeenCalled();
      expect(document.body).not.toHaveTextContent("secret-save-source");
      expect(document.body).not.toHaveTextContent("secret-result-source");
    });
    it("requires a fresh Claude read after resolving an original save from a previous session", async () => {
      claudeShared.get.mockResolvedValue({
        app: "claude",
        status: "ready",
        config: initial,
        originalSave: {
          app: "claude",
          request,
          status: "verificationRequired",
        },
      });
      claudeShared.result.mockResolvedValue({
        app: "claude",
        request,
        status: "completed",
      });
      renderSettingsPage();
      const editor = await openClaude();
      expect(editor).toHaveAttribute("readonly");
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.checkResult" }),
      );
      await screen.findByText("codexSharedSettings.result.completed");
      expect(editor).toHaveAttribute("readonly");
      expect(claudeShared.result).toHaveBeenCalledExactlyOnceWith(request);
      expect(claudeShared.preview).not.toHaveBeenCalled();
      expect(claudeShared.save).not.toHaveBeenCalled();
      fireEvent.click(
        screen.getByRole("button", { name: "codexSharedSettings.reload" }),
      );
      await waitFor(() => expect(claudeShared.get).toHaveBeenCalledTimes(2));
    });
  });

  it("should not render form content when loading", () => {
    settingsMock = createSettingsMock({ settings: null, isLoading: true });

    renderSettingsPage();

    expect(screen.queryByText("language:zh")).not.toBeInTheDocument();
    // 加载状态下显示 spinner 而不是表单内容
    expect(document.querySelector(".animate-spin")).toBeInTheDocument();
  });

  it("should reset import/export status when dialog transitions to open", () => {
    const client = new QueryClient({
      defaultOptions: {
        queries: { retry: false },
      },
    });
    const { rerender } = render(
      <QueryClientProvider client={client}>
        <SettingsPage
          open={false}
          onOpenChange={vi.fn()}
          beforeLeave={(proceed) => proceed()}
        />
      </QueryClientProvider>,
    );

    importExportMock.resetStatus.mockClear();

    rerender(
      <QueryClientProvider client={client}>
        <SettingsPage
          open={true}
          onOpenChange={vi.fn()}
          beforeLeave={(proceed) => proceed()}
        />
      </QueryClientProvider>,
    );

    expect(importExportMock.resetStatus).toHaveBeenCalledTimes(1);
  });

  it("should render general and advanced tabs and trigger child callbacks", () => {
    const onOpenChange = vi.fn();
    // 设置 selectedFile 后，按钮显示 settings.import（可执行导入）
    importExportMock = createImportExportMock({
      selectedFile: "/tmp/config.json",
    });

    renderSettingsPage({ onOpenChange });

    expect(screen.getByText("language:zh")).toBeInTheDocument();
    expect(screen.getByText("theme-settings")).toBeInTheDocument();

    fireEvent.click(screen.getByText("change-language"));
    expect(settingsMock.updateSettings).toHaveBeenCalledWith({
      language: "en",
    });

    fireEvent.click(screen.getByText("window-settings"));
    expect(settingsMock.updateSettings).toHaveBeenCalledWith({
      minimizeToTrayOnClose: false,
    });

    fireEvent.click(screen.getByText("settings.tabAdvanced"));
    fireEvent.click(screen.getByText("settings.advanced.cloudSync.title"));
    expect(screen.getByText("webdav-sync-section:none")).toBeInTheDocument();
    fireEvent.click(screen.getByText("settings.advanced.data.title"));

    // 有文件时，点击导入按钮执行 importConfig
    fireEvent.click(screen.getByRole("button", { name: /settings\.import/ }));
    expect(importExportMock.importConfig).toHaveBeenCalled();

    fireEvent.click(
      screen.getByRole("button", { name: "settings.exportConfig" }),
    );
    expect(importExportMock.exportConfig).toHaveBeenCalled();

    // 清除选择按钮
    fireEvent.click(screen.getByRole("button", { name: "common.clear" }));
    expect(importExportMock.clearSelection).toHaveBeenCalled();
  });

  it("should reset tab content scroll position when switching settings tabs", () => {
    const { container } = renderSettingsPage();
    const scrollContainer = container.querySelector(
      ".overflow-y-auto",
    ) as HTMLDivElement | null;

    expect(scrollContainer).not.toBeNull();

    scrollContainer!.scrollTop = 640;
    fireEvent.click(screen.getByText("settings.tabAdvanced"));

    expect(scrollContainer!.scrollTop).toBe(0);
  });

  it("should pass onImportSuccess callback to useImportExport hook", async () => {
    const onImportSuccess = vi.fn();

    renderSettingsPage({ onImportSuccess });

    expect(useImportExportSpy).toHaveBeenCalledWith(
      expect.objectContaining({ onImportSuccess }),
    );
    expect(lastUseImportExportOptions?.onImportSuccess).toBe(onImportSuccess);

    if (typeof lastUseImportExportOptions?.onImportSuccess === "function") {
      await lastUseImportExportOptions.onImportSuccess();
    }
    expect(onImportSuccess).toHaveBeenCalledTimes(1);
  });

  it("should call saveSettings and close dialog when clicking save", async () => {
    const onOpenChange = vi.fn();
    importExportMock = createImportExportMock();

    renderSettingsPage({ onOpenChange });

    // 保存按钮在 advanced tab 中
    fireEvent.click(screen.getByText("settings.tabAdvanced"));
    fireEvent.click(screen.getByRole("button", { name: /common\.save/ }));

    await waitFor(() => {
      expect(settingsMock.saveSettings).toHaveBeenCalledTimes(1);
      expect(importExportMock.clearSelection).toHaveBeenCalledTimes(1);
      expect(importExportMock.resetStatus).toHaveBeenCalledTimes(2);
      expect(settingsMock.acknowledgeRestart).toHaveBeenCalledTimes(1);
      expect(onOpenChange).toHaveBeenCalledWith(false);
    });
  });

  it("should show restart prompt and allow immediate restart after save", async () => {
    settingsMock = createSettingsMock({
      requiresRestart: true,
      saveSettings: vi.fn().mockResolvedValue({ requiresRestart: true }),
    });

    renderSettingsPage();

    expect(
      await screen.findByText("settings.restartRequired"),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByText("settings.restartNow"));

    await waitFor(() => {
      expect(toastSuccessMock).toHaveBeenCalledWith(
        "settings.devModeRestartHint",
        expect.objectContaining({ closeButton: true }),
      );
    });
  });

  it("should allow postponing restart and close dialog without restarting", async () => {
    const onOpenChange = vi.fn();
    settingsMock = createSettingsMock({ requiresRestart: true });

    renderSettingsPage({ onOpenChange });

    expect(
      await screen.findByText("settings.restartRequired"),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByText("settings.restartLater"));

    await waitFor(() => {
      expect(onOpenChange).toHaveBeenCalledWith(false);
      expect(settingsMock.acknowledgeRestart).toHaveBeenCalledTimes(1);
    });

    expect(settingsApi.restart).not.toHaveBeenCalled();
    expect(toastSuccessMock).not.toHaveBeenCalled();
    expect(toastErrorMock).not.toHaveBeenCalled();
  });

  it("should trigger directory management callbacks inside advanced tab", () => {
    renderSettingsPage();

    fireEvent.click(screen.getByText("settings.tabAdvanced"));
    fireEvent.click(screen.getByText("settings.advanced.configDir.title"));

    fireEvent.click(screen.getByText("browse-directory"));
    expect(settingsMock.browseDirectory).toHaveBeenCalledWith("claude");

    fireEvent.click(screen.getByText("reset-directory"));
    expect(settingsMock.resetDirectory).toHaveBeenCalledWith("claude");

    fireEvent.click(screen.getByText("change-directory"));
    expect(settingsMock.updateDirectory).toHaveBeenCalledWith(
      "codex",
      "/new/path",
    );

    fireEvent.click(screen.getByText("browse-app-config"));
    expect(settingsMock.browseAppConfigDir).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByText("reset-app-config"));
    expect(settingsMock.resetAppConfigDir).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByText("change-app-config"));
    expect(settingsMock.updateAppConfigDir).toHaveBeenCalledWith("/app/new");
  });
});
