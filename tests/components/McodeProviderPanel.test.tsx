import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { McodeProviderPanel } from "@/components/providers/McodeProviderPanel";
const api = vi.hoisted(() => ({ getBackup: vi.fn(), restoreBackup: vi.fn() }));
vi.mock("@/lib/api/mcode", () => ({ mcodeApi: api }));
function show() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <McodeProviderPanel>
        {(busy) => <button disabled={busy}>Existing provider list</button>}
      </McodeProviderPanel>
    </QueryClientProvider>,
  );
  return client;
}
beforeEach(() => {
  api.getBackup
    .mockReset()
    .mockResolvedValue({ id: "backup-1", revision: "revision-1" });
  api.restoreBackup.mockReset().mockResolvedValue(true);
});
describe("MiniMax Code safe restore", () => {
  it("reads backup status without restoring and requires explicit confirmation", async () => {
    const client = show();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    fireEvent.click(
      await screen.findByRole("button", { name: "mcode.restore" }),
    );
    expect(api.restoreBackup).not.toHaveBeenCalled();
    expect(screen.getByText("mcode.restoreConfirm")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
    await waitFor(() =>
      expect(api.restoreBackup).toHaveBeenCalledWith({
        id: "backup-1",
        revision: "revision-1",
      }),
    );
    await waitFor(() =>
      expect(invalidate).toHaveBeenCalledWith({
        queryKey: ["providers", "mcode"],
      }),
    );
  });
  it("does not offer restoration when no backup exists", async () => {
    api.getBackup.mockResolvedValue(null);
    show();
    await waitFor(() => expect(api.getBackup).toHaveBeenCalledTimes(1));
    expect(
      screen.queryByRole("button", { name: "mcode.restore" }),
    ).not.toBeInTheDocument();
  });
  it("keeps the confirmation open on conflict and never forces a restore", async () => {
    api.restoreBackup.mockRejectedValue(new Error("sourceChanged"));
    show();
    fireEvent.click(
      await screen.findByRole("button", { name: "mcode.restore" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.confirm" }));
    await waitFor(() =>
      expect(screen.getByRole("dialog")).toHaveTextContent("sourceChanged"),
    );
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "common.confirm" }),
    ).toBeDisabled();
    expect(api.restoreBackup).toHaveBeenCalledTimes(1);
  });
});
