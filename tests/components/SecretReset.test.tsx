import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SecretReset } from "@/components/SecretReset";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
describe("SecretReset", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());
  it("requires a fresh preview and explicit acknowledgement before resetting", async () => {
    const onReset = vi.fn();
    vi.mocked(invoke)
      .mockResolvedValueOnce({
        fingerprint: "reviewed-state",
        protectedValues: 3,
      })
      .mockResolvedValueOnce("/recovery/fixture.lpbackup");
    render(<SecretReset onReset={onReset} onNeedsRestart={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "secrets.resetTitle" }));
    await screen.findByText("secrets.resetImpact");
    const submit = screen.getByRole("button", { name: "secrets.resetAction" });
    fireEvent.change(screen.getByLabelText("secrets.newPassword"), {
      target: { value: "new protection password" },
    });
    expect(submit).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(submit);
    await screen.findByText("/recovery/fixture.lpbackup");
    expect(invoke).toHaveBeenLastCalledWith("reset_secret_vault", {
      fingerprint: "reviewed-state",
      password: "new protection password",
    });
    expect(onReset).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "secrets.enterApp" }));
    expect(onReset).toHaveBeenCalledOnce();
  });
  it("keeps reset unavailable when the preview cannot be read", async () => {
    vi.mocked(invoke).mockRejectedValueOnce("secret.storage_unavailable");
    render(<SecretReset onReset={vi.fn()} onNeedsRestart={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "secrets.resetTitle" }));
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", { name: "secrets.resetAction" }),
    ).toBeDisabled();
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
  });
});
