import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LegacySyncCleanup } from "@/components/settings/LegacySyncCleanup";
const mocks = vi.hoisted(() => ({
  webdavPreview: vi.fn(),
  webdavCleanup: vi.fn(),
  s3Preview: vi.fn(),
  s3Cleanup: vi.fn(),
}));
vi.mock("@/lib/api", () => ({
  settingsApi: {
    webdavSyncLegacyCleanupPreview: mocks.webdavPreview,
    webdavSyncCleanupLegacy: mocks.webdavCleanup,
    s3SyncLegacyCleanupPreview: mocks.s3Preview,
    s3SyncCleanupLegacy: mocks.s3Cleanup,
  },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() },
}));
beforeEach(() => {
  for (const mock of Object.values(mocks)) mock.mockReset();
});
describe("Legacy snapshot cleanup", () => {
  it.each(["webdav", "s3"] as const)(
    "requires a separate explicit confirmation for %s",
    async (transport) => {
      const preview =
        transport === "webdav" ? mocks.webdavPreview : mocks.s3Preview;
      const cleanup =
        transport === "webdav" ? mocks.webdavCleanup : mocks.s3Cleanup;
      preview.mockResolvedValue({
        paths: ["known/legacy/db.sql"],
        canClean: true,
        receipt: "reviewed-receipt",
        blockedReason: null,
      });
      cleanup.mockResolvedValue({ deletedObjects: 1 });
      render(<LegacySyncCleanup transport={transport} />);
      fireEvent.click(
        screen.getByRole("button", { name: "legacySyncCleanup.check" }),
      );
      await waitFor(() => expect(preview).toHaveBeenCalledTimes(1));
      expect(cleanup).not.toHaveBeenCalled();
      fireEvent.click(
        await screen.findByRole("button", {
          name: "legacySyncCleanup.confirm",
        }),
      );
      await waitFor(() =>
        expect(cleanup).toHaveBeenCalledWith("reviewed-receipt"),
      );
    },
  );
  it("disables deletion when the encrypted backup is not recoverable", async () => {
    mocks.webdavPreview.mockResolvedValue({
      paths: ["known/legacy/db.sql"],
      canClean: false,
      receipt: null,
      blockedReason: "sync.cleanup_encrypted_backup_required",
    });
    render(<LegacySyncCleanup transport="webdav" />);
    fireEvent.click(
      screen.getByRole("button", { name: "legacySyncCleanup.check" }),
    );
    expect(
      await screen.findByRole("button", { name: "legacySyncCleanup.confirm" }),
    ).toBeDisabled();
    expect(mocks.webdavCleanup).not.toHaveBeenCalled();
  });
});
