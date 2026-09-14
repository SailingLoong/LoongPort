import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SecretRestore } from "@/components/SecretRestore";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
describe("SecretRestore", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());
  it("restores a reviewed snapshot without loading ordinary app settings", async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({
        snapshotId: "reviewed-id",
        deviceName: "Other device",
        createdAt: "fixture",
      })
      .mockResolvedValueOnce(undefined);
    const onRestored = vi.fn();
    render(<SecretRestore onRestored={onRestored} onNeedsRestart={vi.fn()} />);
    fireEvent.click(
      screen.getByRole("button", { name: "secretRestore.title" }),
    );
    fireEvent.change(screen.getByLabelText("settings.webdavSync.baseUrl"), {
      target: { value: "https://sync.example.invalid" },
    });
    fireEvent.change(screen.getByLabelText("settings.webdavSync.password"), {
      target: { value: "connection password" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "secretRestore.inspect" }),
    );
    fireEvent.change(await screen.findByLabelText("syncRestore.password"), {
      target: { value: "remote protection password" },
    });
    expect(
      screen.getByRole("button", { name: "syncRestore.confirm" }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("checkbox", { name: "secretRestore.confirmation" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "syncRestore.confirm" }),
    );
    await waitFor(() => expect(onRestored).toHaveBeenCalledOnce());
    expect(invoke).toHaveBeenLastCalledWith("restore_startup_vault", {
      source: {
        transport: "webdav",
        settings: {
          baseUrl: "https://sync.example.invalid",
          password: "connection password",
        },
      },
      password: "remote protection password",
      expectedSnapshotId: "reviewed-id",
      automaticUnlock: false,
    });
    expect(invoke).toHaveBeenCalledTimes(2);
  });
  it("invalidates the reviewed snapshot when connection settings change", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      snapshotId: "reviewed-id",
      deviceName: "Other device",
      createdAt: "fixture",
    });
    render(<SecretRestore onRestored={vi.fn()} onNeedsRestart={vi.fn()} />);
    fireEvent.click(
      screen.getByRole("button", { name: "secretRestore.title" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "secretRestore.inspect" }),
    );
    await screen.findByLabelText("syncRestore.password");
    fireEvent.change(screen.getByLabelText("settings.webdavSync.baseUrl"), {
      target: { value: "https://different.example.invalid" },
    });
    expect(
      screen.queryByRole("button", { name: "syncRestore.confirm" }),
    ).not.toBeInTheDocument();
  });
});
