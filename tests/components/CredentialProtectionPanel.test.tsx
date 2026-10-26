import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { CredentialProtectionStatus } from "@/lib/api/credentialProtection";
import { CredentialProtectionPanel } from "@/components/relay/CredentialProtectionPanel";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const syntheticKey = "  Mixed中文🔑 Key  ";
const initialStatus: CredentialProtectionStatus = {
  mode: "unconfigured",
  status: "needs_configuration",
  shouldPrompt: false,
  canConfigure: true,
  canUnlock: false,
  canSwitchToPlaintext: true,
  canCreateSeparateConfiguration: false,
  reviewToken: "synthetic-review",
  restartRequired: false,
  errorCode: null,
};
let status: CredentialProtectionStatus;
function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const view = render(
    <QueryClientProvider client={client}>
      <CredentialProtectionPanel />
    </QueryClientProvider>,
  );
  return { client, ...view };
}
const chooseEncryption = async () => {
  fireEvent.click(
    await screen.findByRole("button", {
      name: "storageProtection.chooseEncrypted",
    }),
  );
  return screen.getByLabelText("storageProtection.keyLabel");
};
function enterMatchingKey(input: HTMLElement, keyText = syntheticKey) {
  fireEvent.change(input, { target: { value: keyText } });
  fireEvent.change(screen.getByLabelText("storageProtection.confirmKeyLabel"), {
    target: { value: keyText },
  });
}
const writes = () =>
  vi
    .mocked(invoke)
    .mock.calls.filter(
      ([command]) =>
        command !== "get_credential_protection_status" &&
        command !== "get_runtime_availability",
    );
beforeEach(() => {
  status = { ...initialStatus };
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "get_credential_protection_status") return status;
    if (command === "get_runtime_availability")
      return { credentialRuntimeAvailable: true };
    return { ...status, status: "available" };
  });
});
afterEach(() => vi.restoreAllMocks());

describe("user-owned storage protection", () => {
  it("shows two voluntary choices without writing, generating a key or mounting a full-screen gate", async () => {
    mount();
    expect(
      await screen.findByRole("button", {
        name: "storageProtection.chooseEncrypted",
      }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "storageProtection.choosePlaintext" }),
    ).toBeVisible();
    expect(
      screen.queryByLabelText("storageProtection.keyLabel"),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByText(/generate|生成密钥/i)).not.toBeInTheDocument();
    expect(writes()).toEqual([]);
  });

  it("requires plaintext risk acknowledgment and an explicit apply action", async () => {
    mount();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "storageProtection.choosePlaintext",
      }),
    );
    expect(screen.getByText("storageProtection.plaintextRisk")).toBeVisible();
    const apply = screen.getByRole("button", {
      name: "storageProtection.applyPlaintext",
    });
    expect(apply).toBeDisabled();
    expect(writes()).toEqual([]);
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
          reviewToken: "synthetic-review",
        },
      }),
    );
  });

  it.each([
    "中文密钥",
    "🔑🧑‍💻",
    "123",
    "a",
    "A",
    "   ",
    "  key  ",
    "YQ==",
    "e\u0301",
    "é",
    "x".repeat(257),
  ])(
    "accepts the exact original key text without format or strength rules (%j)",
    async (keyText) => {
      mount();
      const input = await chooseEncryption();
      enterMatchingKey(input, keyText);
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
            reviewToken: "synthetic-review",
          },
        }),
      );
      expect(input).toHaveValue("");
      expect(writes()).toHaveLength(1);
    },
  );

  it.each([
    ["key", " key"],
    ["key", "key "],
    ["Key", "key"],
    ["é", "e\u0301"],
    [" ", "  "],
  ])(
    "compares repeated input exactly without trimming, folding case or Unicode normalization (%j, %j)",
    async (keyText, repeated) => {
      mount();
      const input = await chooseEncryption();
      fireEvent.change(input, { target: { value: keyText } });
      fireEvent.change(
        screen.getByLabelText("storageProtection.confirmKeyLabel"),
        { target: { value: repeated } },
      );
      const apply = screen.getByRole("button", {
        name: "storageProtection.applyEncrypted",
      });
      expect(apply).toBeDisabled();
      fireEvent.submit(apply.closest("form")!);
      expect(writes()).toEqual([]);
    },
  );

  it("treats empty fields as unfilled, never as an implicit plaintext skip", async () => {
    mount();
    await chooseEncryption();
    const apply = screen.getByRole("button", {
      name: "storageProtection.applyEncrypted",
    });
    expect(apply).toBeDisabled();
    fireEvent.submit(apply.closest("form")!);
    expect(writes()).toEqual([]);
    expect(
      screen.queryByText("storageProtection.plaintextRisk"),
    ).not.toBeInTheDocument();
  });

  it("submits the user's exact key text, clears the input before completion and never caches the key", async () => {
    const { client } = mount();
    const input = await chooseEncryption();
    expect(input).toHaveClass("[-webkit-text-security:disc]");
    expect(input).toHaveAttribute("autoComplete", "off");
    expect(screen.getByText("storageProtection.keepKeyCopy")).toBeVisible();
    const storage = vi.spyOn(Storage.prototype, "setItem");
    let finish!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    enterMatchingKey(input);
    fireEvent.click(
      screen.getByRole("button", { name: "storageProtection.applyEncrypted" }),
    );
    expect(invoke).toHaveBeenCalledWith("configure_credential_protection", {
      request: {
        mode: "encrypted",
        keyText: syntheticKey,
        preserveExisting: false,
        plaintextAcknowledged: false,
        reviewToken: "synthetic-review",
      },
    });
    expect(input).toHaveValue("");
    expect(client.getMutationCache().getAll()).toEqual([]);
    expect(
      JSON.stringify(
        client
          .getQueryCache()
          .getAll()
          .map((query) => query.state),
      ),
    ).not.toContain(syntheticKey);
    expect(storage).not.toHaveBeenCalled();
    expect(window.location.href).not.toContain(syntheticKey);
    finish({
      ...initialStatus,
      mode: "encrypted",
      status: "available",
      canConfigure: false,
    });
    expect(await screen.findByText("storageProtection.saved")).toBeVisible();
  });

  it("does not leak a backend error containing the key, and clears failed submission input", async () => {
    mount();
    const input = await chooseEncryption();
    const errorLog = vi.spyOn(console, "error");
    vi.mocked(invoke).mockRejectedValueOnce(
      new Error(`invalid secret ${syntheticKey}`),
    );
    enterMatchingKey(input);
    fireEvent.click(
      screen.getByRole("button", { name: "storageProtection.applyEncrypted" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "storageProtection.actionFailed",
    );
    expect(input).toHaveValue("");
    expect(document.body.textContent).not.toContain(syntheticKey);
    expect(errorLog).not.toHaveBeenCalled();
  });

  it("recovers encrypted storage locally with the same key, without offering plaintext downgrade", async () => {
    status = {
      ...initialStatus,
      mode: "encrypted",
      status: "key_required",
      canConfigure: false,
      canSwitchToPlaintext: false,
      canUnlock: true,
    };
    mount();
    const input = await screen.findByLabelText("storageProtection.keyLabel");
    expect(screen.getByText("storageProtection.sameKeyRequired")).toBeVisible();
    expect(
      screen.queryByRole("button", {
        name: "storageProtection.choosePlaintext",
      }),
    ).not.toBeInTheDocument();
    fireEvent.change(input, { target: { value: syntheticKey } });
    fireEvent.click(
      screen.getByRole("button", { name: "storageProtection.unlock" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("unlock_credential_runtime", {
        keyText: syntheticKey,
        reviewToken: "synthetic-review",
      }),
    );
    expect(input).toHaveValue("");
  });

  it("only permits conversion when the backend authorizes it, still requiring plaintext acknowledgment", async () => {
    status = {
      ...initialStatus,
      mode: "encrypted",
      status: "available",
      canConfigure: false,
      canSwitchToPlaintext: true,
    };
    mount();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "storageProtection.choosePlaintext",
      }),
    );
    expect(
      screen.getByText("storageProtection.plaintextConversion"),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "storageProtection.applyPlaintext" }),
    ).toBeDisabled();
  });

  it("requires an explicit separate-configuration choice before offering modes for unreadable originals", async () => {
    status = {
      ...initialStatus,
      mode: "encrypted",
      status: "key_required",
      canConfigure: false,
      canSwitchToPlaintext: false,
      canUnlock: true,
      canCreateSeparateConfiguration: true,
    };
    mount();
    expect(
      screen.queryByRole("button", {
        name: "storageProtection.choosePlaintext",
      }),
    ).not.toBeInTheDocument();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "storageProtection.createSeparate",
      }),
    );
    expect(
      screen.getByText("storageProtection.originalsPreserved"),
    ).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: "storageProtection.choosePlaintext" }),
    );
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "storageProtection.acknowledgePlaintext",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "storageProtection.applyPlaintext" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("configure_credential_protection", {
        request: {
          mode: "plaintext",
          preserveExisting: true,
          plaintextAcknowledged: true,
          reviewToken: "synthetic-review",
        },
      }),
    );
  });

  it("fails closed if status read fails, even if a backend message includes sensitive data", async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error(syntheticKey));
    mount();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "storageProtection.statusFailed",
    );
    expect(
      screen.queryByRole("button", {
        name: "storageProtection.choosePlaintext",
      }),
    ).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain(syntheticKey);
    expect(writes()).toEqual([]);
  });

  it("does not infer configuration permissions when all backend capabilities are false", async () => {
    status = {
      ...initialStatus,
      status: "blocked",
      canConfigure: false,
      canSwitchToPlaintext: false,
      errorCode: "synthetic-private-detail",
    };
    mount();
    expect(
      await screen.findByText("storageProtection.status.blocked"),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", {
        name: "storageProtection.chooseEncrypted",
      }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText("storageProtection.keyLabel"),
    ).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain(status.errorCode);
    expect(writes()).toEqual([]);
  });

  it("does not permit actions with a missing review token even if capabilities claim availability", async () => {
    status = { ...initialStatus, reviewToken: null };
    mount();
    await screen.findByText("storageProtection.status.needs_configuration");
    expect(
      screen.queryByRole("button", {
        name: "storageProtection.choosePlaintext",
      }),
    ).not.toBeInTheDocument();
    expect(writes()).toEqual([]);
  });
});

it.each(["blocked", "key_required"] as const)(
  "does not claim completion for a resolved %s result",
  async (resultStatus) => {
    mount();
    const input = await chooseEncryption();
    vi.mocked(invoke).mockResolvedValueOnce({
      ...initialStatus,
      mode: "encrypted",
      status: resultStatus,
      canConfigure: false,
      errorCode: "synthetic-sensitive-error",
    });
    enterMatchingKey(input);
    fireEvent.click(
      screen.getByRole("button", { name: "storageProtection.applyEncrypted" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "storageProtection.actionFailed",
    );
    expect(
      screen.queryByText("storageProtection.saved"),
    ).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain(
      "synthetic-sensitive-error",
    );
  },
);

it("explains restart-required results without claiming the current runtime is ready", async () => {
  mount();
  const input = await chooseEncryption();
  vi.mocked(invoke).mockResolvedValueOnce({
    ...initialStatus,
    restartRequired: true,
  });
  enterMatchingKey(input);
  fireEvent.click(
    screen.getByRole("button", { name: "storageProtection.applyEncrypted" }),
  );
  expect(
    await screen.findByText("storageProtection.restartRequired"),
  ).toBeVisible();
  expect(screen.queryByText("storageProtection.saved")).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "storageProtection.applyEncrypted" }),
  ).not.toBeInTheDocument();
});

it("clears an unsubmitted key on cancellation before the form is opened again", async () => {
  mount();
  const input = await chooseEncryption();
  enterMatchingKey(input);
  fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
  expect(input).toHaveValue("");
  const next = await chooseEncryption();
  expect(next).toHaveValue("");
  expect(writes()).toEqual([]);
});

it("prevents duplicate submission while an operation is pending", async () => {
  mount();
  const input = await chooseEncryption();
  let finish!: (value: CredentialProtectionStatus) => void;
  vi.mocked(invoke).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  enterMatchingKey(input);
  const apply = screen.getByRole("button", {
    name: "storageProtection.applyEncrypted",
  });
  fireEvent.click(apply);
  fireEvent.click(apply);
  fireEvent.submit(apply.closest("form")!);
  expect(writes()).toHaveLength(1);
  expect(apply).toBeDisabled();
  finish({ ...initialStatus, status: "available" });
  await screen.findByText("storageProtection.saved");
});

it("requires the repeated key to match before configuring encryption and clears both inputs", async () => {
  mount();
  const input = await chooseEncryption();
  const repeated = screen.getByLabelText("storageProtection.confirmKeyLabel");
  fireEvent.change(input, { target: { value: syntheticKey } });
  fireEvent.change(repeated, { target: { value: "12".repeat(32) } });
  const apply = screen.getByRole("button", {
    name: "storageProtection.applyEncrypted",
  });
  expect(apply).toBeDisabled();
  expect(writes()).toEqual([]);
  fireEvent.change(repeated, { target: { value: syntheticKey } });
  fireEvent.click(apply);
  await waitFor(() => expect(writes()).toHaveLength(1));
  expect(input).toHaveValue("");
  expect(repeated).toHaveValue("");
});

it("shows the known key-cache warning without making arbitrary error codes into a cache diagnosis", async () => {
  status = {
    ...initialStatus,
    mode: "encrypted",
    status: "available",
    canConfigure: false,
    errorCode: "protection.key_cache_unavailable",
  };
  mount();
  expect(
    await screen.findByText("storageProtection.keyCacheUnavailable"),
  ).toBeVisible();
});

it("does not treat key-configuration permission as permission to convert encrypted data to plaintext", async () => {
  status = {
    ...initialStatus,
    mode: "encrypted",
    status: "available",
    canConfigure: true,
    canSwitchToPlaintext: false,
  };
  mount();
  await screen.findByRole("button", {
    name: "storageProtection.chooseEncrypted",
  });
  expect(
    screen.queryByRole("button", { name: "storageProtection.choosePlaintext" }),
  ).not.toBeInTheDocument();
});

it("refreshes authoritative status after a rejected operation so a stale review can be retried", async () => {
  mount();
  const input = await chooseEncryption();
  vi.mocked(invoke).mockRejectedValueOnce(new Error("stale review"));
  enterMatchingKey(input);
  fireEvent.click(
    screen.getByRole("button", { name: "storageProtection.applyEncrypted" }),
  );
  await screen.findByRole("alert");
  await waitFor(() =>
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "get_credential_protection_status",
        ),
    ).toHaveLength(2),
  );
});

it.each(["🔓中文", "1", " "])(
  "recovers with original key text unchanged (%j)",
  async (keyText) => {
    status = {
      ...initialStatus,
      mode: "encrypted",
      status: "key_required",
      canConfigure: false,
      canUnlock: true,
      canSwitchToPlaintext: false,
    };
    mount();
    const input = await screen.findByLabelText("storageProtection.keyLabel");
    fireEvent.change(input, { target: { value: keyText } });
    const unlock = screen.getByRole("button", {
      name: "storageProtection.unlock",
    });
    expect(unlock).toBeEnabled();
    fireEvent.click(unlock);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("unlock_credential_runtime", {
        keyText,
        reviewToken: "synthetic-review",
      }),
    );
    expect(input).toHaveValue("");
  },
);

it("does not consider pasted CRLF and LF confirmations equal, and submits matching CRLF unchanged", async () => {
  mount();
  const input = await chooseEncryption();
  const repeated = screen.getByLabelText("storageProtection.confirmKeyLabel");
  const paste = (field: HTMLElement, text: string) =>
    fireEvent.paste(field, { clipboardData: { getData: () => text } });
  paste(input, "\r\n原文🔑\r\n");
  paste(repeated, "\n原文🔑\n");
  const apply = screen.getByRole("button", {
    name: "storageProtection.applyEncrypted",
  });
  expect(apply).toBeDisabled();
  expect(writes()).toEqual([]);
  (repeated as HTMLTextAreaElement).select();
  paste(repeated, "\r\n原文🔑\r\n");
  expect(apply).toBeEnabled();
  fireEvent.click(apply);
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("configure_credential_protection", {
      request: {
        mode: "encrypted",
        keyText: "\r\n原文🔑\r\n",
        preserveExisting: false,
        plaintextAcknowledged: false,
        reviewToken: "synthetic-review",
      },
    }),
  );
  expect(input).toHaveValue("");
  expect(repeated).toHaveValue("");
});

it("recovers by pasting the exact original multiline key without a silent downgrade", async () => {
  status = {
    ...initialStatus,
    mode: "encrypted",
    status: "key_required",
    canConfigure: false,
    canUnlock: true,
    canSwitchToPlaintext: false,
  };
  mount();
  const input = await screen.findByLabelText("storageProtection.keyLabel");
  const keyText = "\r\n old\rkey\n ";
  fireEvent.paste(input, { clipboardData: { getData: () => keyText } });
  fireEvent.click(
    screen.getByRole("button", { name: "storageProtection.unlock" }),
  );
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("unlock_credential_runtime", {
      keyText,
      reviewToken: "synthetic-review",
    }),
  );
  expect(writes()).toHaveLength(1);
  expect(input).toHaveValue("");
});
