import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ZCodeAccountPanel } from "@/components/zcode/ZCodeAccountPanel";
import {
  zcodeAccountsApi,
  type CatalogStatus,
  type RecoveryStatus,
} from "@/lib/api/zcodeAccounts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@/lib/api/zcodeAccounts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/zcodeAccounts")>()),
  zcodeAccountsApi: {
    inspect: vi.fn(),
    status: vi.fn(),
    capture: vi.fn(),
    switch: vi.fn(),
    recoveryStatus: vi.fn(),
    archive: vi.fn(),
    confirmRecovery: vi.fn(),
    recapture: vi.fn(),
    deleteRecovery: vi.fn(),
  },
}));
const source = {
  installPath: "/Applications/ZCode.app",
  dataRoot: "/example/.zcode/v2",
  keyMode: "standard" as const,
};
const context = {
  contextId: "context-one",
  contextRevision: "context-revision",
  dataRoot: source.dataRoot,
  family: "zai" as const,
  version: "1.0",
  build: "fixture",
};
const catalog: CatalogStatus = {
  revision: "catalog-one",
  profiles: [
    { id: "opaque-zai", family: "zai", label: "Personal Z.ai" },
    { id: "opaque-bigmodel", family: "bigmodel", label: null },
  ],
  current: null,
  pending: false,
  nativeUnconfirmed: false,
};
const recovery: RecoveryStatus = {
  revision: "recovery-one",
  pending: false,
  nativeUnconfirmed: false,
  records: [],
};
function mount(onBusyChange = vi.fn()) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ZCodeAccountPanel onBusyChange={onBusyChange} />
    </QueryClientProvider>,
  );
  return client;
}
async function inspect() {
  fireEvent.change(screen.getByLabelText("ZCode data directory"), {
    target: { value: source.dataRoot },
  });
  fireEvent.click(
    screen.getByRole("checkbox", {
      name: "Use only the standard local key to verify the selected data",
    }),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Inspect selected source" }),
  );
  await screen.findByText("Personal Z.ai");
}
function confirm(name: string) {
  fireEvent.click(
    within(screen.getByRole("dialog")).getByRole("button", { name }),
  );
}
beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(zcodeAccountsApi.inspect).mockReset().mockResolvedValue(context);
  vi.mocked(zcodeAccountsApi.status).mockReset().mockResolvedValue(catalog);
  vi.mocked(zcodeAccountsApi.recoveryStatus)
    .mockReset()
    .mockResolvedValue(recovery);
  vi.mocked(zcodeAccountsApi.capture).mockReset().mockResolvedValue("saved");
  vi.mocked(zcodeAccountsApi.switch).mockReset().mockResolvedValue("switched");
  vi.mocked(zcodeAccountsApi.archive).mockReset().mockResolvedValue("archived");
  vi.mocked(zcodeAccountsApi.confirmRecovery)
    .mockReset()
    .mockResolvedValue(undefined);
  vi.mocked(zcodeAccountsApi.recapture).mockReset().mockResolvedValue("saved");
  vi.mocked(zcodeAccountsApi.deleteRecovery)
    .mockReset()
    .mockResolvedValue(undefined);
});
describe("ZCode saved accounts", () => {
  it("loads only local recovery on mount, requires reviewed source and never captures on inspect or cancel", async () => {
    mount();
    await waitFor(() =>
      expect(zcodeAccountsApi.recoveryStatus).toHaveBeenCalledTimes(1),
    );
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.status).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Inspect selected source" }),
    ).toBeDisabled();
    await inspect();
    expect(zcodeAccountsApi.inspect).toHaveBeenCalledWith(source);
    expect(zcodeAccountsApi.status).toHaveBeenCalledWith(
      source,
      context.contextRevision,
    );
    expect(zcodeAccountsApi.capture).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Save current account" }),
    );
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent(source.dataRoot);
    expect(dialog).toHaveTextContent("encrypted");
    expect(dialog).toHaveTextContent("switch");
    confirm("Cancel account action");
    expect(zcodeAccountsApi.capture).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.switch).not.toHaveBeenCalled();
  });
  it("serializes repeat capture clicks, passes reviewed revisions, and keeps navigation blocked through status refresh", async () => {
    let finish!: (value: "saved") => void;
    vi.mocked(zcodeAccountsApi.capture).mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const onBusy = vi.fn();
    mount(onBusy);
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Save current account" }),
    );
    confirm("Save encrypted account");
    confirm("Save encrypted account");
    await waitFor(() =>
      expect(zcodeAccountsApi.capture).toHaveBeenCalledTimes(1),
    );
    expect(zcodeAccountsApi.capture).toHaveBeenCalledWith(
      source,
      "context-revision",
      "catalog-one",
    );
    expect(onBusy).toHaveBeenLastCalledWith(true);
    let refreshDone!: (value: CatalogStatus) => void;
    vi.mocked(zcodeAccountsApi.status).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          refreshDone = resolve;
        }),
    );
    await act(async () => finish("saved"));
    await waitFor(() =>
      expect(zcodeAccountsApi.status).toHaveBeenCalledTimes(2),
    );
    expect(onBusy).toHaveBeenLastCalledWith(true);
    await act(async () => refreshDone(catalog));
    await screen.findByText("Account saved locally.");
    await waitFor(() => expect(onBusy).toHaveBeenLastCalledWith(false));
    expect(zcodeAccountsApi.status).toHaveBeenCalledTimes(2);
  });
  it("separates families, blocks cross-family targets and switches only the selected opaque ID without optimistic current state", async () => {
    mount();
    await inspect();
    const otherFamily = screen.getByRole("region", {
      name: "BigModel accounts",
    });
    expect(
      within(otherFamily).getByRole("button", { name: "Switch saved account" }),
    ).toBeDisabled();
    const sameFamily = screen.getByRole("region", { name: "Z.ai accounts" });
    fireEvent.click(
      within(sameFamily).getByRole("button", { name: "Switch saved account" }),
    );
    confirm("Switch to this account");
    await waitFor(() =>
      expect(zcodeAccountsApi.switch).toHaveBeenCalledWith(
        source,
        "context-revision",
        "opaque-zai",
        "catalog-one",
      ),
    );
    expect(screen.getByText("Current account: unknown")).toBeInTheDocument();
  });
  it("invalidates inspected context when source selection changes", async () => {
    mount();
    await inspect();
    fireEvent.change(screen.getByLabelText("ZCode data directory"), {
      target: { value: "/different/.zcode/v2" },
    });
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    expect(screen.queryByText("Personal Z.ai")).not.toBeInTheDocument();
    expect(zcodeAccountsApi.capture).not.toHaveBeenCalled();
  });
  it.each(["key_context_unknown", "team_unsupported", "native_gate_pending"])(
    "shows safe actionable %s errors without raw canary content",
    async (code) => {
      vi.mocked(zcodeAccountsApi.inspect).mockRejectedValue({
        code: `zcode.account.${code}`,
        remedy: "openNativeSettings",
        committed: false,
        message: "secret-canary-token",
      });
      const client = mount();
      fireEvent.change(screen.getByLabelText("ZCode data directory"), {
        target: { value: source.dataRoot },
      });
      fireEvent.click(
        screen.getByRole("checkbox", {
          name: "Use only the standard local key to verify the selected data",
        }),
      );
      fireEvent.click(
        screen.getByRole("button", { name: "Inspect selected source" }),
      );
      await screen.findByRole("alert");
      expect(
        screen.getByRole("button", { name: "Save current account" }),
      ).toBeDisabled();
      expect(document.body.textContent).not.toContain("secret-canary");
      expect(
        JSON.stringify(
          client
            .getQueryCache()
            .getAll()
            .map((q) => q.state),
        ),
      ).not.toContain("secret-canary");
    },
  );
  it("requires a new review after a stale revision and never replays a mutation on refresh", async () => {
    vi.mocked(zcodeAccountsApi.capture).mockRejectedValueOnce({
      code: "zcode.account.catalog_changed",
      remedy: "refreshContext",
      committed: false,
    });
    mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Save current account" }),
    );
    confirm("Save encrypted account");
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Refresh account status" }),
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.status).toHaveBeenCalledTimes(2),
    );
    expect(zcodeAccountsApi.capture).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("retains a committed-but-recovery-required warning without success or automatic retry", async () => {
    vi.mocked(zcodeAccountsApi.switch).mockRejectedValueOnce({
      code: "zcode.account.committed_recovery_required",
      remedy: "reviewRecovery",
      committed: true,
      message: "secret-canary-token",
    });
    mount();
    await inspect();
    fireEvent.click(
      within(screen.getByRole("region", { name: "Z.ai accounts" })).getByRole(
        "button",
        { name: "Switch saved account" },
      ),
    );
    confirm("Switch to this account");
    expect(await screen.findByRole("alert")).toHaveTextContent("committed");
    expect(
      screen.queryByText(
        `Switched ${source.dataRoot} locally. Start official ZCode and verify the account there.`,
      ),
    ).not.toBeInTheDocument();
    expect(zcodeAccountsApi.switch).toHaveBeenCalledTimes(1);
    expect(document.body.textContent).not.toContain("secret-canary");
  });
  it("keeps local archive and confirmed-record cleanup available when source inspection fails", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      pending: true,
      nativeUnconfirmed: true,
      records: [
        {
          id: "old-confirmed",
          disposition: "full-before",
          latestCompleted: false,
        },
        {
          id: "old-unconfirmed",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    });
    mount();
    await screen.findByText("old-confirmed");
    fireEvent.click(
      screen.getByRole("button", { name: "Archive pending recovery" }),
    );
    confirm("Preserve pending recovery");
    await waitFor(() =>
      expect(zcodeAccountsApi.archive).toHaveBeenCalledWith("recovery-one"),
    );
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    const record = screen.getByRole("group", {
      name: "Recovery record old-confirmed",
    });
    expect(
      within(record).getByRole("button", {
        name: "Permanently delete recovery record",
      }),
    ).toBeEnabled();
    const unconfirmed = screen.getByRole("group", {
      name: "Recovery record old-unconfirmed",
    });
    expect(
      within(unconfirmed).getByRole("button", {
        name: "Permanently delete recovery record",
      }),
    ).toBeDisabled();
  });
  it("allows explicit recovery confirmation with an inspected source when the saved catalog is invalid", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      nativeUnconfirmed: true,
      records: [
        {
          id: "unconfirmed-record",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    });
    vi.mocked(zcodeAccountsApi.status).mockRejectedValue({
      code: "zcode.account.saved_data_invalid",
      remedy: "reviewSavedData",
      committed: false,
    });
    mount();
    fireEvent.change(screen.getByLabelText("ZCode data directory"), {
      target: { value: source.dataRoot },
    });
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Use only the standard local key to verify the selected data",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Inspect selected source" }),
    );
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Recapture after official sign-in" }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm this recovery record" }),
    );
    confirm("Check selected recovery record");
    await waitFor(() =>
      expect(zcodeAccountsApi.confirmRecovery).toHaveBeenCalledWith(
        source,
        "context-revision",
        "unconfirmed-record",
        "recovery-one",
      ),
    );
    expect(zcodeAccountsApi.capture).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.recapture).not.toHaveBeenCalled();
  });
  it("ignores an unrelated catalog revision change while confirming the reviewed recovery record", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      nativeUnconfirmed: true,
      records: [
        {
          id: "unconfirmed-record",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    });
    const client = mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm this recovery record" }),
    );
    await act(async () => {
      client.setQueryData(
        [
          "zcodeAccountCatalog",
          source.installPath,
          source.dataRoot,
          source.keyMode,
          context.contextRevision,
        ],
        {
          ...catalog,
          revision: "unrelated-catalog-new",
          profiles: [
            { ...catalog.profiles[0], label: "Fresh catalog label" },
            catalog.profiles[1],
          ],
        },
      );
    });
    await screen.findByText("Fresh catalog label");
    confirm("Check selected recovery record");
    await waitFor(() =>
      expect(zcodeAccountsApi.confirmRecovery).toHaveBeenCalledWith(
        source,
        "context-revision",
        "unconfirmed-record",
        "recovery-one",
      ),
    );
    expect(zcodeAccountsApi.confirmRecovery).toHaveBeenCalledTimes(1);
  });
  it("refuses recovery confirmation if the reviewed recovery revision changes", async () => {
    const initialRecovery: RecoveryStatus = {
      ...recovery,
      nativeUnconfirmed: true,
      records: [
        {
          id: "unconfirmed-record",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    };
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue(
      initialRecovery,
    );
    const client = mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm this recovery record" }),
    );
    await act(async () => {
      client.setQueryData(["zcodeAccountRecovery"], {
        ...initialRecovery,
        revision: "recovery-new",
        records: [
          ...initialRecovery.records,
          {
            id: "new-recovery-record",
            disposition: "full-after",
            latestCompleted: true,
          },
        ],
      });
    });
    await screen.findByText("new-recovery-record");
    confirm("Check selected recovery record");
    await screen.findByRole("alert");
    expect(zcodeAccountsApi.confirmRecovery).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("discards a reviewed recovery confirmation when its selected source changes", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      nativeUnconfirmed: true,
      records: [
        {
          id: "unconfirmed-record",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    });
    mount();
    await inspect();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm this recovery record" }),
    );
    confirm("Cancel account action");
    fireEvent.change(screen.getByLabelText("ZCode data directory"), {
      target: { value: "/different/.zcode/v2" },
    });
    expect(
      screen.getByRole("button", { name: "Confirm this recovery record" }),
    ).toBeDisabled();
    expect(zcodeAccountsApi.confirmRecovery).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("blocks ordinary actions when pending and full, but permits exact-record confirmation, recapture and archive deduplication", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      pending: true,
      nativeUnconfirmed: true,
      records: [
        {
          id: "old-one",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
        {
          id: "old-two",
          disposition: "native-unconfirmed",
          latestCompleted: false,
        },
      ],
    });
    mount();
    await inspect();
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    expect(
      screen
        .getAllByRole("button", { name: "Switch saved account" })
        .every((button) => button.hasAttribute("disabled")),
    ).toBe(true);
    expect(
      screen.getByRole("button", { name: "Archive pending recovery" }),
    ).toBeEnabled();
    expect(screen.getByText(/Recovery storage is full/)).toHaveTextContent(
      "sign in again",
    );
    const record = screen.getByRole("group", {
      name: "Recovery record old-one",
    });
    fireEvent.click(
      within(record).getByRole("button", {
        name: "Confirm this recovery record",
      }),
    );
    confirm("Check selected recovery record");
    await waitFor(() =>
      expect(zcodeAccountsApi.confirmRecovery).toHaveBeenCalledWith(
        source,
        "context-revision",
        "old-one",
        "recovery-one",
      ),
    );
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    fireEvent.click(
      within(record).getByRole("button", {
        name: "Recapture after official sign-in",
      }),
    );
    expect(screen.getByRole("dialog")).toHaveTextContent("quit ZCode normally");
    confirm("Save and confirm selected record");
    await waitFor(() =>
      expect(zcodeAccountsApi.recapture).toHaveBeenCalledWith(
        source,
        "context-revision",
        "old-one",
        "recovery-one",
        "catalog-one",
      ),
    );
  });
  it("binds permanent cleanup to the named ID and revision and cancellation dispatches nothing", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      records: [
        {
          id: "selected-record",
          disposition: "full-after",
          latestCompleted: false,
        },
        {
          id: "other-record",
          disposition: "explicit-capture",
          latestCompleted: true,
        },
      ],
    });
    mount();
    await screen.findByText("selected-record");
    const row = screen.getByRole("group", {
      name: "Recovery record selected-record",
    });
    fireEvent.click(
      within(row).getByRole("button", {
        name: "Permanently delete recovery record",
      }),
    );
    expect(screen.getByRole("dialog")).toHaveTextContent("selected-record");
    expect(screen.getByRole("dialog")).toHaveTextContent("cannot be recovered");
    expect(screen.getByRole("dialog")).toHaveTextContent("source and target");
    confirm("Cancel account action");
    expect(zcodeAccountsApi.deleteRecovery).not.toHaveBeenCalled();
    fireEvent.click(
      within(row).getByRole("button", {
        name: "Permanently delete recovery record",
      }),
    );
    confirm("Delete selected record permanently");
    await waitFor(() =>
      expect(zcodeAccountsApi.deleteRecovery).toHaveBeenCalledWith(
        "selected-record",
        "recovery-one",
      ),
    );
  });
  it("keeps a completed local switch visible when the following status refresh fails", async () => {
    mount();
    await inspect();
    vi.mocked(zcodeAccountsApi.status).mockRejectedValueOnce({
      code: "zcode.account.storage_failed",
      remedy: "checkLocalStorage",
      committed: false,
    });
    fireEvent.click(
      within(screen.getByRole("region", { name: "Z.ai accounts" })).getByRole(
        "button",
        { name: "Switch saved account" },
      ),
    );
    confirm("Switch to this account");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "refreshing its status failed",
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      `Switched ${source.dataRoot} locally. Start official ZCode and verify the account there.`,
    );
    expect(screen.getByText("Current account: unknown")).toBeInTheDocument();
    expect(zcodeAccountsApi.switch).toHaveBeenCalledTimes(1);
    expect(zcodeAccountsApi.capture).not.toHaveBeenCalled();
  });
  it("keeps completed permanent cleanup visible when local recovery refresh fails", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      records: [
        {
          id: "selected-record",
          disposition: "full-after",
          latestCompleted: false,
        },
      ],
    });
    mount();
    await screen.findByText("selected-record");
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockRejectedValueOnce({
      code: "zcode.account.storage_failed",
      remedy: "checkLocalStorage",
      committed: false,
    });
    fireEvent.click(
      screen.getByRole("button", {
        name: "Permanently delete recovery record",
      }),
    );
    confirm("Delete selected record permanently");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "refreshing its status failed",
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "Selected recovery record permanently deleted.",
    );
    expect(zcodeAccountsApi.deleteRecovery).toHaveBeenCalledTimes(1);
    expect(zcodeAccountsApi.capture).not.toHaveBeenCalled();
  });
  it("keeps a failed status refresh read-only and exposes only static error text", async () => {
    const client = mount();
    await inspect();
    vi.mocked(zcodeAccountsApi.status).mockRejectedValueOnce({
      code: "zcode.account.context_changed",
      remedy: "refreshContext",
      message: "secret-canary-token",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Refresh account status" }),
    );
    await screen.findByRole("alert");
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeDisabled();
    expect(zcodeAccountsApi.capture).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.switch).not.toHaveBeenCalled();
    expect(
      JSON.stringify(
        client
          .getQueryCache()
          .getAll()
          .map((q) => q.state),
      ),
    ).not.toContain("secret-canary");
  });
  it("rejects a dialog whose catalog revision changed without silently rebasing the selected account", async () => {
    const client = mount();
    await inspect();
    fireEvent.click(
      within(screen.getByRole("region", { name: "Z.ai accounts" })).getByRole(
        "button",
        { name: "Switch saved account" },
      ),
    );
    await act(async () => {
      client.setQueryData(
        [
          "zcodeAccountCatalog",
          source.installPath,
          source.dataRoot,
          source.keyMode,
          context.contextRevision,
        ],
        {
          ...catalog,
          revision: "catalog-new",
          profiles: [
            { ...catalog.profiles[0], label: "New catalog label" },
            catalog.profiles[1],
          ],
        },
      );
    });
    await screen.findByText("New catalog label");
    confirm("Switch to this account");
    await screen.findByRole("alert");
    expect(zcodeAccountsApi.switch).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("blocks a new switch at full capacity even when both older records are confirmed", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockResolvedValue({
      ...recovery,
      records: [
        { id: "old-one", disposition: "full-before", latestCompleted: false },
        { id: "old-two", disposition: "full-after", latestCompleted: false },
      ],
    });
    mount();
    await inspect();
    expect(
      within(screen.getByRole("region", { name: "Z.ai accounts" })).getByRole(
        "button",
        { name: "Switch saved account" },
      ),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Save current account" }),
    ).toBeEnabled();
    expect(
      screen
        .getAllByRole("button", { name: "Permanently delete recovery record" })
        .every((button) => !button.hasAttribute("disabled")),
    ).toBe(true);
  });
  it("does not put backend error payloads into the query cache", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockRejectedValue({
      code: "untrusted-secret-canary",
      remedy: "untrusted-secret-canary",
      message: "secret-canary-token",
      committed: false,
    });
    const client = mount();
    await screen.findByRole("alert");
    expect(document.body.textContent).not.toContain("secret-canary");
    expect(
      JSON.stringify(
        client
          .getQueryCache()
          .getAll()
          .map((q) => q.state),
      ),
    ).not.toContain("secret-canary");
    expect(client.getMutationCache().getAll()).toHaveLength(0);
  });
  it("reports unsupported local storage without promising recovery or reading the native source", async () => {
    vi.mocked(zcodeAccountsApi.recoveryStatus).mockRejectedValue({
      code: "zcode.account.unsupported_platform",
      remedy: "finishPlatformCheck",
      committed: false,
    });
    mount();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "This platform does not support the requested account operation.",
    );
    expect(document.body.textContent).not.toContain(
      "Local recovery can be inspected",
    );
    expect(
      screen.getByRole("button", { name: "Archive pending recovery" }),
    ).toBeDisabled();
    expect(zcodeAccountsApi.inspect).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.status).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.capture).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.archive).not.toHaveBeenCalled();
  });
});

describe("ZCode account command bindings", () => {
  it("sends only the explicit source, opaque IDs and reviewed camelCase revisions", async () => {
    const { zcodeAccountsApi: api } = await vi.importActual<
      typeof import("@/lib/api/zcodeAccounts")
    >("@/lib/api/zcodeAccounts");
    vi.mocked(invoke).mockResolvedValueOnce(context);
    await api.inspect(source);
    vi.mocked(invoke).mockResolvedValueOnce(catalog);
    await api.status(source, "ctx");
    vi.mocked(invoke).mockResolvedValueOnce("saved");
    await api.capture(source, "ctx", "cat");
    vi.mocked(invoke).mockResolvedValueOnce("switched");
    await api.switch(source, "ctx", "opaque-id", "cat");
    vi.mocked(invoke).mockResolvedValueOnce(recovery);
    await api.recoveryStatus();
    vi.mocked(invoke).mockResolvedValueOnce("archived");
    await api.archive("rec");
    await api.confirmRecovery(source, "ctx", "record-id", "rec");
    vi.mocked(invoke).mockResolvedValueOnce("saved");
    await api.recapture(source, "ctx", "record-id", "rec", "cat");
    await api.deleteRecovery("record-id", "rec");
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["inspect_zcode_account_context", { source }],
      ["get_zcode_account_status", { source, contextRevision: "ctx" }],
      [
        "capture_zcode_current_account",
        { source, contextRevision: "ctx", catalogRevision: "cat" },
      ],
      [
        "switch_zcode_saved_account",
        {
          source,
          contextRevision: "ctx",
          id: "opaque-id",
          catalogRevision: "cat",
        },
      ],
      ["get_zcode_account_recovery", undefined],
      ["archive_zcode_account_recovery", { revision: "rec" }],
      [
        "confirm_zcode_account_recovery",
        {
          source,
          contextRevision: "ctx",
          id: "record-id",
          recoveryRevision: "rec",
        },
      ],
      [
        "recapture_zcode_account_recovery",
        {
          source,
          contextRevision: "ctx",
          id: "record-id",
          recoveryRevision: "rec",
          catalogRevision: "cat",
        },
      ],
      ["delete_zcode_account_recovery", { id: "record-id", revision: "rec" }],
    ]);
  });
  it("projects only safe metadata and never returns unknown error fields", async () => {
    const { zcodeAccountsApi: api } = await vi.importActual<
      typeof import("@/lib/api/zcodeAccounts")
    >("@/lib/api/zcodeAccounts");
    vi.mocked(invoke).mockResolvedValueOnce({
      ...catalog,
      secret: "canary-secret",
      profiles: catalog.profiles.map((profile) => ({
        ...profile,
        accessToken: "canary-secret",
      })),
    });
    expect(await api.status(source, "ctx")).toEqual(catalog);
    vi.mocked(invoke).mockRejectedValueOnce({
      code: "zcode.account.catalog_changed",
      remedy: "refreshContext",
      committed: false,
      message: "canary-secret",
      token: "canary-secret",
    });
    await expect(api.capture(source, "ctx", "cat")).rejects.toEqual({
      code: "zcode.account.catalog_changed",
      remedy: "refreshContext",
      committed: false,
    });
  });
});
