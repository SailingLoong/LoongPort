import {
  act,
  render,
  screen,
  fireEvent,
  waitFor,
} from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { WebdavSyncSection } from "@/components/settings/WebdavSyncSection";
import { invoke } from "@tauri-apps/api/core";
import type {
  WebDavSyncSettings,
  S3SyncSettings,
  RemoteSnapshotInfo,
} from "@/types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const toastSuccessMock = vi.fn();
const toastErrorMock = vi.fn();
const toastWarningMock = vi.fn();
const toastInfoMock = vi.fn();

vi.mock("sonner", () => ({
  toast: {
    success: (...args: unknown[]) => toastSuccessMock(...args),
    error: (...args: unknown[]) => toastErrorMock(...args),
    warning: (...args: unknown[]) => toastWarningMock(...args),
    info: (...args: unknown[]) => toastInfoMock(...args),
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
  }),
}));

vi.mock("@/components/ui/button", () => ({
  Button: ({ children, ...props }: any) => (
    <button {...props}>{children}</button>
  ),
}));

vi.mock("@/components/ui/input", () => ({
  Input: (props: any) => <input {...props} />,
}));

vi.mock("@/components/ui/switch", () => ({
  Switch: ({ checked, onCheckedChange, ...props }: any) => (
    <button
      role="switch"
      aria-checked={checked}
      onClick={() => onCheckedChange?.(!checked)}
      {...props}
    />
  ),
}));

vi.mock("@/components/ui/select", () => ({
  Select: ({ value, onValueChange, children }: any) => (
    <select
      data-testid="webdav-preset-select"
      value={value}
      onChange={(event) => onValueChange?.(event.target.value)}
    >
      {children}
    </select>
  ),
  SelectTrigger: ({ children }: any) => <>{children}</>,
  SelectValue: () => null,
  SelectContent: ({ children }: any) => <>{children}</>,
  SelectItem: ({ value, children }: any) => (
    <option value={value}>{children}</option>
  ),
}));

vi.mock("@/components/ui/dialog", () => ({
  Dialog: ({ open, children }: any) => (open ? <div>{children}</div> : null),
  DialogContent: ({ children }: any) => <div>{children}</div>,
  DialogDescription: ({ children }: any) => <div>{children}</div>,
  DialogFooter: ({ children }: any) => <div>{children}</div>,
  DialogHeader: ({ children }: any) => <div>{children}</div>,
  DialogTitle: ({ children }: any) => <h2>{children}</h2>,
}));

const { settingsApiMock } = vi.hoisted(() => ({
  settingsApiMock: {
    webdavTestConnection: vi.fn(),
    webdavSyncSaveSettings: vi.fn(),
    webdavSyncFetchRemoteInfo: vi.fn(),
    webdavSyncUpload: vi.fn(),
    webdavSyncDownload: vi.fn(),
    webdavSyncRestore: vi.fn(),
    s3SyncFetchRemoteInfo: vi.fn(),
    s3SyncDownload: vi.fn(),
    s3SyncUpload: vi.fn(),
    s3SyncRestore: vi.fn(),
  },
}));

vi.mock("@/lib/api", () => ({
  settingsApi: settingsApiMock,
}));

const baseConfig: WebDavSyncSettings = {
  enabled: true,
  baseUrl: "https://dav.example.com/dav/",
  username: "alice",
  password: "secret",
  remoteRoot: "cc-switch-sync",
  profile: "default",
  autoSync: false,
  status: {},
};

function renderSection(config?: WebDavSyncSettings, s3Config?: S3SyncSettings) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const view = render(
    <QueryClientProvider client={client}>
      <WebdavSyncSection config={config} s3Config={s3Config} />
    </QueryClientProvider>,
  );
  return { ...view, client };
}

describe("WebdavSyncSection", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset().mockResolvedValue(null);
    toastSuccessMock.mockReset();
    toastErrorMock.mockReset();
    toastWarningMock.mockReset();
    toastInfoMock.mockReset();
    settingsApiMock.webdavTestConnection.mockReset();
    settingsApiMock.webdavSyncSaveSettings.mockReset();
    settingsApiMock.webdavSyncFetchRemoteInfo.mockReset();
    settingsApiMock.webdavSyncUpload.mockReset();
    settingsApiMock.webdavSyncDownload.mockReset();
    settingsApiMock.webdavSyncRestore.mockReset();
    settingsApiMock.s3SyncFetchRemoteInfo.mockReset();
    settingsApiMock.s3SyncDownload.mockReset();
    settingsApiMock.s3SyncUpload.mockReset();
    settingsApiMock.s3SyncRestore.mockReset();

    settingsApiMock.webdavSyncSaveSettings.mockResolvedValue({ success: true });
    settingsApiMock.webdavTestConnection.mockResolvedValue({
      success: true,
      message: "ok",
    });
    settingsApiMock.webdavSyncFetchRemoteInfo.mockResolvedValue({
      deviceName: "My MacBook",
      createdAt: "2026-02-01T10:00:00Z",
      snapshotId: "snapshot-1",
      version: 2,
      compatible: true,
      artifacts: ["db.sql", "skills.zip"],
    });
    settingsApiMock.webdavSyncUpload.mockResolvedValue({ status: "uploaded" });
    settingsApiMock.webdavSyncDownload.mockResolvedValue({
      status: "downloaded",
    });
  });

  it.each([
    ["webdav", "upload"],
    ["webdav", "download"],
    ["s3", "upload"],
    ["s3", "download"],
  ] as const)(
    "explains the real upgrade pause for %s %s without retry or restore",
    async (transport, action) => {
      const operation =
        transport === "webdav"
          ? action === "upload"
            ? settingsApiMock.webdavSyncUpload
            : settingsApiMock.webdavSyncDownload
          : action === "upload"
            ? settingsApiMock.s3SyncUpload
            : settingsApiMock.s3SyncDownload;
      operation.mockRejectedValue("配置错误: upgrade.sync_paused");
      settingsApiMock.s3SyncFetchRemoteInfo.mockResolvedValue({
        compatible: true,
        snapshotId: "synthetic-snapshot",
        version: 2,
        createdAt: "2026-02-01T10:00:00Z",
        artifacts: ["db.sql"],
      });
      const view = renderSection(
        transport === "webdav" ? baseConfig : undefined,
        transport === "s3"
          ? {
              enabled: true,
              bucket: "synthetic-bucket",
              region: "test",
              accessKeyId: "synthetic",
              secretAccessKey: "synthetic",
              status: {},
            }
          : undefined,
      );
      const prefix = `settings.${transport}Sync`;
      fireEvent.click(
        screen.getByRole("button", { name: `${prefix}.${action}` }),
      );
      const confirm = await screen.findByRole("button", {
        name: `${prefix}.confirm${action === "upload" ? "Upload" : "Download"}.confirm`,
      });
      fireEvent.click(confirm);
      await waitFor(() =>
        expect(toastErrorMock).toHaveBeenCalledWith(
          "startupUpgrade.syncPausedTitle",
          expect.objectContaining({
            description: expect.stringContaining(
              "startupUpgrade.syncPausedReason",
            ),
          }),
        ),
      );
      // The retired manual-protection settings page is no longer a destination.
      // Keep the real pause explanation without sending users to that old route.
      expect(toastErrorMock.mock.lastCall?.[1].description).not.toContain(
        "startupUpgrade.syncReviewHint",
      );
      expect(operation).toHaveBeenCalledTimes(1);
      expect(settingsApiMock.webdavSyncRestore).not.toHaveBeenCalled();
      expect(settingsApiMock.s3SyncRestore).not.toHaveBeenCalled();
      expect(settingsApiMock.webdavSyncSaveSettings).not.toHaveBeenCalled();
      expect(
        screen.queryByLabelText("syncRestore.password"),
      ).not.toBeInTheDocument();
      // Reopening then cancelling a fresh confirmation must not repeat the write.
      view.unmount();
      renderSection(
        transport === "webdav" ? baseConfig : undefined,
        transport === "s3"
          ? {
              enabled: true,
              bucket: "synthetic-bucket",
              region: "test",
              accessKeyId: "synthetic",
              secretAccessKey: "synthetic",
              status: {},
            }
          : undefined,
      );
      fireEvent.click(
        screen.getByRole("button", { name: `${prefix}.${action}` }),
      );
      await screen.findByRole("button", {
        name: `${prefix}.confirm${action === "upload" ? "Upload" : "Download"}.confirm`,
      });
      fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
      expect(operation).toHaveBeenCalledTimes(1);
    },
  );

  it.each(["network timeout", "配置错误: upgrade.sync_paused_extra"])(
    "keeps ordinary upload errors on the original path: %s",
    async (message) => {
      settingsApiMock.webdavSyncUpload.mockRejectedValue(new Error(message));
      renderSection(baseConfig);
      fireEvent.click(
        screen.getByRole("button", { name: "settings.webdavSync.upload" }),
      );
      fireEvent.click(
        await screen.findByRole("button", {
          name: "settings.webdavSync.confirmUpload.confirm",
        }),
      );
      await waitFor(() =>
        expect(toastErrorMock).toHaveBeenCalledWith(
          "settings.webdavSync.uploadFailed",
        ),
      );
      expect(toastErrorMock).not.toHaveBeenCalledWith(
        "startupUpgrade.syncPausedTitle",
        expect.anything(),
      );
    },
  );

  it.each(["webdav", "s3"] as const)(
    "offers %s source restore for plaintext only after reviewing a compatible snapshot",
    async (transport) => {
      const api =
        transport === "webdav"
          ? {
              info: settingsApiMock.webdavSyncFetchRemoteInfo,
              download: settingsApiMock.webdavSyncDownload,
              restore: settingsApiMock.webdavSyncRestore,
            }
          : {
              info: settingsApiMock.s3SyncFetchRemoteInfo,
              download: settingsApiMock.s3SyncDownload,
              restore: settingsApiMock.s3SyncRestore,
            };
      api.info.mockResolvedValue({
        deviceName: "Source",
        snapshotId: "reviewed-original",
        compatible: true,
        artifacts: [],
        version: 2,
      });
      api.download.mockRejectedValue(
        "配置错误: protection.encryption_required",
      );
      api.restore.mockResolvedValue({ status: "restored" });
      const { client } = renderSection(
        transport === "webdav" ? baseConfig : undefined,
        transport === "s3"
          ? {
              enabled: true,
              bucket: "synthetic",
              region: "test",
              accessKeyId: "synthetic",
              secretAccessKey: "synthetic",
              status: {},
            }
          : undefined,
      );
      const prefix = `settings.${transport === "webdav" ? "webdavSync" : "s3Sync"}`;
      fireEvent.click(
        screen.getByRole("button", { name: `${prefix}.download` }),
      );
      fireEvent.click(
        await screen.findByRole("button", {
          name: `${prefix}.confirmDownload.confirm`,
        }),
      );
      const field = await screen.findByLabelText("syncRestore.password");
      expect(api.restore).not.toHaveBeenCalled();
      const raw = "\r\n中文🔑 aB\rline\n ";
      fireEvent.paste(field, { clipboardData: { getData: () => raw } });
      fireEvent.click(
        screen.getByRole("button", { name: "syncRestore.confirm" }),
      );
      await waitFor(() =>
        expect(api.restore).toHaveBeenCalledWith(raw, "reviewed-original"),
      );
      expect(api.download).toHaveBeenCalledTimes(1);
      expect(JSON.stringify(client.getQueryCache().getAll())).not.toContain(
        "中文",
      );
      expect(client.getMutationCache().getAll()).toHaveLength(0);
    },
  );

  it.each(["", undefined])(
    "does not offer plaintext restore without a reviewed snapshot id (%s)",
    async (snapshotId) => {
      settingsApiMock.webdavSyncFetchRemoteInfo.mockResolvedValue({
        deviceName: "Source",
        snapshotId,
        compatible: true,
        artifacts: [],
        version: 2,
      });
      settingsApiMock.webdavSyncDownload.mockRejectedValue(
        "protection.encryption_required",
      );
      renderSection(baseConfig);
      fireEvent.click(
        screen.getByRole("button", { name: "settings.webdavSync.download" }),
      );
      fireEvent.click(
        await screen.findByRole("button", {
          name: "settings.webdavSync.confirmDownload.confirm",
        }),
      );
      await waitFor(() => expect(toastErrorMock).toHaveBeenCalled());
      expect(
        screen.queryByLabelText("syncRestore.password"),
      ).not.toBeInTheDocument();
      expect(settingsApiMock.webdavSyncRestore).not.toHaveBeenCalled();
    },
  );

  it("does not offer source restore for encryption-required upload or an unrelated error containing its code", async () => {
    settingsApiMock.webdavSyncUpload.mockRejectedValue(
      "protection.encryption_required",
    );
    settingsApiMock.webdavSyncDownload.mockRejectedValue(
      "unrelated protection.encryption_required detail",
    );
    renderSection(baseConfig);
    for (const action of ["upload", "download"] as const) {
      fireEvent.click(
        screen.getByRole("button", { name: `settings.webdavSync.${action}` }),
      );
      fireEvent.click(
        await screen.findByRole("button", {
          name: `settings.webdavSync.confirm${action === "upload" ? "Upload" : "Download"}.confirm`,
        }),
      );
      await waitFor(() =>
        expect(
          settingsApiMock[
            action === "upload" ? "webdavSyncUpload" : "webdavSyncDownload"
          ],
        ).toHaveBeenCalled(),
      );
      expect(
        screen.queryByLabelText("syncRestore.password"),
      ).not.toBeInTheDocument();
    }
    expect(settingsApiMock.webdavSyncRestore).not.toHaveBeenCalled();
  });

  it("does not announce restoration or repeat an ambiguous native restore reply", async () => {
    settingsApiMock.webdavSyncDownload.mockRejectedValue(
      "sync.vault_adoption_required",
    );
    settingsApiMock.webdavSyncRestore.mockResolvedValue({ status: "unknown" });
    renderSection(baseConfig);
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.download" }),
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "settings.webdavSync.confirmDownload.confirm",
      }),
    );
    const field = await screen.findByLabelText("syncRestore.password");
    fireEvent.change(field, { target: { value: "synthetic source" } });
    fireEvent.click(
      screen.getByRole("button", { name: "syncRestore.confirm" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "syncRestore.failed",
    );
    expect(toastSuccessMock).not.toHaveBeenCalled();
    expect(settingsApiMock.webdavSyncRestore).toHaveBeenCalledTimes(1);
    expect(field).toHaveValue("");
  });

  it("requires an explicit password restore after foreign vault admission and binds the reviewed snapshot", async () => {
    settingsApiMock.webdavSyncDownload.mockRejectedValue(
      "配置错误: sync.vault_adoption_required",
    );
    settingsApiMock.webdavSyncRestore.mockResolvedValue({ status: "restored" });
    renderSection(baseConfig);
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.download" }),
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "settings.webdavSync.confirmDownload.confirm",
      }),
    );
    expect(
      await screen.findByText("syncRestore.description"),
    ).toBeInTheDocument();
    expect(settingsApiMock.webdavSyncRestore).not.toHaveBeenCalled();
    const confirm = screen.getByRole("button", { name: "syncRestore.confirm" });
    expect(confirm).toBeDisabled();
    fireEvent.change(screen.getByLabelText("syncRestore.password"), {
      target: { value: "source protection password" },
    });
    fireEvent.click(confirm);
    await waitFor(() =>
      expect(settingsApiMock.webdavSyncRestore).toHaveBeenCalledWith(
        "source protection password",
        "snapshot-1",
      ),
    );
    await waitFor(() =>
      expect(
        screen.queryByLabelText("syncRestore.password"),
      ).not.toBeInTheDocument(),
    );
  });

  it("keeps restore deliberate when password verification fails", async () => {
    settingsApiMock.webdavSyncDownload.mockRejectedValue(
      "sync.vault_revision_newer",
    );
    settingsApiMock.webdavSyncRestore.mockRejectedValue(
      "secret.authentication_failed",
    );
    renderSection(baseConfig);
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.download" }),
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "settings.webdavSync.confirmDownload.confirm",
      }),
    );
    const password = await screen.findByLabelText("syncRestore.password");
    fireEvent.change(password, { target: { value: "incorrect password" } });
    fireEvent.click(
      screen.getByRole("button", { name: "syncRestore.confirm" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "syncRestore.failed",
    );
    expect(password).toHaveValue("");
    expect(settingsApiMock.webdavSyncDownload).toHaveBeenCalledTimes(1);
  });

  it("uses the S3 restore action only after deliberate confirmation", async () => {
    settingsApiMock.s3SyncFetchRemoteInfo.mockResolvedValue({
      deviceName: "Source device",
      snapshotId: "s3-snapshot",
      compatible: true,
      createdAt: "2026-02-01T10:00:00Z",
      artifacts: ["db.sql", "skills.zip"],
    });
    settingsApiMock.s3SyncDownload.mockRejectedValue(
      "sync.vault_adoption_required",
    );
    settingsApiMock.s3SyncRestore.mockResolvedValue({ status: "restored" });
    renderSection(undefined, {
      enabled: true,
      bucket: "test-bucket",
      region: "test",
      accessKeyId: "test",
      secretAccessKey: "test",
      status: {},
    });
    fireEvent.click(
      screen.getByRole("button", { name: "settings.s3Sync.download" }),
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "settings.s3Sync.confirmDownload.confirm",
      }),
    );
    const password = await screen.findByLabelText("syncRestore.password");
    expect(settingsApiMock.s3SyncRestore).not.toHaveBeenCalled();
    fireEvent.change(password, {
      target: { value: "cloud protection password" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "syncRestore.confirm" }),
    );
    await waitFor(() =>
      expect(settingsApiMock.s3SyncRestore).toHaveBeenCalledWith(
        "cloud protection password",
        "s3-snapshot",
      ),
    );
    expect(settingsApiMock.webdavSyncRestore).not.toHaveBeenCalled();
  });

  it("shows auto sync error callout when last auto sync failed", async () => {
    renderSection({
      ...baseConfig,
      status: {
        lastError: "network timeout",
        lastErrorSource: "auto",
      },
    });

    expect(
      screen.getByText("settings.webdavSync.autoSyncLastErrorTitle"),
    ).toBeInTheDocument();
    expect(screen.getByText("network timeout")).toBeInTheDocument();
    await act(async () => {});
  });

  it("does not show auto sync error callout for manual sync errors", async () => {
    renderSection({
      ...baseConfig,
      status: {
        lastError: "manual upload failed",
        lastErrorSource: "manual",
      },
    });

    expect(
      screen.queryByText("settings.webdavSync.autoSyncLastErrorTitle"),
    ).not.toBeInTheDocument();
    await act(async () => {});
  });

  it("does not show auto sync error callout when source is missing", async () => {
    renderSection({
      ...baseConfig,
      autoSync: true,
      status: {
        lastError: "legacy error without source",
      },
    });

    expect(
      screen.queryByText("settings.webdavSync.autoSyncLastErrorTitle"),
    ).not.toBeInTheDocument();
    await act(async () => {});
  });

  it("shows validation error when saving without base url", async () => {
    renderSection({ ...baseConfig, baseUrl: "" });

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.save" }),
    );

    expect(toastErrorMock).toHaveBeenCalledWith(
      "settings.webdavSync.missingUrl",
    );
    expect(settingsApiMock.webdavSyncSaveSettings).not.toHaveBeenCalled();
    await act(async () => {});
  });

  it("saves settings and auto tests connection", async () => {
    renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.save" }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavSyncSaveSettings).toHaveBeenCalledTimes(1);
    });
    expect(settingsApiMock.webdavSyncSaveSettings).toHaveBeenCalledWith(
      expect.objectContaining({
        baseUrl: "https://dav.example.com/dav/",
        username: "alice",
        password: "",
        autoSync: false,
      }),
      false,
    );
    await waitFor(() => {
      expect(settingsApiMock.webdavTestConnection).toHaveBeenCalledWith(
        expect.objectContaining({
          baseUrl: "https://dav.example.com/dav/",
        }),
        true,
      );
    });
    expect(toastSuccessMock).toHaveBeenCalledWith(
      "settings.webdavSync.saveAndTestSuccess",
    );
  });

  it("preserves password only for the single post-save refresh", async () => {
    const view = renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.save" }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavSyncSaveSettings).toHaveBeenCalledTimes(1);
    });

    view.rerender(
      <QueryClientProvider client={view.client}>
        <WebdavSyncSection config={{ ...baseConfig, password: "" }} />
      </QueryClientProvider>,
    );

    expect(
      (
        screen.getByPlaceholderText(
          "settings.webdavSync.passwordPlaceholder",
        ) as HTMLInputElement
      ).value,
    ).toBe("secret");

    view.rerender(
      <QueryClientProvider client={view.client}>
        <WebdavSyncSection config={{ ...baseConfig, password: "" }} />
      </QueryClientProvider>,
    );

    expect(
      (
        screen.getByPlaceholderText(
          "settings.webdavSync.passwordPlaceholder",
        ) as HTMLInputElement
      ).value,
    ).toBe("");
  });

  it("does not preserve password after a later external config refresh", async () => {
    const view = renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.save" }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavSyncSaveSettings).toHaveBeenCalledTimes(1);
    });

    view.rerender(
      <QueryClientProvider client={view.client}>
        <WebdavSyncSection config={{ ...baseConfig, password: "" }} />
      </QueryClientProvider>,
    );

    expect(
      (
        screen.getByPlaceholderText(
          "settings.webdavSync.passwordPlaceholder",
        ) as HTMLInputElement
      ).value,
    ).toBe("secret");

    view.rerender(
      <QueryClientProvider client={view.client}>
        <WebdavSyncSection
          config={{ ...baseConfig, username: "bob", password: "" }}
        />
      </QueryClientProvider>,
    );

    expect(
      (
        screen.getByPlaceholderText(
          "settings.webdavSync.passwordPlaceholder",
        ) as HTMLInputElement
      ).value,
    ).toBe("");
  });

  it("does not submit a preserved password again when testing without touching it", async () => {
    const view = renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.save" }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavSyncSaveSettings).toHaveBeenCalledTimes(1);
    });

    view.rerender(
      <QueryClientProvider client={view.client}>
        <WebdavSyncSection config={{ ...baseConfig, password: "" }} />
      </QueryClientProvider>,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.test" }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavTestConnection).toHaveBeenLastCalledWith(
        expect.objectContaining({
          password: "",
        }),
        true,
      );
    });
  });

  it("saves auto sync as true after toggle", async () => {
    renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("switch", { name: "settings.webdavSync.autoSync" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "confirm.autoSync.confirm" }),
    );
    await waitFor(() => {
      expect(
        screen.getByRole("switch", { name: "settings.webdavSync.autoSync" }),
      ).toHaveAttribute("aria-checked", "true");
    });
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.save" }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavSyncSaveSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          autoSync: true,
        }),
        false,
      );
    });
  });

  it("blocks upload when there are unsaved changes", async () => {
    renderSection(baseConfig);

    fireEvent.change(
      screen.getByPlaceholderText("settings.webdavSync.usernamePlaceholder"),
      { target: { value: "bob" } },
    );
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.upload" }),
    );

    expect(toastErrorMock).toHaveBeenCalledWith(
      "settings.webdavSync.unsavedChanges",
    );
    expect(settingsApiMock.webdavSyncFetchRemoteInfo).not.toHaveBeenCalled();
    await act(async () => {});
  });

  it("disables sync buttons until config is saved", async () => {
    renderSection(undefined);

    const uploadButton = screen.getByRole("button", {
      name: "settings.webdavSync.upload",
    });
    const downloadButton = screen.getByRole("button", {
      name: "settings.webdavSync.download",
    });

    expect(uploadButton).toBeDisabled();
    expect(downloadButton).toBeDisabled();
    expect(settingsApiMock.webdavSyncFetchRemoteInfo).not.toHaveBeenCalled();
    await act(async () => {});
  });

  it("fetches remote info and uploads after confirmation", async () => {
    const remoteInfo: RemoteSnapshotInfo = {
      deviceName: "Deferred remote device",
      createdAt: "2026-02-01T10:00:00Z",
      snapshotId: "deferred-snapshot",
      version: 2,
      protocolVersion: 2,
      layout: "current",
      remotePath: "/synthetic/profile/snapshots/deferred-snapshot",
      compatible: true,
      artifacts: ["db.sql", "skills.zip"],
    };
    let resolveRemoteInfo!: (info: RemoteSnapshotInfo) => void;
    const pendingRemoteInfo = new Promise<RemoteSnapshotInfo>((resolve) => {
      resolveRemoteInfo = resolve;
    });
    settingsApiMock.webdavSyncFetchRemoteInfo.mockReturnValueOnce(
      pendingRemoteInfo,
    );
    renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.upload" }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavSyncFetchRemoteInfo).toHaveBeenCalledTimes(
        1,
      );
    });
    expect(
      screen.queryByRole("button", {
        name: "settings.webdavSync.confirmUpload.confirm",
      }),
    ).not.toBeInTheDocument();
    for (const button of screen.getAllByRole("button", {
      name: "settings.webdavSync.fetchingRemote",
    })) {
      expect(button).toBeDisabled();
    }
    expect(settingsApiMock.webdavSyncUpload).not.toHaveBeenCalled();

    // A started request is not a rendered confirmation: wait for the dialog.
    const confirmation = screen.findByRole("button", {
      name: "settings.webdavSync.confirmUpload.confirm",
    });
    await act(async () => {
      resolveRemoteInfo(remoteInfo);
    });
    const confirmButton = await confirmation;
    expect(screen.getByText("Deferred remote device")).toBeInTheDocument();
    expect(settingsApiMock.webdavSyncUpload).not.toHaveBeenCalled();
    fireEvent.click(confirmButton);

    await waitFor(() => {
      expect(settingsApiMock.webdavSyncUpload).toHaveBeenCalledTimes(1);
      expect(toastSuccessMock).toHaveBeenCalledWith(
        "settings.webdavSync.uploadSuccess",
      );
    });
  });

  it("blocks upload confirmation if form changes after dialog opens", async () => {
    renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.upload" }),
    );
    await waitFor(() => {
      expect(settingsApiMock.webdavSyncFetchRemoteInfo).toHaveBeenCalledTimes(
        1,
      );
    });

    const confirmButton = await screen.findByRole("button", {
      name: "settings.webdavSync.confirmUpload.confirm",
    });
    fireEvent.change(screen.getByPlaceholderText("cc-switch-sync"), {
      target: { value: "new-root" },
    });
    fireEvent.click(confirmButton);

    await waitFor(() => {
      expect(toastErrorMock).toHaveBeenCalledWith(
        "settings.webdavSync.unsavedChanges",
      );
    });
    expect(settingsApiMock.webdavSyncUpload).not.toHaveBeenCalled();
  });

  it("fetches remote info and downloads after confirmation", async () => {
    renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.download" }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavSyncFetchRemoteInfo).toHaveBeenCalledTimes(
        1,
      );
    });

    fireEvent.click(
      await screen.findByRole("button", {
        name: "settings.webdavSync.confirmDownload.confirm",
      }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavSyncDownload).toHaveBeenCalledTimes(1);
    });
    expect(toastSuccessMock).toHaveBeenCalledWith(
      "settings.webdavSync.downloadSuccess",
    );
  });

  it("blocks download confirmation if form changes after dialog opens", async () => {
    renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.download" }),
    );
    await waitFor(() => {
      expect(settingsApiMock.webdavSyncFetchRemoteInfo).toHaveBeenCalledTimes(
        1,
      );
    });

    const confirmButton = await screen.findByRole("button", {
      name: "settings.webdavSync.confirmDownload.confirm",
    });
    fireEvent.change(screen.getByPlaceholderText("default"), {
      target: { value: "new-profile" },
    });
    fireEvent.click(confirmButton);

    await waitFor(() => {
      expect(toastErrorMock).toHaveBeenCalledWith(
        "settings.webdavSync.unsavedChanges",
      );
    });
    expect(settingsApiMock.webdavSyncDownload).not.toHaveBeenCalled();
  });

  it("shows info when no remote snapshot is found for download", async () => {
    settingsApiMock.webdavSyncFetchRemoteInfo.mockResolvedValueOnce({
      empty: true,
    });
    renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.download" }),
    );

    await waitFor(() => {
      expect(toastInfoMock).toHaveBeenCalledWith(
        "settings.webdavSync.noRemoteData",
      );
    });
    expect(settingsApiMock.webdavSyncDownload).not.toHaveBeenCalled();
  });

  it("blocks download when remote snapshot is incompatible", async () => {
    settingsApiMock.webdavSyncFetchRemoteInfo.mockResolvedValueOnce({
      deviceName: "Legacy Machine",
      createdAt: "2025-01-01T00:00:00Z",
      snapshotId: "legacy-snapshot",
      version: 1,
      compatible: false,
      artifacts: ["db.sql"],
    });
    renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.download" }),
    );

    await waitFor(() => {
      expect(toastErrorMock).toHaveBeenCalledWith(
        "settings.webdavSync.incompatibleVersion",
      );
    });
    expect(settingsApiMock.webdavSyncDownload).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("button", {
        name: "settings.webdavSync.confirmDownload.confirm",
      }),
    ).not.toBeInTheDocument();
  });

  it("shows error when download fails after confirmation", async () => {
    settingsApiMock.webdavSyncDownload.mockRejectedValueOnce(new Error("boom"));
    renderSection(baseConfig);

    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.download" }),
    );
    await waitFor(() => {
      expect(settingsApiMock.webdavSyncFetchRemoteInfo).toHaveBeenCalledTimes(
        1,
      );
    });

    fireEvent.click(
      screen.getByRole("button", {
        name: "settings.webdavSync.confirmDownload.confirm",
      }),
    );

    await waitFor(() => {
      expect(toastErrorMock).toHaveBeenCalledWith(
        "settings.webdavSync.downloadFailed",
      );
    });
  });
});

describe("independent upgrade sync confirmation", () => {
  const paused = {
    checkpointId: "synthetic-checkpoint",
    revision: "saved-revision",
    paused: true,
  };
  beforeEach(() => {
    vi.mocked(invoke)
      .mockReset()
      .mockImplementation(async (command) => {
        if (command === "get_upgrade_sync_resume_review") return paused;
        if (command === "confirm_upgrade_sync_resume")
          return { ...paused, paused: false };
        throw new Error("unexpected synthetic command");
      });
    for (const method of Object.values(settingsApiMock)) method.mockReset();
    settingsApiMock.webdavSyncSaveSettings.mockResolvedValue({ success: true });
    settingsApiMock.webdavTestConnection.mockResolvedValue({ success: true });
  });
  const confirmations = () =>
    vi
      .mocked(invoke)
      .mock.calls.filter(
        ([command]) => command === "confirm_upgrade_sync_resume",
      );
  async function openReview() {
    fireEvent.click(
      await screen.findByRole("button", {
        name: "startupUpgrade.syncResumeReview",
      }),
    );
    return screen.findByRole("button", {
      name: "startupUpgrade.syncResumeConfirm",
    });
  }
  it.each(["independentConfig", "compatibleDevices"])(
    "requires a fresh saved review and explicit %s choice",
    async (choice) => {
      vi.mocked(invoke)
        .mockResolvedValueOnce(paused)
        .mockResolvedValueOnce({ ...paused, revision: "fresh-saved-revision" });
      renderSection(baseConfig);
      const confirm = await openReview();
      expect(confirm).toBeDisabled();
      expect(
        screen.getByText("startupUpgrade.syncResumeSavedConfig"),
      ).toBeVisible();
      expect(confirmations()).toHaveLength(0);
      fireEvent.click(
        screen.getByRole("button", {
          name: `startupUpgrade.syncResumeChoice.${choice}`,
        }),
      );
      expect(confirmations()).toHaveLength(0);
      fireEvent.click(confirm);
      fireEvent.click(confirm);
      await screen.findByText("startupUpgrade.syncResumeConfirmed");
      expect(confirmations()).toEqual([
        [
          "confirm_upgrade_sync_resume",
          {
            expectedCheckpointId: paused.checkpointId,
            expectedRevision: "fresh-saved-revision",
            choice,
          },
        ],
      ]);
      expect(
        vi
          .mocked(invoke)
          .mock.calls.filter(
            ([command]) => command === "get_upgrade_sync_resume_review",
          ),
      ).toHaveLength(2);
      expect(settingsApiMock.webdavSyncUpload).not.toHaveBeenCalled();
      expect(settingsApiMock.webdavSyncDownload).not.toHaveBeenCalled();
      expect(settingsApiMock.webdavSyncRestore).not.toHaveBeenCalled();
    },
  );
  it.each(["cancel", "edit", "unmount"])(
    "does not confirm after %s",
    async (action) => {
      const view = renderSection(baseConfig);
      const confirm = await openReview();
      fireEvent.click(
        screen.getByRole("button", {
          name: "startupUpgrade.syncResumeChoice.independentConfig",
        }),
      );
      if (action === "cancel")
        fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
      if (action === "edit")
        fireEvent.change(
          screen.getByPlaceholderText(
            "settings.webdavSync.usernamePlaceholder",
          ),
          { target: { value: "new-user" } },
        );
      if (action === "unmount") view.unmount();
      fireEvent.click(confirm);
      expect(confirmations()).toHaveLength(0);
    },
  );
  it("blocks a dirty form and never substitutes saving or connection testing for confirmation", async () => {
    renderSection(baseConfig);
    const reviewButton = await screen.findByRole("button", {
      name: "startupUpgrade.syncResumeReview",
    });
    fireEvent.change(
      screen.getByPlaceholderText("settings.webdavSync.usernamePlaceholder"),
      { target: { value: "new-user" } },
    );
    expect(reviewButton).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.test" }),
    );
    await waitFor(() =>
      expect(settingsApiMock.webdavTestConnection).toHaveBeenCalledTimes(1),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "settings.webdavSync.save" }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.save" }),
    );
    await waitFor(() =>
      expect(settingsApiMock.webdavSyncSaveSettings).toHaveBeenCalledTimes(1),
    );
    expect(confirmations()).toHaveLength(0);
  });
  it.each([null, { ...paused, paused: undefined }])(
    "does not offer confirmation for unavailable or unknown review %j",
    async (next) => {
      renderSection(baseConfig);
      await screen.findByRole("button", {
        name: "startupUpgrade.syncResumeReview",
      });
      vi.mocked(invoke).mockResolvedValueOnce(next);
      fireEvent.click(
        screen.getByRole("button", { name: "startupUpgrade.syncResumeReview" }),
      );
      await screen.findByText("startupUpgrade.syncResumeUnavailable");
      expect(
        screen.queryByRole("button", {
          name: "startupUpgrade.syncResumeConfirm",
        }),
      ).not.toBeInTheDocument();
      expect(confirmations()).toHaveLength(0);
    },
  );
  it("queries a lost confirmation response and never replays the choice", async () => {
    let confirmed = false;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "confirm_upgrade_sync_resume") {
        confirmed = true;
        throw new Error("synthetic lost response");
      }
      if (command === "get_upgrade_sync_resume_review")
        return { ...paused, paused: !confirmed };
      throw new Error("unexpected synthetic command");
    });
    renderSection(baseConfig);
    const confirm = await openReview();
    fireEvent.click(
      screen.getByRole("button", {
        name: "startupUpgrade.syncResumeChoice.compatibleDevices",
      }),
    );
    fireEvent.click(confirm);
    await screen.findByText("startupUpgrade.syncResumeConfirmed");
    expect(confirmations()).toHaveLength(1);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "get_upgrade_sync_resume_review",
        ),
    ).toHaveLength(3);
  });
  it("keeps an unknown confirmation readback unavailable and allows read-only recheck", async () => {
    renderSection(baseConfig);
    const confirm = await openReview();
    fireEvent.click(
      screen.getByRole("button", {
        name: "startupUpgrade.syncResumeChoice.independentConfig",
      }),
    );
    vi.mocked(invoke)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(null);
    fireEvent.click(confirm);
    await screen.findByText("startupUpgrade.syncResumeUnavailable");
    expect(
      screen.queryByText("startupUpgrade.syncResumeConfirmed"),
    ).not.toBeInTheDocument();
    vi.mocked(invoke).mockResolvedValueOnce(paused);
    await openReview();
    expect(confirmations()).toHaveLength(1);
  });

  it("invalidates the confirmation when externally saved settings change", async () => {
    const view = renderSection(baseConfig);
    await openReview();
    fireEvent.click(
      screen.getByRole("button", {
        name: "startupUpgrade.syncResumeChoice.compatibleDevices",
      }),
    );
    view.rerender(
      <QueryClientProvider client={view.client}>
        <WebdavSyncSection
          config={{ ...baseConfig, profile: "another-saved-profile" }}
        />
      </QueryClientProvider>,
    );
    expect(
      screen.queryByRole("button", {
        name: "startupUpgrade.syncResumeConfirm",
      }),
    ).not.toBeInTheDocument();
    expect(confirmations()).toHaveLength(0);
  });

  it("blocks confirmation for unsaved S3 edits", async () => {
    renderSection(undefined, {
      enabled: true,
      bucket: "synthetic-bucket",
      region: "test",
      accessKeyId: "synthetic",
      secretAccessKey: "synthetic",
      status: {},
    });
    await openReview();
    fireEvent.click(
      screen.getByRole("button", {
        name: "startupUpgrade.syncResumeChoice.independentConfig",
      }),
    );
    fireEvent.change(
      screen.getByPlaceholderText("settings.s3Sync.bucketPlaceholder"),
      { target: { value: "changed-bucket" } },
    );
    expect(
      screen.queryByRole("button", {
        name: "startupUpgrade.syncResumeConfirm",
      }),
    ).not.toBeInTheDocument();
    expect(confirmations()).toHaveLength(0);
  });
  it("describes independent sync pause after completion without claiming an active checkpoint", async () => {
    settingsApiMock.webdavSyncFetchRemoteInfo.mockResolvedValue({
      empty: true,
    });
    settingsApiMock.webdavSyncUpload.mockRejectedValue("upgrade.sync_paused");
    toastErrorMock.mockReset();
    renderSection(baseConfig);
    await screen.findByRole("button", {
      name: "startupUpgrade.syncResumeReview",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.upload" }),
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "settings.webdavSync.confirmUpload.confirm",
      }),
    );
    await waitFor(() =>
      expect(toastErrorMock).toHaveBeenCalledWith(
        "startupUpgrade.syncPausedTitle",
        expect.objectContaining({
          description: expect.stringContaining(
            "startupUpgrade.completedSyncPaused",
          ),
        }),
      ),
    );
    expect(toastErrorMock.mock.lastCall?.[1].description).not.toContain(
      "startupUpgrade.syncPausedReason",
    );
    expect(confirmations()).toHaveLength(0);
  });
  it("keeps a read-only recheck after saved settings invalidate the pending initial review", async () => {
    let finish!: (value: typeof paused) => void;
    const pending = new Promise<typeof paused>((resolve) => {
      finish = resolve;
    });
    vi.mocked(invoke).mockReturnValueOnce(pending);
    const view = renderSection(baseConfig);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_upgrade_sync_resume_review"),
    );
    view.rerender(
      <QueryClientProvider client={view.client}>
        <WebdavSyncSection
          config={{ ...baseConfig, profile: "new-saved-profile" }}
        />
      </QueryClientProvider>,
    );
    await act(async () => {
      finish(paused);
      await pending;
    });
    expect(
      screen.queryByText("startupUpgrade.syncResumeConfirmed"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("startupUpgrade.completedSyncPaused"),
    ).not.toBeInTheDocument();
    expect(confirmations()).toHaveLength(0);
    const recheck = await screen.findByRole("button", {
      name: "startupUpgrade.syncResumeReview",
    });
    expect(recheck).toBeEnabled();
    vi.mocked(invoke).mockResolvedValueOnce({
      ...paused,
      revision: "new-saved-revision",
    });
    fireEvent.click(recheck);
    const confirm = await screen.findByRole("button", {
      name: "startupUpgrade.syncResumeConfirm",
    });
    expect(confirm).toBeDisabled();
    expect(confirmations()).toHaveLength(0);
    fireEvent.click(
      screen.getByRole("button", {
        name: "startupUpgrade.syncResumeChoice.compatibleDevices",
      }),
    );
    fireEvent.click(confirm);
    await screen.findByText("startupUpgrade.syncResumeConfirmed");
    expect(confirmations()).toEqual([
      [
        "confirm_upgrade_sync_resume",
        {
          expectedCheckpointId: paused.checkpointId,
          expectedRevision: "new-saved-revision",
          choice: "compatibleDevices",
        },
      ],
    ]);
  });
});
