import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackupListSection } from "@/components/settings/BackupListSection";

const api = vi.hoisted(() => ({
  listDbBackups: vi.fn(),
  restoreDbBackup: vi.fn(),
  createDbBackup: vi.fn(),
  renameDbBackup: vi.fn(),
  deleteDbBackup: vi.fn(),
}));
const toast = vi.hoisted(() => ({
  success: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
}));
vi.mock("@/lib/api", () => ({ backupsApi: api }));
vi.mock("sonner", () => ({ toast }));
let client: QueryClient;
beforeEach(() => {
  vi.resetAllMocks();
  api.listDbBackups.mockResolvedValue([
    {
      filename: "source.db",
      sizeBytes: 128,
      createdAt: "2026-10-25T00:00:00Z",
    },
  ]);
  client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: 1, retryDelay: 0 },
    },
  });
});
afterEach(async () => {
  await client.cancelQueries();
  client.clear();
});
async function confirmation() {
  render(
    <QueryClientProvider client={client}>
      <BackupListSection onSettingsChange={() => {}} />
    </QueryClientProvider>,
  );
  fireEvent.click(await screen.findByRole("button", { name: "Restore" }));
  return within(screen.getByRole("dialog")).getByRole("button", {
    name: "Restore",
  });
}
describe("backup restore committed-result consumer", () => {
  it("shows partial success with the unchanged safety ID and refreshes queries without restoring twice", async () => {
    api.restoreDbBackup.mockResolvedValue({
      success: true,
      backupId: "safety-123",
      warning:
        "Post-operation synchronization failed: Some live configuration or runtime state could not be synchronized. See local logs.",
    });
    const invalidate = vi.spyOn(client, "invalidateQueries");
    fireEvent.click(await confirmation());
    await waitFor(() => expect(toast.warning).toHaveBeenCalled());
    expect(toast.warning.mock.calls[0][0]).toBe(
      "Database restored; some synchronization is incomplete.",
    );
    expect(JSON.stringify(toast.warning.mock.calls)).toContain("safety-123");
    expect(JSON.stringify(toast.warning.mock.calls)).toContain(
      "Some live configuration or runtime state could not be synchronized.",
    );
    expect(toast.success).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
    expect(invalidate).toHaveBeenCalledTimes(1);
    expect(api.listDbBackups.mock.calls.length).toBeGreaterThan(1);
    expect(api.restoreDbBackup).toHaveBeenCalledExactlyOnceWith("source.db");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("keeps ordinary success and the safety ID", async () => {
    api.restoreDbBackup.mockResolvedValue({
      success: true,
      backupId: "safety-ok",
    });
    fireEvent.click(await confirmation());
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    expect(JSON.stringify(toast.success.mock.calls)).toContain("safety-ok");
    expect(JSON.stringify(toast.success.mock.calls)).not.toContain(
      "[object Object]",
    );
    expect(toast.warning).not.toHaveBeenCalled();
    expect(api.restoreDbBackup).toHaveBeenCalledTimes(1);
  });
  it("does not retry an execution rejection even when mutation defaults enable retries", async () => {
    api.restoreDbBackup.mockRejectedValue(
      new Error("restore outcome unresolved"),
    );
    fireEvent.click(await confirmation());
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("restore outcome unresolved"),
    );
    expect(api.restoreDbBackup).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
    expect(toast.warning).not.toHaveBeenCalled();
  });
  it("ignores same-tick repeated confirmation while the original restore is pending", async () => {
    let finish!: (value: unknown) => void;
    api.restoreDbBackup.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const button = await confirmation();
    act(() => {
      button.click();
      button.click();
    });
    await waitFor(() => expect(api.restoreDbBackup).toHaveBeenCalledTimes(1));
    expect(toast.success).not.toHaveBeenCalled();
    await act(async () => {
      finish({ success: true, backupId: "one-safety" });
    });
    await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(1));
    expect(api.restoreDbBackup).toHaveBeenCalledTimes(1);
  });
});
