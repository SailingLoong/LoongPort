import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SecretUnlock } from "@/components/SecretUnlock";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

describe("SecretUnlock", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it("keeps the recovery view on failure and enters the app only after success", async () => {
    const onUnlocked = vi.fn();
    vi.mocked(invoke).mockRejectedValueOnce("secret.password_rejected");
    render(<SecretUnlock onUnlocked={onUnlocked} />);
    fireEvent.change(screen.getByLabelText("secrets.password"), {
      target: { value: "test protection password" },
    });
    fireEvent.click(screen.getByRole("button", { name: "secrets.unlock" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "secrets.passwordRejected",
    );
    expect(onUnlocked).not.toHaveBeenCalled();

    vi.mocked(invoke).mockResolvedValueOnce(undefined);
    fireEvent.click(screen.getByRole("button", { name: "secrets.unlock" }));
    await waitFor(() => expect(onUnlocked).toHaveBeenCalledOnce());
    expect(invoke).toHaveBeenLastCalledWith("unlock_secret_vault", {
      password: "test protection password",
    });
    expect(screen.getByLabelText("secrets.password")).toHaveValue("");
  });

  it("retries the system store without submitting the password field", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined);
    const onUnlocked = vi.fn();
    render(<SecretUnlock onUnlocked={onUnlocked} />);
    fireEvent.click(
      screen.getByRole("button", { name: "secrets.retrySystemUnlock" }),
    );
    await waitFor(() => expect(onUnlocked).toHaveBeenCalledOnce());
    expect(invoke).toHaveBeenCalledWith("unlock_secret_vault", {
      password: null,
    });
  });
});
