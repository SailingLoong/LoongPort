import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { save } from "@tauri-apps/plugin-dialog";
import { ZCodeBackupDialog } from "@/components/zcode/ZCodeBackupDialog";
import { zcodeBackupApi, type BundleExportResult } from "@/lib/api/zcodeBackup";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import ja from "@/i18n/locales/ja.json";

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));
vi.mock("@/lib/api/zcodeBackup", () => ({
  zcodeBackupApi: { exportBundle: vi.fn(), result: vi.fn() },
}));

const requestId = "10000000-0000-4000-8000-000000000001";
const profiles = [
  { id: "one", family: "bigmodel" as const, label: "al…@example.test" },
  { id: "two", family: "zai" as const, label: "bo…@example.test" },
];
function result(
  status: BundleExportResult["status"] = "saved",
  id = requestId,
): BundleExportResult {
  return {
    requestId: id,
    status,
    destination: status === "saved" ? "/synthetic/verified.zsb" : null,
    count: status === "saved" ? 1 : null,
    error: null,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function mount(
  extra: Partial<React.ComponentProps<typeof ZCodeBackupDialog>> = {},
) {
  const props = {
    open: true,
    onClose: vi.fn(),
    profiles,
    catalogRevision: "catalog-one",
    libraryDataRoot: "/synthetic/library-one",
    ...extra,
  };
  return { ...render(<ZCodeBackupDialog {...props} />), props };
}
function passwords(password = "synthetic password", confirmation = password) {
  fireEvent.change(screen.getByLabelText("Backup password"), {
    target: { value: password },
  });
  fireEvent.change(screen.getByLabelText("Confirm backup password"), {
    target: { value: confirmation },
  });
}
function prepare(password?: string) {
  fireEvent.click(screen.getByRole("checkbox", { name: /al…@example.test/ }));
  passwords(password);
}
function submit() {
  fireEvent.click(
    screen.getByRole("button", { name: "Choose location and back up" }),
  );
}
async function submitted() {
  submit();
  await waitFor(() =>
    expect(zcodeBackupApi.exportBundle).toHaveBeenCalledOnce(),
  );
}

describe("ZCode encrypted backup dialog", () => {
  beforeEach(() => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue(requestId);
    vi.mocked(save).mockReset().mockResolvedValue("/synthetic/chosen.zsb");
    vi.mocked(zcodeBackupApi.exportBundle)
      .mockReset()
      .mockResolvedValue(result());
    vi.mocked(zcodeBackupApi.result).mockReset().mockResolvedValue(result());
  });

  it("explains sensitive scope and original-environment limits before selecting a file", () => {
    mount();
    expect(screen.getByText(/sensitive sign-in sessions/)).toBeInTheDocument();
    expect(
      screen.getByText(/original operating system, user and home/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Closing this dialog does not undo a submitted backup/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Choose location and back up" }),
    ).toBeDisabled();
    expect(save).not.toHaveBeenCalled();
  });

  it.each([
    ["", ""],
    ["   ", "   "],
    ["synthetic password", "different password"],
    [" synthetic password ", "synthetic password"],
  ])(
    "requires a nonblank, exactly matching password pair (%j, %j)",
    (password, confirmation) => {
      mount();
      prepare();
      passwords(password, confirmation);
      expect(
        screen.getByRole("button", { name: "Choose location and back up" }),
      ).toBeDisabled();
      expect(save).not.toHaveBeenCalled();
    },
  );

  it("passes the exact password bytes and selection to one export, then trusts authenticated result only", async () => {
    mount();
    prepare("  synthetic 密码  ");
    const write = deferred<BundleExportResult>();
    vi.mocked(zcodeBackupApi.exportBundle).mockReturnValue(write.promise);
    await submitted();
    expect(save).toHaveBeenCalledExactlyOnceWith({
      defaultPath: "zcode-accounts.zsb",
      filters: [{ name: "ZCode encrypted backup", extensions: ["zsb"] }],
    });
    expect(zcodeBackupApi.exportBundle).toHaveBeenCalledExactlyOnceWith({
      requestId,
      dataRoot: "/synthetic/library-one",
      catalogRevision: "catalog-one",
      profileIds: ["one"],
      destination: "/synthetic/chosen.zsb",
      password: "  synthetic 密码  ",
      passwordConfirmation: "  synthetic 密码  ",
    });
    expect(
      screen.queryByText(/Backup saved and verified/),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("/synthetic/chosen.zsb")).not.toBeInTheDocument();
    expect(
      screen.queryByDisplayValue("  synthetic 密码  "),
    ).not.toBeInTheDocument();
    await act(async () => write.resolve(result()));
    expect(
      screen.getByText(/Backup saved and verified: 1 account/),
    ).toBeInTheDocument();
    expect(screen.getByText("/synthetic/verified.zsb")).toBeInTheDocument();
  });

  it("cancelling the native chooser does not export and clears passwords", async () => {
    vi.mocked(save).mockResolvedValue(null);
    mount();
    prepare();
    submit();
    await waitFor(() =>
      expect(screen.getByLabelText("Backup password")).toHaveValue(""),
    );
    expect(zcodeBackupApi.exportBundle).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("button", { name: "Query original result" }),
    ).not.toBeInTheDocument();
  });

  it("limits selection to 50 while allowing a selected account to be deselected", () => {
    mount({
      profiles: Array.from({ length: 51 }, (_, i) => ({
        id: `account-${i}`,
        family: "bigmodel",
        label: `Account ${i + 1}`,
      })),
    });
    const boxes = screen.getAllByRole("checkbox");
    for (const box of boxes.slice(0, 50)) fireEvent.click(box);
    expect(boxes[50]).toBeDisabled();
    expect(screen.getByText("Selected: 50 / 50")).toBeInTheDocument();
    fireEvent.click(boxes[0]);
    expect(boxes[50]).not.toBeDisabled();
    fireEvent.click(boxes[50]);
    expect(screen.getByText("Selected: 50 / 50")).toBeInTheDocument();
  });

  it("ignores double clicks while the chooser and write are pending", async () => {
    const chooser = deferred<string | null>();
    const write = deferred<BundleExportResult>();
    vi.mocked(save).mockReturnValue(chooser.promise);
    vi.mocked(zcodeBackupApi.exportBundle).mockReturnValue(write.promise);
    mount();
    prepare();
    const button = screen.getByRole("button", {
      name: "Choose location and back up",
    });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(save).toHaveBeenCalledOnce();
    await act(async () => chooser.resolve("/synthetic/chosen.zsb"));
    expect(zcodeBackupApi.exportBundle).toHaveBeenCalledOnce();
    expect(
      screen.queryByRole("button", { name: "Choose location and back up" }),
    ).not.toBeInTheDocument();
    await act(async () => write.resolve(result()));
  });

  it.each(["close", "unmount", "source", "catalog"])(
    "invalidates a late chooser after %s before it can export",
    async (change) => {
      const chooser = deferred<string | null>();
      vi.mocked(save).mockReturnValue(chooser.promise);
      const view = mount();
      prepare();
      submit();
      if (change === "close") {
        fireEvent.click(screen.getByRole("button", { name: "Close backup" }));
        expect(view.props.onClose).toHaveBeenCalledOnce();
      } else if (change === "unmount") view.unmount();
      else
        view.rerender(
          <ZCodeBackupDialog
            {...view.props}
            {...(change === "source"
              ? { libraryDataRoot: "/synthetic/library-two" }
              : { catalogRevision: "catalog-two" })}
          />,
        );
      await act(async () => chooser.resolve("/synthetic/stale.zsb"));
      expect(zcodeBackupApi.exportBundle).not.toHaveBeenCalled();
      expect(
        screen.queryByDisplayValue("synthetic password"),
      ).not.toBeInTheDocument();
    },
  );

  it("recovers a lost write reply by querying the same request without another export", async () => {
    vi.mocked(zcodeBackupApi.exportBundle).mockRejectedValue(
      new Error("secret-canary"),
    );
    mount();
    prepare();
    await submitted();
    await screen.findByText(/The backup result is unknown/);
    expect(screen.queryByText(/secret-canary/)).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    await screen.findByText(/Backup saved and verified/);
    expect(zcodeBackupApi.result).toHaveBeenCalledExactlyOnceWith(requestId);
    expect(zcodeBackupApi.exportBundle).toHaveBeenCalledOnce();
    expect(save).toHaveBeenCalledOnce();
  });

  it.each(["unknown", "failed"] as const)(
    "does not fabricate a path or success for %s",
    async (status) => {
      vi.mocked(zcodeBackupApi.exportBundle).mockResolvedValue(result(status));
      mount();
      prepare();
      await submitted();
      expect(
        screen.queryByText(/Backup saved and verified/),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByText(/\/synthetic\/.*\.zsb/),
      ).not.toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "Query original result" }),
      ).toBeEnabled();
    },
  );

  it("can query a pending write and ignores its older completion after a newer query", async () => {
    const write = deferred<BundleExportResult>();
    vi.mocked(zcodeBackupApi.exportBundle).mockReturnValue(write.promise);
    mount();
    prepare();
    await submitted();
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    await screen.findByText(/Backup saved and verified/);
    await act(async () => write.resolve(result("working")));
    expect(screen.getByText(/Backup saved and verified/)).toBeInTheDocument();
  });

  it.each(["write", "query"])(
    "ignores a late %s after closing and reopening",
    async (action) => {
      const pending = deferred<BundleExportResult>();
      vi.mocked(zcodeBackupApi.exportBundle).mockResolvedValue(
        result("working"),
      );
      if (action === "write")
        vi.mocked(zcodeBackupApi.exportBundle).mockReturnValueOnce(
          pending.promise,
        );
      else
        vi.mocked(zcodeBackupApi.result).mockReturnValueOnce(pending.promise);
      const view = mount();
      prepare();
      await submitted();
      if (action === "query")
        fireEvent.click(
          screen.getByRole("button", { name: "Query original result" }),
        );
      fireEvent.click(screen.getByRole("button", { name: "Close backup" }));
      view.rerender(<ZCodeBackupDialog {...view.props} open={false} />);
      view.rerender(<ZCodeBackupDialog {...view.props} />);
      passwords("new password");
      await act(async () => pending.resolve(result()));
      expect(screen.getByLabelText("Backup password")).toHaveValue(
        "new password",
      );
      expect(
        screen.queryByText(/Backup saved and verified/),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: "Query original result" }),
      ).not.toBeInTheDocument();
    },
  );

  it("sanitizes a chooser rejection and allows a fresh choice without starting an export", async () => {
    vi.mocked(save).mockRejectedValue(new Error("secret-canary"));
    mount();
    prepare();
    submit();
    await screen.findByRole("alert");
    expect(screen.queryByText(/secret-canary/)).not.toBeInTheDocument();
    expect(zcodeBackupApi.exportBundle).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Backup password")).toHaveValue("");
  });

  it.each([
    ["write", "source"],
    ["query", "source"],
    ["write", "catalog"],
    ["query", "catalog"],
    ["write", "unmount"],
    ["query", "unmount"],
  ])(
    "ignores a late %s after %s invalidates the local session",
    async (action, change) => {
      const pending = deferred<BundleExportResult>();
      vi.mocked(zcodeBackupApi.exportBundle).mockResolvedValue(
        result("working"),
      );
      if (action === "write")
        vi.mocked(zcodeBackupApi.exportBundle).mockReturnValueOnce(
          pending.promise,
        );
      else
        vi.mocked(zcodeBackupApi.result).mockReturnValueOnce(pending.promise);
      const view = mount();
      prepare();
      await submitted();
      if (action === "query")
        fireEvent.click(
          screen.getByRole("button", { name: "Query original result" }),
        );
      if (change === "unmount") view.unmount();
      else {
        view.rerender(
          <ZCodeBackupDialog
            {...view.props}
            {...(change === "source"
              ? { libraryDataRoot: "/synthetic/library-two" }
              : { catalogRevision: "catalog-two" })}
          />,
        );
        passwords("new password");
      }
      await act(async () => pending.resolve(result()));
      expect(
        screen.queryByText(/Backup saved and verified/),
      ).not.toBeInTheDocument();
      if (change !== "unmount")
        expect(screen.getByLabelText("Backup password")).toHaveValue(
          "new password",
        );
    },
  );

  it("deduplicates pending result queries and keeps rejection recovery on the same request", async () => {
    vi.mocked(zcodeBackupApi.exportBundle).mockResolvedValue(result("working"));
    const pending = deferred<BundleExportResult>();
    vi.mocked(zcodeBackupApi.result).mockReturnValueOnce(pending.promise);
    mount();
    prepare();
    await submitted();
    const button = screen.getByRole("button", {
      name: "Query original result",
    });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(zcodeBackupApi.result).toHaveBeenCalledExactlyOnceWith(requestId);
    await act(async () => pending.reject(new Error("secret-canary")));
    expect(
      screen.getByText(/The backup result is unknown/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/secret-canary/)).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    await screen.findByText(/Backup saved and verified/);
    expect(zcodeBackupApi.result).toHaveBeenLastCalledWith(requestId);
    expect(zcodeBackupApi.exportBundle).toHaveBeenCalledOnce();
  });

  it.each([
    ["en", en],
    ["zh", zh],
    ["zh-TW", zhTW],
    ["ja", ja],
  ] as const)(
    "provides the backup copy and count placeholders in %s",
    (_locale, tree) => {
      const backup = tree.zcode.accounts.backup;
      expect(backup).toBeDefined();
      expect(backup.title).toBeTruthy();
      expect(backup.sensitiveScope).toBeTruthy();
      expect(backup.environmentScope).toBeTruthy();
      expect(backup.selected).toContain("{{count}}");
      expect(backup.saved).toContain("{{count}}");
      expect(Object.keys(backup).sort()).toEqual(
        Object.keys(en.zcode.accounts.backup).sort(),
      );
    },
  );
});
