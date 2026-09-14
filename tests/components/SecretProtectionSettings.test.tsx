import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SecretProtectionSettings } from "@/components/settings/SecretProtectionSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

function mount() {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({
          defaultOptions: {
            queries: { retry: false },
            mutations: { retry: false },
          },
        })
      }
    >
      <SecretProtectionSettings />
    </QueryClientProvider>,
  );
}

describe("SecretProtectionSettings", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_secret_protection")
        return { automaticUnlock: true, passwordConfigured: true };
      return undefined;
    });
  });

  it("changes the password and clears it only after a successful save", async () => {
    mount();
    const input = await screen.findByLabelText("secrets.newPassword");
    fireEvent.change(input, {
      target: { value: "replacement protection password" },
    });
    fireEvent.click(
      screen.getByRole("switch", { name: "secrets.automaticUnlock" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_secret_password", {
        password: "replacement protection password",
        automaticUnlock: false,
      }),
    );
    await waitFor(() => expect(input).toHaveValue(""));
  });

  it("rotates only when explicitly selected and preserves retry input on failure", async () => {
    mount();
    const input = await screen.findByLabelText("secrets.newPassword");
    fireEvent.change(input, { target: { value: "new rotation password" } });
    fireEvent.click(screen.getByRole("switch", { name: "secrets.rotateKey" }));
    expect(screen.getByText("secrets.rotateNotice")).toBeVisible();
    vi.mocked(invoke).mockRejectedValueOnce("secret.operation_failed");
    fireEvent.click(screen.getByRole("button", { name: "secrets.rotateKey" }));
    await screen.findByRole("alert");
    expect(invoke).toHaveBeenLastCalledWith("rotate_secret_key", {
      password: "new rotation password",
      automaticUnlock: true,
    });
    expect(input).toHaveValue("new rotation password");
  });
});
