import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { CredentialProtectionEntry } from "@/components/relay/CredentialProtectionEntry";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const pending = {
  token: "synthetic-original-operation-token",
  status: "pending",
  canRecover: true,
  restartRequired: false,
};
const blockedProtection = {
  mode: "encrypted",
  status: "blocked",
  shouldPrompt: false,
  canConfigure: false,
  canUnlock: false,
  canSwitchToPlaintext: false,
  canCreateSeparateConfiguration: false,
  reviewToken: null,
  restartRequired: false,
  errorCode: null,
};
beforeEach(() => {
  vi.mocked(invoke)
    .mockReset()
    .mockImplementation(async (command) => {
      if (command === "get_startup_recovery") return pending;
      if (command === "get_credential_protection_status")
        return blockedProtection;
      throw new Error("unexpected test IPC");
    });
});
function mount(showInline = true) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const view = render(
    <QueryClientProvider client={client}>
      <nav aria-label="shell">
        <button>Settings</button>
      </nav>
      <CredentialProtectionEntry showInline={showInline} />
    </QueryClientProvider>,
  );
  return { client, ...view };
}
it("exposes the existing pending-operation recovery from the mounted entry without replacing shell navigation", async () => {
  mount();
  expect(
    await screen.findByRole("heading", { name: "startupRecovery.title" }),
  ).toBeVisible();
  expect(screen.getByRole("button", { name: "Settings" })).toBeEnabled();
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "startupRecovery.resume" }),
  ).toBeDisabled();
  expect(invoke).toHaveBeenCalledWith("get_startup_recovery");
  expect(document.querySelector(".min-h-screen")).toBeNull();
});
it("does not probe recovery or disturb normal choices outside the unavailable-runtime inline entry", async () => {
  const { client } = mount(false);
  await waitFor(() =>
    expect(client.getQueryState(["credentialProtection"])?.status).toBe(
      "success",
    ),
  );
  expect(invoke).not.toHaveBeenCalledWith("get_startup_recovery");
  expect(screen.queryByText("startupRecovery.title")).not.toBeInTheDocument();
});
it("treats the exact native no-pending result as absence, without inventing a recovery or changing mode", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "get_startup_recovery") throw "secret.no_pending_operation";
    if (command === "get_credential_protection_status")
      return blockedProtection;
    throw new Error("unexpected test IPC");
  });
  mount();
  expect(await screen.findByText("storageProtection.title")).toBeVisible();
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("get_startup_recovery"),
  );
  expect(screen.queryByText("startupRecovery.title")).not.toBeInTheDocument();
  expect(
    vi
      .mocked(invoke)
      .mock.calls.every(([command]) => command.startsWith("get_")),
  ).toBe(true);
});
it.each([
  { ...pending, canRecover: false },
  { ...pending, canRecover: "true" },
  { ...pending, token: "" },
  {
    ...pending,
    status: "verification_required",
    canRecover: false,
    restartRequired: true,
  },
  { ...pending, status: "completed", canRecover: false, restartRequired: true },
  { ...pending, status: "future_status" },
  { ...pending, status: ["pending"] },
])(
  "never enables pending recovery from unavailable or malformed authority (%j)",
  async (view) => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "get_startup_recovery" ? view : blockedProtection,
    );
    const { client } = mount();
    await waitFor(() => expect(client.isFetching()).toBe(0));
    expect(
      screen.queryByLabelText("startupRecovery.sourceKey"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "startupRecovery.resume" }),
    ).not.toBeInTheDocument();
  },
);
it("does not cache or echo a native status failure and offers only a fresh inspection", async () => {
  const secret = "synthetic-sensitive-error-text";
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "get_startup_recovery")
      throw { code: "unknown", message: secret };
    return blockedProtection;
  });
  const { client } = mount();
  expect(await screen.findByText("startupRecovery.statusFailed")).toBeVisible();
  expect(document.body).not.toHaveTextContent(secret);
  expect(
    JSON.stringify(
      client
        .getQueryCache()
        .getAll()
        .map((q) => q.state),
    ),
  ).not.toContain(secret);
  expect(
    screen.getByRole("button", { name: "startupRecovery.recheck" }),
  ).toBeEnabled();
  expect(
    screen.queryByText("storageProtection.chooseEncrypted"),
  ).not.toBeInTheDocument();
});
const completed = {
  ...pending,
  status: "completed",
  canRecover: false,
  restartRequired: true,
};
function pasteKey(input: HTMLElement, text: string) {
  fireEvent.paste(input, {
    clipboardData: {
      getData: (format: string) => (format === "text/plain" ? text : ""),
    },
  });
}
const recoveryWrites = () =>
  vi
    .mocked(invoke)
    .mock.calls.filter(([command]) => command === "recover_startup_operation");
it("submits the exact source text and original token once, clears immediately, and keeps secrets out of query/storage", async () => {
  let finish!: (value: unknown) => void;
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "get_startup_recovery") return pending;
    if (command === "recover_startup_operation")
      return new Promise((resolve) => {
        finish = resolve;
      });
    return blockedProtection;
  });
  const { client } = mount();
  const input = await screen.findByLabelText("startupRecovery.sourceKey");
  const secret = " \r\n🔑中文e\u0301 MiXeD \r ";
  pasteKey(input, secret);
  const submit = screen.getByRole("button", { name: "startupRecovery.resume" });
  fireEvent.click(submit);
  fireEvent.click(submit);
  expect(recoveryWrites()).toEqual([
    ["recover_startup_operation", { token: pending.token, password: secret }],
  ]);
  expect(input).toHaveValue("");
  expect(
    JSON.stringify(
      client
        .getQueryCache()
        .getAll()
        .map((q) => q.state),
    ),
  ).not.toContain(secret);
  expect(client.getMutationCache().getAll()).toHaveLength(0);
  expect(JSON.stringify(localStorage)).not.toContain(secret);
  expect(JSON.stringify(sessionStorage)).not.toContain(secret);
  finish(completed);
  expect(await screen.findByText("startupRecovery.completed")).toBeVisible();
  expect(screen.getByRole("button", { name: "secrets.restart" })).toBeEnabled();
  expect(screen.queryByText("storageProtection.saved")).not.toBeInTheDocument();
  expect(client.getQueryData(["runtimeAvailability"])).toBeUndefined();
});
it.each([" ", "1", "🔑", "e\u0301", "é"])(
  "accepts a nonempty original source input without strength rules (%j)",
  async (text) => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "recover_startup_operation" ? completed : pending,
    );
    mount();
    pasteKey(await screen.findByLabelText("startupRecovery.sourceKey"), text);
    fireEvent.click(
      screen.getByRole("button", { name: "startupRecovery.resume" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("recover_startup_operation", {
        token: pending.token,
        password: text,
      }),
    );
    expect(document.body).not.toHaveTextContent(
      /strength|复杂度|过短|简单.*风险/,
    );
  },
);
it("never interprets an empty input as plaintext, OS-key fallback, or permission to recover", async () => {
  mount();
  await screen.findByLabelText("startupRecovery.sourceKey");
  fireEvent.click(
    screen.getByRole("button", { name: "startupRecovery.resume" }),
  );
  expect(recoveryWrites()).toHaveLength(0);
});
it("refreshes authority after a wrong source key without echoing or caching native error details", async () => {
  const secret = "synthetic-rejected-source-key";
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "recover_startup_operation")
      throw {
        code: "secret.password_rejected",
        message: secret,
        restartRequired: false,
      };
    return pending;
  });
  const { client } = mount();
  const input = await screen.findByLabelText("startupRecovery.sourceKey");
  pasteKey(input, secret);
  fireEvent.click(
    screen.getByRole("button", { name: "startupRecovery.resume" }),
  );
  expect(await screen.findByText("startupRecovery.keyRejected")).toBeVisible();
  await waitFor(() =>
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(([c]) => c === "get_startup_recovery"),
    ).toHaveLength(2),
  );
  expect(input).toHaveValue("");
  expect(document.body).not.toHaveTextContent(secret);
  expect(
    JSON.stringify(
      client
        .getQueryCache()
        .getAll()
        .map((q) => q.state),
    ),
  ).not.toContain(secret);
  expect(recoveryWrites()).toHaveLength(1);
});
it("discards an expired token and never replays the original input after a refreshed token", async () => {
  let refreshed = false;
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "get_startup_recovery")
      return {
        ...pending,
        token: refreshed ? "fresh-operation-token" : pending.token,
      };
    if (command === "recover_startup_operation") {
      refreshed = true;
      throw { code: "upgrade.source_changed", restartRequired: false };
    }
    return blockedProtection;
  });
  mount();
  pasteKey(await screen.findByLabelText("startupRecovery.sourceKey"), "a");
  fireEvent.click(
    screen.getByRole("button", { name: "startupRecovery.resume" }),
  );
  expect(
    await screen.findByText("startupRecovery.sourceChanged"),
  ).toBeVisible();
  await waitFor(() =>
    expect(screen.getByLabelText("startupRecovery.sourceKey")).toHaveValue(""),
  );
  expect(recoveryWrites()).toHaveLength(1);
  pasteKey(screen.getByLabelText("startupRecovery.sourceKey"), "b");
  fireEvent.click(
    screen.getByRole("button", { name: "startupRecovery.resume" }),
  );
  await waitFor(() => expect(recoveryWrites()).toHaveLength(2));
  expect(recoveryWrites()[1]).toEqual([
    "recover_startup_operation",
    { token: "fresh-operation-token", password: "b" },
  ]);
});
it("checks an unknown operation result instead of replaying it, and accepts only authoritative completion/restart", async () => {
  let attempted = false;
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "get_startup_recovery")
      return attempted ? completed : pending;
    if (command === "recover_startup_operation") {
      attempted = true;
      throw new Error("synthetic lost reply");
    }
    return blockedProtection;
  });
  mount();
  pasteKey(await screen.findByLabelText("startupRecovery.sourceKey"), "source");
  fireEvent.click(
    screen.getByRole("button", { name: "startupRecovery.resume" }),
  );
  expect(await screen.findByText("startupRecovery.completed")).toBeVisible();
  expect(recoveryWrites()).toHaveLength(1);
  expect(
    screen.queryByRole("button", { name: "startupRecovery.resume" }),
  ).not.toBeInTheDocument();
});
it.each([
  {
    ...pending,
    status: "verification_required",
    canRecover: false,
    restartRequired: true,
  },
  { ...pending, status: "pending", canRecover: false, restartRequired: false },
  {
    status: "available",
    canRecover: true,
    token: pending.token,
    restartRequired: false,
  },
])(
  "does not treat a resolved non-completion as a successful recovery (%j)",
  async (result) => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "recover_startup_operation" ? result : pending,
    );
    mount();
    pasteKey(
      await screen.findByLabelText("startupRecovery.sourceKey"),
      "source",
    );
    fireEvent.click(
      screen.getByRole("button", { name: "startupRecovery.resume" }),
    );
    await waitFor(() => expect(recoveryWrites()).toHaveLength(1));
    await waitFor(() =>
      expect(document.body).toHaveTextContent(
        /startupRecovery\.(verificationRequired|notCompleted|unknownResult)/,
      ),
    );
    expect(
      screen.queryByText("startupRecovery.completed"),
    ).not.toBeInTheDocument();
  },
);
it("keeps an ambiguous post-submit no-pending inspection from silently reopening setup or retrying", async () => {
  let attempted = false;
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "get_startup_recovery") {
      if (attempted) throw "secret.no_pending_operation";
      return pending;
    }
    if (command === "recover_startup_operation") {
      attempted = true;
      throw { code: "unknown", message: "private native detail" };
    }
    return blockedProtection;
  });
  mount();
  pasteKey(await screen.findByLabelText("startupRecovery.sourceKey"), "source");
  fireEvent.click(
    screen.getByRole("button", { name: "startupRecovery.resume" }),
  );
  expect(
    await screen.findByText("startupRecovery.unknownResult"),
  ).toBeVisible();
  expect(screen.queryByText("storageProtection.title")).not.toBeInTheDocument();
  expect(recoveryWrites()).toHaveLength(1);
  expect(document.body).not.toHaveTextContent("private native detail");
});
it("honors native restart-required rejection and never duplicates a finished operation", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "recover_startup_operation")
      throw { code: "secret.restart_required", restartRequired: true };
    if (command === "restart_app")
      throw new Error("synthetic-restart-private-detail");
    return pending;
  });
  mount();
  pasteKey(await screen.findByLabelText("startupRecovery.sourceKey"), "source");
  fireEvent.click(
    screen.getByRole("button", { name: "startupRecovery.resume" }),
  );
  const restart = await screen.findByRole("button", {
    name: "secrets.restart",
  });
  expect(
    screen.queryByLabelText("startupRecovery.sourceKey"),
  ).not.toBeInTheDocument();
  fireEvent.click(restart);
  expect(
    await screen.findByText("startupRecovery.restartFailed"),
  ).toBeVisible();
  expect(document.body).not.toHaveTextContent(
    "synthetic-restart-private-detail",
  );
  expect(recoveryWrites()).toHaveLength(1);
});
it("clears transient source input on recheck and unmount", async () => {
  const { unmount } = mount();
  const first = await screen.findByLabelText("startupRecovery.sourceKey");
  pasteKey(first, "synthetic-unsubmitted-source");
  fireEvent.click(
    screen.getByRole("button", { name: "startupRecovery.recheck" }),
  );
  expect(first).toHaveValue("");
  const next = await screen.findByLabelText("startupRecovery.sourceKey");
  pasteKey(next, "synthetic-final-source");
  unmount();
  expect(next).toHaveValue("");
});
it("rejects a non-string status instead of caching an invalid native DTO", async () => {
  vi.mocked(invoke).mockResolvedValue({ ...pending, status: ["pending"] });
  const { client } = mount();
  await waitFor(() =>
    expect(client.getQueryState(["startupRecovery"])?.status).toBe("error"),
  );
  expect(client.getQueryData(["startupRecovery"])).toBeUndefined();
});
