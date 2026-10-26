import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { CredentialProtectionEntry } from "@/components/relay/CredentialProtectionEntry";
import type { CredentialProtectionStatus } from "@/lib/api/credentialProtection";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
let status: CredentialProtectionStatus;
const key = "  🔑原文 Key  ";
beforeEach(() => {
  status = {
    mode: "unconfigured",
    status: "needs_configuration",
    shouldPrompt: true,
    canConfigure: true,
    canUnlock: false,
    canSwitchToPlaintext: true,
    canCreateSeparateConfiguration: false,
    reviewToken: "prompt-review",
    restartRequired: false,
    errorCode: null,
  };
  vi.mocked(invoke)
    .mockReset()
    .mockImplementation(async (command) => {
      if (command === "get_startup_recovery")
        throw "secret.no_pending_operation";
      if (command === "get_credential_protection_status") return status;
      return {
        ...status,
        mode: "plaintext",
        status: "available",
        shouldPrompt: false,
      };
    });
});
function mount(showInline = false) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <CredentialProtectionEntry showInline={showInline} />
    </QueryClientProvider>,
  );
  return client;
}
it("opens the requested setup dialog with key and repeated-key fields, not a full-screen gate", async () => {
  mount();
  const dialog = await screen.findByRole("dialog");
  expect(dialog).toHaveTextContent("storageProtection.promptTitle");
  expect(screen.getByLabelText("storageProtection.keyLabel")).toHaveClass(
    "[-webkit-text-security:disc]",
  );
  expect(
    screen.getByLabelText("storageProtection.confirmKeyLabel"),
  ).toHaveClass("[-webkit-text-security:disc]");
  expect(screen.getByText("storageProtection.localOnlyNotice")).toBeVisible();
  expect(
    vi
      .mocked(invoke)
      .mock.calls.every(([command]) =>
        ["get_credential_protection_status", "get_startup_recovery"].includes(
          command,
        ),
      ),
  ).toBe(true);
});
it("closing the dialog never silently chooses plaintext or saves a key", async () => {
  mount(true);
  await screen.findByRole("dialog");
  const first = screen.getByLabelText("storageProtection.keyLabel");
  fireEvent.change(first, { target: { value: key } });
  fireEvent.change(screen.getByLabelText("storageProtection.confirmKeyLabel"), {
    target: { value: key },
  });
  fireEvent.click(screen.getByRole("button", { name: "common.close" }));
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  expect(first).toHaveValue("");
  expect(
    vi
      .mocked(invoke)
      .mock.calls.every(([command]) =>
        ["get_credential_protection_status", "get_startup_recovery"].includes(
          command,
        ),
      ),
  ).toBe(true);
  expect(
    screen.getByRole("button", { name: "storageProtection.chooseEncrypted" }),
  ).toBeVisible();
});
it("skip means an explicit acknowledged plaintext choice and completed owner state closes the prompt", async () => {
  mount();
  fireEvent.click(
    await screen.findByRole("button", {
      name: "storageProtection.skipPlaintext",
    }),
  );
  expect(screen.getByText("storageProtection.plaintextRisk")).toBeVisible();
  const apply = screen.getByRole("button", {
    name: "storageProtection.applyPlaintext",
  });
  expect(apply).toBeDisabled();
  fireEvent.click(
    screen.getByRole("checkbox", {
      name: "storageProtection.acknowledgePlaintext",
    }),
  );
  fireEvent.click(apply);
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("configure_credential_protection", {
      request: {
        mode: "plaintext",
        preserveExisting: false,
        plaintextAcknowledged: true,
        reviewToken: "prompt-review",
      },
    }),
  );
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
});
it("does not prompt again after the persistent backend choice is complete", async () => {
  status = {
    ...status,
    shouldPrompt: false,
    mode: "plaintext",
    status: "available",
  };
  const client = mount();
  await waitFor(() =>
    expect(client.getQueryState(["credentialProtection"])?.status).toBe(
      "success",
    ),
  );
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(document.body).not.toHaveTextContent("storageProtection.promptTitle");
});
it("never lets an unreadable encrypted upgrade skip into plaintext", async () => {
  status = {
    ...status,
    mode: "encrypted",
    status: "key_required",
    canConfigure: false,
    canSwitchToPlaintext: false,
    canUnlock: true,
  };
  mount();
  await screen.findByRole("dialog");
  expect(
    screen.queryByRole("button", { name: "storageProtection.skipPlaintext" }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByLabelText("storageProtection.confirmKeyLabel"),
  ).not.toBeInTheDocument();
  expect(screen.getByText("storageProtection.sameKeyRequired")).toBeVisible();
});

it("accepts literal custom text in the setup dialog without a format gate or strength notice", async () => {
  mount();
  await screen.findByRole("dialog");
  vi.mocked(invoke).mockResolvedValueOnce({
    ...status,
    mode: "encrypted",
    status: "available",
    shouldPrompt: false,
  });
  const keyText = "  1中文🔑  ";
  fireEvent.change(screen.getByLabelText("storageProtection.keyLabel"), {
    target: { value: keyText },
  });
  fireEvent.change(screen.getByLabelText("storageProtection.confirmKeyLabel"), {
    target: { value: keyText },
  });
  const apply = screen.getByRole("button", {
    name: "storageProtection.applyEncrypted",
  });
  expect(apply).toBeEnabled();
  expect(
    screen.queryByText(
      /strength|complexity|weak|too short|强度|复杂度|过短|简单.*风险/i,
    ),
  ).not.toBeInTheDocument();
  fireEvent.click(apply);
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("configure_credential_protection", {
      request: {
        mode: "encrypted",
        keyText,
        preserveExisting: false,
        plaintextAcknowledged: false,
        reviewToken: "prompt-review",
      },
    }),
  );
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
});
