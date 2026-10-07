import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ZCodeOAuthAdd } from "@/components/zcode/ZCodeOAuthAdd";
import { zcodeLoginApi, type LoginProgress } from "@/lib/api/zcodeLogin";
import { settingsApi } from "@/lib/api/settings";

vi.mock("@/lib/api/zcodeLogin", () => ({
  zcodeLoginApi: {
    begin: vi.fn(),
    progress: vi.fn(),
    confirmKey: vi.fn(),
    declineKey: vi.fn(),
    save: vi.fn(),
    cancel: vi.fn(),
  },
}));
vi.mock("@/lib/api/settings", () => ({
  settingsApi: { openExternal: vi.fn() },
}));

function result(
  phase: LoginProgress["phase"] = "waiting",
  extra: Partial<LoginProgress> = {},
): LoginProgress {
  return {
    flowId: "flow-one",
    phase,
    family: "bigmodel",
    authorization: {
      url: "https://example.test/authorize",
      expiresAt: 2_000_000_000,
      pollIntervalSec: 60,
    },
    account:
      phase === "waiting"
        ? null
        : {
            id: "opaque",
            label: "al…@example.test",
            duplicate: false,
            identitySource: "officialLogin",
          },
    project:
      phase === "waiting"
        ? null
        : {
            organizationId: "org-one",
            organizationName: "Personal organization",
            projectId: "project-one",
            projectName: "Personal project",
          },
    connections:
      phase === "waiting"
        ? null
        : {
            start: "ready",
            coding: phase === "keyRequired" ? "unavailable" : "ready",
            needsKey: phase === "keyRequired",
          },
    keyCreated: false,
    keyMayExist: false,
    keyManagementUrl: "https://example.test/keys",
    error: null,
    saved: phase === "saved" ? { id: "opaque", outcome: "saved" } : null,
    ...extra,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (value: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function mount() {
  const onClose = vi.fn();
  const onSaved = vi.fn(async () => {});
  const view = render(
    <ZCodeOAuthAdd open onClose={onClose} onSaved={onSaved} />,
  );
  return { ...view, onClose, onSaved };
}
async function begin() {
  fireEvent.click(
    screen.getByRole("button", { name: "Continue official sign-in" }),
  );
  await screen.findByRole("button", { name: "Query original result" });
}
async function query(next: LoginProgress) {
  vi.mocked(zcodeLoginApi.progress).mockResolvedValue(next);
  fireEvent.click(
    screen.getByRole("button", { name: "Query original result" }),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Cancel reading" }),
    ).not.toBeInTheDocument(),
  );
}

describe("ZCode official login dialog", () => {
  beforeEach(() => {
    vi.mocked(zcodeLoginApi.begin).mockReset().mockResolvedValue(result());
    vi.mocked(zcodeLoginApi.progress).mockReset().mockResolvedValue(result());
    vi.mocked(zcodeLoginApi.confirmKey)
      .mockReset()
      .mockResolvedValue(result("review", { keyCreated: true }));
    vi.mocked(zcodeLoginApi.declineKey)
      .mockReset()
      .mockResolvedValue(
        result("review", {
          connections: {
            start: "ready",
            coding: "unavailable",
            needsKey: true,
          },
        }),
      );
    vi.mocked(zcodeLoginApi.save)
      .mockReset()
      .mockResolvedValue(result("saved"));
    vi.mocked(zcodeLoginApi.cancel)
      .mockReset()
      .mockResolvedValue(result("cancelled"));
    vi.mocked(settingsApi.openExternal)
      .mockReset()
      .mockResolvedValue(undefined);
  });

  it("explains the official browser flow and opens the returned authorization once", async () => {
    const pending = deferred<LoginProgress>();
    vi.mocked(zcodeLoginApi.begin).mockReturnValue(pending.promise);
    mount();
    expect(
      screen.getByText(/Passwords are entered only on the official page/),
    ).toBeInTheDocument();
    const button = screen.getByRole("button", {
      name: "Continue official sign-in",
    });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(zcodeLoginApi.begin).toHaveBeenCalledExactlyOnceWith("bigmodel");
    await act(async () => pending.resolve(result()));
    expect(settingsApi.openExternal).toHaveBeenCalledExactlyOnceWith(
      "https://example.test/authorize",
    );
    expect(zcodeLoginApi.save).not.toHaveBeenCalled();
  });

  it("keeps a failed begin retryable without claiming an account or asking for an unavailable result", async () => {
    vi.mocked(zcodeLoginApi.begin).mockRejectedValueOnce(
      new Error("private-begin-response"),
    );
    mount();
    fireEvent.click(
      screen.getByRole("button", { name: "Continue official sign-in" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "No official authorization result was received. Try starting official sign-in again.",
    );
    expect(
      screen.queryByRole("button", { name: "Query original result" }),
    ).not.toBeInTheDocument();
    expect(settingsApi.openExternal).not.toHaveBeenCalled();
    expect(zcodeLoginApi.save).not.toHaveBeenCalled();
    await begin();
    expect(zcodeLoginApi.begin).toHaveBeenCalledTimes(2);
  });

  it("requires specific account and personal project consent before creating a Key", async () => {
    mount();
    await begin();
    await query(result("keyRequired"));
    expect(screen.getByText("al…@example.test")).toBeInTheDocument();
    expect(
      screen.getByText("Personal organization", { selector: "dd" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Personal project", { selector: "dd" }),
    ).toBeInTheDocument();
    expect(screen.getByText("zcode-api-key")).toBeInTheDocument();
    expect(screen.getByText(/Encrypted on this computer/)).toBeInTheDocument();
    expect(zcodeLoginApi.confirmKey).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Open official Key management" }),
    );
    await waitFor(() =>
      expect(settingsApi.openExternal).toHaveBeenLastCalledWith(
        "https://example.test/keys",
      ),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Authorize creating and saving this Key",
      }),
    );
    await screen.findByRole("button", {
      name: "Save to encrypted account vault",
    });
    expect(zcodeLoginApi.confirmKey).toHaveBeenCalledExactlyOnceWith(
      "flow-one",
      "org-one",
      "project-one",
    );
    expect(zcodeLoginApi.save).not.toHaveBeenCalled();
  });

  it("declines Key creation but preserves the backend Start and pending Coding facts", async () => {
    mount();
    await begin();
    await query(result("keyRequired"));
    fireEvent.click(
      screen.getByRole("button", { name: "Not now; keep connection pending" }),
    );
    await screen.findByRole("button", {
      name: "Save to encrypted account vault",
    });
    expect(zcodeLoginApi.declineKey).toHaveBeenCalledExactlyOnceWith(
      "flow-one",
    );
    expect(zcodeLoginApi.confirmKey).not.toHaveBeenCalled();
    expect(screen.getByText("Start Plan: Ready")).toBeInTheDocument();
    expect(screen.getByText("Coding Plan: Unavailable")).toBeInTheDocument();
    expect(
      screen.getByText(/Coding connection is pending/),
    ).toBeInTheDocument();
  });

  it.each([false, true])(
    "defaults a duplicate to keep and requires explicit update (%s)",
    async (update) => {
      mount();
      await begin();
      await query(
        result("review", {
          account: {
            id: "opaque",
            label: "al…@example.test",
            duplicate: true,
            identitySource: "officialLogin",
          },
        }),
      );
      const choice = screen.getByRole("checkbox", {
        name: "Explicitly update this existing account",
      });
      expect(choice).not.toBeChecked();
      if (update) fireEvent.click(choice);
      fireEvent.click(
        screen.getByRole("button", { name: "Save to encrypted account vault" }),
      );
      await screen.findByRole("button", { name: "Add another account" });
      expect(zcodeLoginApi.save).toHaveBeenCalledExactlyOnceWith(
        "flow-one",
        update,
      );
    },
  );

  it("queries a lost Key response from the consent stage without repeating creation", async () => {
    vi.mocked(zcodeLoginApi.confirmKey).mockRejectedValue(
      new Error("private-key-response"),
    );
    mount();
    await begin();
    await query(result("keyRequired"));
    fireEvent.click(
      screen.getByRole("button", {
        name: "Authorize creating and saving this Key",
      }),
    );
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", {
        name: "Authorize creating and saving this Key",
      }),
    ).toBeDisabled();
    expect(screen.queryByText(/private-key-response/)).not.toBeInTheDocument();
    await query(result("review", { keyCreated: true, keyMayExist: true }));
    expect(zcodeLoginApi.confirmKey).toHaveBeenCalledTimes(1);
    expect(
      screen.getByRole("button", { name: "Save to encrypted account vault" }),
    ).toBeEnabled();
  });

  it("recovers a lost save and refreshes the list once without replaying Save", async () => {
    vi.mocked(zcodeLoginApi.save).mockRejectedValue(
      new Error("private-save-response"),
    );
    const { onSaved } = mount();
    await begin();
    await query(result("review"));
    fireEvent.click(
      screen.getByRole("button", { name: "Save to encrypted account vault" }),
    );
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", { name: "Save to encrypted account vault" }),
    ).toBeDisabled();
    await query(result("saved"));
    expect(onSaved).toHaveBeenCalledTimes(1);
    await query(result("saved"));
    expect(onSaved).toHaveBeenCalledTimes(1);
    expect(zcodeLoginApi.save).toHaveBeenCalledTimes(1);
  });

  it("retains a saved receipt when the list refresh fails", async () => {
    const { onSaved } = mount();
    onSaved.mockRejectedValue(new Error("private-refresh"));
    await begin();
    await query(result("review"));
    fireEvent.click(
      screen.getByRole("button", { name: "Save to encrypted account vault" }),
    );
    await screen.findByText(
      /Account saved, but the account list could not refresh/,
    );
    expect(
      screen.getByRole("button", { name: "Add another account" }),
    ).toBeEnabled();
    expect(
      screen.queryByRole("button", { name: "Save to encrypted account vault" }),
    ).not.toBeInTheDocument();
    await query(result("saved"));
    expect(onSaved).toHaveBeenCalledTimes(1);
  });

  it.each(["key", "save"] as const)(
    "requires an explicit backend retry remedy and a fresh click after lost %s response",
    async (action) => {
      const phase = action === "key" ? "keyRequired" : "review";
      const api =
        action === "key" ? zcodeLoginApi.confirmKey : zcodeLoginApi.save;
      vi.mocked(api).mockRejectedValueOnce(new Error("lost response"));
      mount();
      await begin();
      await query(result(phase));
      const name =
        action === "key"
          ? "Authorize creating and saving this Key"
          : "Save to encrypted account vault";
      fireEvent.click(screen.getByRole("button", { name }));
      await screen.findByRole("alert");
      await query(
        result(phase, {
          error: {
            code: "zcode.account.storage_failed",
            remedy: "queryOriginal",
            committed: false,
          },
        }),
      );
      expect(screen.getByRole("button", { name })).toBeDisabled();
      await query(
        result(phase, {
          error: {
            code: "zcode.account.storage_failed",
            remedy: action === "key" ? "retryKeyConsent" : "retrySave",
            committed: false,
          },
        }),
      );
      expect(api).toHaveBeenCalledTimes(1);
      expect(screen.getByRole("button", { name })).toBeEnabled();
      fireEvent.click(screen.getByRole("button", { name }));
      await waitFor(() => expect(api).toHaveBeenCalledTimes(2));
    },
  );

  it("keeps a non-retryable backend save error queryable without another Save", async () => {
    vi.mocked(zcodeLoginApi.save).mockResolvedValue(
      result("review", {
        error: {
          code: "zcode.account.storage_failed",
          remedy: "queryOriginal",
          committed: false,
        },
      }),
    );
    mount();
    await begin();
    await query(result("review"));
    fireEvent.click(
      screen.getByRole("button", { name: "Save to encrypted account vault" }),
    );
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", { name: "Save to encrypted account vault" }),
    ).toBeDisabled();
    await query(result("saved"));
    expect(zcodeLoginApi.save).toHaveBeenCalledTimes(1);
  });

  it("cancels an in-flight read locally and ignores its late saved reply", async () => {
    const pending = deferred<LoginProgress>();
    const { onSaved } = mount();
    await begin();
    vi.mocked(zcodeLoginApi.progress).mockReturnValue(pending.promise);
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel reading" }));
    await act(async () => pending.resolve(result("saved")));
    expect(onSaved).not.toHaveBeenCalled();
    expect(
      screen.getByRole("heading", {
        name: "Waiting for official authorization",
      }),
    ).toBeInTheDocument();
    expect(zcodeLoginApi.cancel).not.toHaveBeenCalled();
    await query(result("review"));
    expect(
      screen.getByRole("button", { name: "Save to encrypted account vault" }),
    ).toBeEnabled();
  });

  it.each(["close", "escape", "unmount", "closed prop"])(
    "cancels exactly this flow on %s",
    async (action) => {
      const { unmount, rerender, onClose, onSaved } = mount();
      await begin();
      if (action === "close")
        fireEvent.click(screen.getByRole("button", { name: "Close sign-in" }));
      if (action === "escape")
        fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
      if (action === "unmount") unmount();
      if (action === "closed prop")
        rerender(
          <ZCodeOAuthAdd open={false} onClose={onClose} onSaved={onSaved} />,
        );
      await waitFor(() =>
        expect(zcodeLoginApi.cancel).toHaveBeenCalledExactlyOnceWith(
          "flow-one",
        ),
      );
      expect(zcodeLoginApi.save).not.toHaveBeenCalled();
    },
  );

  it("cancels a late begin from a closed dialog without opening the old browser", async () => {
    const pending = deferred<LoginProgress>();
    vi.mocked(zcodeLoginApi.begin).mockReturnValueOnce(pending.promise);
    const { onClose, onSaved, rerender } = mount();
    fireEvent.click(
      screen.getByRole("button", { name: "Continue official sign-in" }),
    );
    rerender(
      <ZCodeOAuthAdd open={false} onClose={onClose} onSaved={onSaved} />,
    );
    rerender(<ZCodeOAuthAdd open onClose={onClose} onSaved={onSaved} />);
    vi.mocked(zcodeLoginApi.begin).mockResolvedValue(
      result("waiting", { flowId: "flow-two" }),
    );
    await begin();
    await act(async () => pending.resolve(result()));
    expect(zcodeLoginApi.cancel).toHaveBeenCalledWith("flow-one");
    expect(settingsApi.openExternal).toHaveBeenCalledTimes(1);
    await query(result("review", { flowId: "flow-two" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Save to encrypted account vault" }),
    );
    expect(zcodeLoginApi.save).toHaveBeenCalledWith("flow-two", false);
  });

  it("preserves the stage on read errors and sanitizes backend error text", async () => {
    mount();
    await begin();
    await query(result("keyRequired"));
    vi.mocked(zcodeLoginApi.progress).mockRejectedValue({
      code: "private-secret",
      remedy: "private-url",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    await screen.findByRole("alert");
    expect(
      screen.getByRole("heading", {
        name: "Complete the Coding Plan connection",
      }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/private-/)).not.toBeInTheDocument();
  });

  it("uses the selected family and displays unknown connections without inventing readiness", async () => {
    vi.mocked(zcodeLoginApi.begin).mockResolvedValue(
      result("waiting", { family: "zai" }),
    );
    mount();
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("option", { name: "z.ai" }));
    await begin();
    expect(zcodeLoginApi.begin).toHaveBeenCalledExactlyOnceWith("zai");
    await query(
      result("review", {
        family: "zai",
        connections: {
          start: "unknown",
          coding: "unavailable",
          needsKey: true,
        },
      }),
    );
    expect(screen.getByText("z.ai", { selector: "dd" })).toBeInTheDocument();
    expect(screen.getByText("Start Plan: Unknown")).toBeInTheDocument();
    expect(screen.getByText("Coding Plan: Unavailable")).toBeInTheDocument();
    expect(screen.queryByText("Start Plan: Ready")).not.toBeInTheDocument();
  });

  it("queries waiting and preparing snapshots, then stops automatic reads for consent", async () => {
    vi.useFakeTimers();
    const { unmount } = mount();
    try {
      vi.mocked(zcodeLoginApi.begin).mockResolvedValue(
        result("waiting", {
          authorization: {
            url: "https://example.test/authorize",
            expiresAt: 1,
            pollIntervalSec: 2,
          },
        }),
      );
      await act(async () =>
        fireEvent.click(
          screen.getByRole("button", { name: "Continue official sign-in" }),
        ),
      );
      expect(
        screen.getByRole("heading", {
          name: "Waiting for official authorization",
        }),
      ).toBeInTheDocument();
      expect(zcodeLoginApi.progress).not.toHaveBeenCalled();
      vi.mocked(zcodeLoginApi.progress).mockResolvedValue(
        result("preparing", { authorization: null }),
      );
      await act(async () => vi.advanceTimersByTimeAsync(2000));
      expect(
        screen.getByRole("heading", {
          name: "Identity verified; preparing connections",
        }),
      ).toBeInTheDocument();
      vi.mocked(zcodeLoginApi.progress).mockResolvedValue(
        result("keyRequired"),
      );
      await act(async () => vi.advanceTimersByTimeAsync(2000));
      expect(
        screen.getByRole("heading", {
          name: "Complete the Coding Plan connection",
        }),
      ).toBeInTheDocument();
      await act(async () => vi.advanceTimersByTimeAsync(120_000));
      expect(zcodeLoginApi.progress).toHaveBeenCalledTimes(2);
      expect(zcodeLoginApi.confirmKey).not.toHaveBeenCalled();
    } finally {
      unmount();
      vi.useRealTimers();
    }
  });

  it.each(["key", "save"] as const)(
    "ignores a late %s result after a newer flow starts",
    async (action) => {
      const pending = deferred<LoginProgress>();
      const api =
        action === "key" ? zcodeLoginApi.confirmKey : zcodeLoginApi.save;
      vi.mocked(api).mockReturnValueOnce(pending.promise);
      const { onSaved, onClose, rerender } = mount();
      await begin();
      await query(result(action === "key" ? "keyRequired" : "review"));
      const button = screen.getByRole("button", {
        name:
          action === "key"
            ? "Authorize creating and saving this Key"
            : "Save to encrypted account vault",
      });
      fireEvent.click(button);
      fireEvent.click(button);
      expect(api).toHaveBeenCalledTimes(1);
      rerender(
        <ZCodeOAuthAdd open={false} onClose={onClose} onSaved={onSaved} />,
      );
      rerender(<ZCodeOAuthAdd open onClose={onClose} onSaved={onSaved} />);
      vi.mocked(zcodeLoginApi.begin).mockResolvedValue(
        result("waiting", { flowId: "flow-two" }),
      );
      await begin();
      await act(async () =>
        pending.resolve(result(action === "key" ? "review" : "saved")),
      );
      expect(
        screen.getByRole("heading", {
          name: "Waiting for official authorization",
        }),
      ).toBeInTheDocument();
      expect(onSaved).not.toHaveBeenCalled();
      expect(zcodeLoginApi.cancel).toHaveBeenCalledWith("flow-one");
    },
  );

  it("prevents duplicate Key creation when the backend says a Key may already exist", async () => {
    mount();
    await begin();
    await query(result("keyRequired", { keyMayExist: true }));
    expect(
      screen.getByRole("button", {
        name: "Authorize creating and saving this Key",
      }),
    ).toBeDisabled();
    expect(
      screen.getByText(/official Key may already exist/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Open official Key management" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Query original result" }),
    ).toBeEnabled();
    expect(zcodeLoginApi.confirmKey).not.toHaveBeenCalled();
  });

  it("starts another independent account after saving and retires the old flow", async () => {
    const { onSaved } = mount();
    await begin();
    await query(result("review"));
    fireEvent.click(
      screen.getByRole("button", { name: "Save to encrypted account vault" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Add another account" }),
    );
    expect(zcodeLoginApi.cancel).toHaveBeenCalledExactlyOnceWith("flow-one");
    expect(
      screen.getByRole("heading", { name: "Add a sign-in account" }),
    ).toBeInTheDocument();
    vi.mocked(zcodeLoginApi.begin).mockResolvedValue(
      result("waiting", { flowId: "flow-two" }),
    );
    await begin();
    await query(result("review", { flowId: "flow-two" }));
    vi.mocked(zcodeLoginApi.save).mockResolvedValue(
      result("saved", {
        flowId: "flow-two",
        saved: { id: "opaque-two", outcome: "saved" },
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Save to encrypted account vault" }),
    );
    await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(2));
    expect(zcodeLoginApi.save).toHaveBeenLastCalledWith("flow-two", false);
  });

  it("cancels the old source and ignores its late reply when the library root changes", async () => {
    const pending = deferred<LoginProgress>();
    const onSaved = vi.fn(async () => {});
    const onClose = vi.fn();
    const { rerender } = render(
      <ZCodeOAuthAdd
        open
        onClose={onClose}
        onSaved={onSaved}
        libraryDataRoot="/synthetic/one"
      />,
    );
    await begin();
    expect(zcodeLoginApi.begin).toHaveBeenCalledExactlyOnceWith(
      "bigmodel",
      "/synthetic/one",
    );
    vi.mocked(zcodeLoginApi.progress).mockReturnValueOnce(pending.promise);
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    rerender(
      <ZCodeOAuthAdd
        open
        onClose={onClose}
        onSaved={onSaved}
        libraryDataRoot="/synthetic/two"
      />,
    );
    expect(zcodeLoginApi.cancel).toHaveBeenCalledExactlyOnceWith("flow-one");
    vi.mocked(zcodeLoginApi.begin).mockResolvedValue(
      result("waiting", { flowId: "flow-two" }),
    );
    await begin();
    expect(zcodeLoginApi.begin).toHaveBeenLastCalledWith(
      "bigmodel",
      "/synthetic/two",
    );
    await act(async () => pending.resolve(result("saved")));
    expect(onSaved).not.toHaveBeenCalled();
    expect(
      screen.getByRole("heading", {
        name: "Waiting for official authorization",
      }),
    ).toBeInTheDocument();
  });
});
