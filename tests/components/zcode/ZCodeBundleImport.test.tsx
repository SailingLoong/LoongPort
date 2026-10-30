import {
  act,
  render,
  screen,
  fireEvent,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ZCodeBundleImport } from "@/components/zcode/ZCodeBundleImport";
import {
  zcodeAccountsApi,
  type BundlePreview,
  type BundleCheckProgress,
} from "@/lib/api/zcodeAccounts";
vi.mock("@/lib/api/zcodeAccounts", async (original) => ({
  ...(await original<typeof import("@/lib/api/zcodeAccounts")>()),
  zcodeAccountsApi: {
    previewBundle: vi.fn(),
    checkBundle: vi.fn(),
    bundleCheck: vi.fn(),
    importBundle: vi.fn(),
    pendingBundleImport: vi.fn(),
    acknowledgeBundleImport: vi.fn(),
    bundleImportResult: vi.fn(),
    cancelBundle: vi.fn(),
  },
}));
const root = "/synthetic/data";
const previewResult: BundlePreview = {
  previewId: "lease",
  rows: [
    {
      index: 0,
      id: "opaque",
      label: "a…",
      family: "zai",
      duplicate: true,
      ambiguous: false,
      error: null,
    },
  ],
};
const progress = (
  status: BundleCheckProgress["status"] = "ready",
): BundleCheckProgress => ({
  previewId: "lease",
  selected: [{ index: 0, updateDuplicate: false }],
  status,
  rows: [{ index: 0, capabilities: null, error: null }],
  completed: status === "ready" ? 1 : 0,
  total: 1,
  error: null,
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => {
    resolve = yes;
  });
  return { resolve, promise };
}
function mount() {
  const onClose = vi.fn();
  const onImported = vi.fn(async () => {});
  const view = render(
    <ZCodeBundleImport
      open
      onClose={onClose}
      libraryDataRoot={root}
      catalogRevision="catalog"
      onImported={onImported}
    />,
  );
  return { ...view, onClose, onImported };
}
async function preview(password = "synthetic-password") {
  fireEvent.change(screen.getByLabelText("Account bundle (.zsb)"), {
    target: { files: [new File([new Uint8Array([1, 2, 3])], "synthetic.zsb")] },
  });
  fireEvent.change(screen.getByLabelText("Bundle password"), {
    target: { value: password },
  });
  fireEvent.click(
    screen.getByRole("button", { name: "Preview account bundle" }),
  );
  await screen.findByLabelText("Select account 1");
}
async function verify() {
  fireEvent.click(
    screen.getByRole("checkbox", {
      name: "Allow official checks for the selected accounts",
    }),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Verify selected accounts" }),
  );
  await screen.findByText(
    "Official checks finished. Review each capability before saving.",
  );
}
const save = () =>
  fireEvent.click(
    screen.getByRole("button", {
      name: "Import selected into encrypted vault",
    }),
  );
beforeEach(() => {
  vi.mocked(zcodeAccountsApi.acknowledgeBundleImport).mockReset();
  vi.mocked(zcodeAccountsApi.pendingBundleImport)
    .mockReset()
    .mockReturnValue(null);
  vi.mocked(zcodeAccountsApi.bundleImportResult)
    .mockReset()
    .mockResolvedValue(null);
  vi.mocked(zcodeAccountsApi.previewBundle)
    .mockReset()
    .mockResolvedValue(previewResult);
  vi.mocked(zcodeAccountsApi.checkBundle)
    .mockReset()
    .mockImplementation(async (_previewId, selected) => ({
      ...progress(),
      selected,
    }));
  vi.mocked(zcodeAccountsApi.bundleCheck)
    .mockReset()
    .mockResolvedValue(progress());
  vi.mocked(zcodeAccountsApi.importBundle)
    .mockReset()
    .mockResolvedValue(["kept"]);
  vi.mocked(zcodeAccountsApi.cancelBundle)
    .mockReset()
    .mockResolvedValue(undefined);
});
describe("ZCode bundle import", () => {
  it("recovers the retained request if another window claims the pointer before dispatch", async () => {
    mount();
    await preview();
    await verify();
    vi.mocked(zcodeAccountsApi.pendingBundleImport).mockReturnValue(
      "other-window-request",
    );
    vi.mocked(zcodeAccountsApi.importBundle).mockRejectedValue({
      code: "zcode.account.save_result_unknown",
      remedy: "queryOriginal",
      committed: false,
    });
    save();
    await screen.findByRole("button", { name: "Query original result" });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Query original result" }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.bundleImportResult).toHaveBeenCalledWith(
        root,
        "other-window-request",
      ),
    );
    expect(zcodeAccountsApi.importBundle).toHaveBeenCalledTimes(1);
    expect(zcodeAccountsApi.acknowledgeBundleImport).not.toHaveBeenCalled();
  });
  it("permits a fresh preview only after authoritative notRecorded, without replaying the old request", async () => {
    vi.mocked(zcodeAccountsApi.pendingBundleImport).mockReturnValue(
      "older-request",
    );
    vi.mocked(zcodeAccountsApi.bundleImportResult).mockResolvedValue({
      requestId: "older-request",
      status: "notRecorded",
      rows: [],
    });
    const { onImported } = mount();
    expect(screen.getByLabelText("Account bundle (.zsb)")).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    await screen.findByText(/This authenticated library has no saved result/);
    await waitFor(() =>
      expect(screen.getByLabelText("Account bundle (.zsb)")).toBeEnabled(),
    );
    expect(zcodeAccountsApi.acknowledgeBundleImport).toHaveBeenCalledWith(
      root,
      "older-request",
    );
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
    expect(onImported).toHaveBeenCalledOnce();
    await preview();
    expect(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    ).toBeDisabled();
    expect(zcodeAccountsApi.checkBundle).not.toHaveBeenCalled();
  });

  it("keeps closed late import success recoverable on reopening", async () => {
    const pending = deferred<("saved" | "refreshed" | "kept")[]>();
    vi.mocked(zcodeAccountsApi.importBundle).mockReturnValue(pending.promise);
    const first = mount();
    await preview();
    await verify();
    save();
    fireEvent.click(
      screen.getByRole("button", { name: "Close bundle import" }),
    );
    await act(async () => pending.resolve(["kept"]));
    expect(first.onImported).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.acknowledgeBundleImport).not.toHaveBeenCalled();
    first.unmount();
    vi.mocked(zcodeAccountsApi.pendingBundleImport).mockReturnValue("lease");
    mount();
    expect(
      screen.getByRole("button", { name: "Query original result" }),
    ).toBeEnabled();
    expect(zcodeAccountsApi.importBundle).toHaveBeenCalledTimes(1);
  });
  it("keeps unknown recovery and blocks a new preview from replacing it", async () => {
    vi.mocked(zcodeAccountsApi.pendingBundleImport).mockReturnValue(
      "older-request",
    );
    mount();
    expect(screen.getByLabelText("Account bundle (.zsb)")).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    await screen.findByText(/No original receipt is available/);
    expect(zcodeAccountsApi.acknowledgeBundleImport).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Account bundle (.zsb)")).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Query original result" }),
    ).toBeEnabled();
  });
  it("queries the original receipt after a lost reply without another import", async () => {
    vi.mocked(zcodeAccountsApi.importBundle).mockRejectedValue(
      new Error("lost reply"),
    );
    vi.mocked(zcodeAccountsApi.bundleImportResult).mockResolvedValue({
      requestId: "lease",
      status: "completed",
      rows: [{ index: 0, id: "a".repeat(64), family: "zai", outcome: "kept" }],
    });
    const { onImported } = mount();
    await preview();
    await verify();
    save();
    await screen.findByText(/Unconfirmed/);
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    expect(
      await screen.findByRole("list", {
        name: "Last import results by account",
      }),
    ).toHaveTextContent("Kept existing");
    await waitFor(() => expect(onImported).toHaveBeenCalledOnce());
    expect(zcodeAccountsApi.bundleImportResult).toHaveBeenCalledWith(
      root,
      "lease",
    );
    expect(zcodeAccountsApi.importBundle).toHaveBeenCalledTimes(1);
    expect(zcodeAccountsApi.checkBundle).toHaveBeenCalledTimes(1);
  });
  it("recovers on a fresh dialog from its saved pointer without preview or checks", async () => {
    vi.mocked(zcodeAccountsApi.pendingBundleImport).mockReturnValue(
      "prior-request",
    );
    vi.mocked(zcodeAccountsApi.bundleImportResult).mockResolvedValue({
      requestId: "prior-request",
      status: "completed",
      rows: [
        { index: 3, id: "b".repeat(64), family: "bigmodel", outcome: "saved" },
      ],
    });
    const { onImported } = mount();
    expect(zcodeAccountsApi.bundleImportResult).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    await screen.findByText(/Import completed: 1 saved/);
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.previewBundle).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.checkBundle).not.toHaveBeenCalled();
    expect(onImported).toHaveBeenCalledOnce();
  });
  it("keeps absent receipts unknown and never replays the import", async () => {
    vi.mocked(zcodeAccountsApi.pendingBundleImport).mockReturnValue(
      "prior-request",
    );
    const { onImported } = mount();
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    await screen.findByText(/No original receipt is available/);
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
    expect(onImported).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Query original result" }),
    ).toBeEnabled();
  });
  it("ignores a late receipt after closing and does not auto-submit on reopening", async () => {
    const pending = deferred<
      import("@/lib/api/zcodeAccounts").BundleImportResult | null
    >();
    vi.mocked(zcodeAccountsApi.pendingBundleImport).mockReturnValue(
      "prior-request",
    );
    vi.mocked(zcodeAccountsApi.bundleImportResult).mockReturnValue(
      pending.promise,
    );
    const { onImported } = mount();
    fireEvent.click(
      screen.getByRole("button", { name: "Query original result" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Close bundle import" }),
    );
    await act(async () =>
      pending.resolve({
        requestId: "prior-request",
        status: "completed",
        rows: [
          { index: 0, id: "a".repeat(64), family: "zai", outcome: "saved" },
        ],
      }),
    );
    expect(onImported).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.acknowledgeBundleImport).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("polls only the owned verification and ignores its late result after cancellation", async () => {
    const pending = deferred<BundleCheckProgress>();
    vi.mocked(zcodeAccountsApi.checkBundle).mockResolvedValue(
      progress("checking"),
    );
    vi.mocked(zcodeAccountsApi.bundleCheck).mockReturnValue(pending.promise);
    const { unmount } = mount();
    await preview();
    vi.useFakeTimers();
    try {
      await act(async () => {
        fireEvent.click(
          screen.getByRole("checkbox", {
            name: "Allow official checks for the selected accounts",
          }),
        );
        fireEvent.click(
          screen.getByRole("button", { name: "Verify selected accounts" }),
        );
      });
      await act(async () => vi.advanceTimersByTimeAsync(1000));
      expect(zcodeAccountsApi.bundleCheck).toHaveBeenCalledExactlyOnceWith(
        "lease",
      );
      fireEvent.click(
        screen.getByRole("button", { name: "Close bundle import" }),
      );
      await act(async () => pending.resolve(progress()));
      expect(
        screen.queryByText(
          "Official checks finished. Review each capability before saving.",
        ),
      ).not.toBeInTheDocument();
      expect(zcodeAccountsApi.cancelBundle).toHaveBeenCalledExactlyOnceWith(
        "lease",
      );
    } finally {
      unmount();
      vi.useRealTimers();
    }
  });
  it("previews locally, preserves password bytes, clears the password and defaults duplicate updates off", async () => {
    mount();
    await preview("  exact password  ");
    expect(zcodeAccountsApi.previewBundle).toHaveBeenCalledExactlyOnceWith(
      root,
      "catalog",
      [1, 2, 3],
      "  exact password  ",
    );
    expect(screen.getByLabelText("Bundle password")).toHaveValue("");
    expect(zcodeAccountsApi.checkBundle).not.toHaveBeenCalled();
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
    expect(
      screen.getByLabelText("Update existing account 1"),
    ).not.toBeChecked();
    expect(
      screen.getByRole("button", { name: "Verify selected accounts" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    ).toBeDisabled();
  });
  it("requires explicit official-check consent and does not save until a separate action", async () => {
    mount();
    await preview();
    await verify();
    expect(zcodeAccountsApi.checkBundle).toHaveBeenCalledExactlyOnceWith(
      "lease",
      [{ index: 0, updateDuplicate: false }],
      true,
    );
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
    save();
    await waitFor(() =>
      expect(zcodeAccountsApi.importBundle).toHaveBeenCalledExactlyOnceWith(
        root,
        "catalog",
        "lease",
        [{ index: 0, updateDuplicate: false }],
        undefined,
      ),
    );
  });
  it("shows a masked outcome for every selected account", async () => {
    vi.mocked(zcodeAccountsApi.previewBundle).mockResolvedValue({
      previewId: "lease",
      rows: ["a…", "b…", "c…"].map((label, index) => ({
        index,
        id: `opaque-${index}`,
        label,
        family: "zai",
        duplicate: index > 0,
        ambiguous: false,
        error: null,
      })),
    });
    vi.mocked(zcodeAccountsApi.checkBundle).mockResolvedValue({
      ...progress(),
      rows: [0, 1, 2].map((index) => ({
        index,
        capabilities: null,
        error: null,
      })),
      total: 3,
      completed: 3,
      selected: [
        { index: 0, updateDuplicate: false },
        { index: 1, updateDuplicate: true },
        { index: 2, updateDuplicate: false },
      ],
    });
    vi.mocked(zcodeAccountsApi.importBundle).mockResolvedValue([
      "saved",
      "refreshed",
      "kept",
    ]);
    mount();
    await preview();
    fireEvent.click(screen.getByLabelText("Update existing account 2"));
    await verify();
    save();
    const rows = within(
      await screen.findByRole("list", {
        name: "Last import results by account",
      }),
    ).getAllByRole("listitem");
    expect(rows).toHaveLength(3);
    expect(rows[0]).toHaveTextContent("a…");
    expect(rows[0]).toHaveTextContent("Saved");
    expect(rows[1]).toHaveTextContent("b…");
    expect(rows[1]).toHaveTextContent("Updated");
    expect(rows[2]).toHaveTextContent("Kept existing");
  });
  it("keeps a lost import response unconfirmed and never offers a second write", async () => {
    vi.mocked(zcodeAccountsApi.importBundle).mockRejectedValue(
      new Error("lost private response"),
    );
    mount();
    await preview();
    await verify();
    save();
    const result = await screen.findByRole("list", {
      name: "Last import results by account",
    });
    expect(within(result).getByRole("listitem")).toHaveTextContent(
      "Unconfirmed — inspect saved accounts",
    );
    expect(screen.queryByText(/lost private/)).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    ).not.toBeInTheDocument();
    expect(zcodeAccountsApi.importBundle).toHaveBeenCalledTimes(1);
  });
  it("keeps an original-result recovery action reachable after a lost import reply", async () => {
    vi.mocked(zcodeAccountsApi.importBundle).mockRejectedValue(
      new Error("lost synthetic response"),
    );
    const { onImported } = mount();
    await preview();
    await verify();
    save();
    expect(
      await screen.findByRole("list", {
        name: "Last import results by account",
      }),
    ).toHaveTextContent("Unconfirmed");
    expect(zcodeAccountsApi.importBundle).toHaveBeenCalledTimes(1);
    expect(onImported).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Query original result" }),
    ).toBeEnabled();
  });
  it("preserves confirmed import when refreshing the list fails or its catalog revision changes", async () => {
    const { onImported, onClose, rerender } = mount();
    onImported.mockRejectedValue(new Error("private refresh"));
    await preview();
    await verify();
    save();
    await screen.findByText(
      /Import completed, but the saved account list could not be refreshed/,
    );
    rerender(
      <ZCodeBundleImport
        open
        onClose={onClose}
        libraryDataRoot={root}
        catalogRevision="catalog-new"
        onImported={onImported}
      />,
    );
    expect(
      screen.getByRole("list", { name: "Last import results by account" }),
    ).toHaveTextContent("Kept existing");
    expect(onImported).toHaveBeenCalledTimes(1);
    expect(zcodeAccountsApi.importBundle).toHaveBeenCalledTimes(1);
  });
  it("requires a fresh check if the selected record or duplicate choice changes", async () => {
    mount();
    await preview();
    await verify();
    fireEvent.click(screen.getByLabelText("Update existing account 1"));
    expect(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    ).toBeDisabled();
    expect(
      screen.getByRole("checkbox", {
        name: "Allow official checks for the selected accounts",
      }),
    ).not.toBeChecked();
    await verify();
    save();
    await waitFor(() =>
      expect(zcodeAccountsApi.importBundle).toHaveBeenCalledWith(
        root,
        "catalog",
        "lease",
        [{ index: 0, updateDuplicate: true }],
        undefined,
      ),
    );
  });
  it("rejects an older ready result after a changed-choice check was not delivered", async () => {
    mount();
    await preview();
    await verify();
    fireEvent.click(screen.getByLabelText("Update existing account 1"));
    vi.mocked(zcodeAccountsApi.checkBundle).mockRejectedValueOnce(
      new Error("lost new check"),
    );
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Allow official checks for the selected accounts",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Verify selected accounts" }),
    );
    await screen.findByRole("alert");
    // The server still knows only the previous choice (updateDuplicate: false).
    vi.mocked(zcodeAccountsApi.bundleCheck).mockResolvedValue({
      ...progress(),
      selected: [{ index: 0, updateDuplicate: false }],
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Query verification result" }),
    );
    await screen.findByText(
      "This result belongs to an earlier selection. Authorize checks for the current selection again.",
    );
    expect(
      screen.queryByText(
        "Official checks finished. Review each capability before saving.",
      ),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    ).toBeDisabled();
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
  });
  it("requires an explicit choice for an ambiguous identity and checks exactly the chosen record", async () => {
    vi.mocked(zcodeAccountsApi.previewBundle).mockResolvedValue({
      previewId: "lease",
      rows: [0, 1].map((index) => ({
        ...previewResult.rows[0],
        index,
        duplicate: false,
        ambiguous: true,
      })),
    });
    vi.mocked(zcodeAccountsApi.checkBundle).mockResolvedValue({
      ...progress(),
      rows: [{ index: 1, capabilities: null, error: null }],
      selected: [{ index: 1, updateDuplicate: false }],
    });
    mount();
    await preview();
    expect(screen.getByLabelText("Select account 1")).not.toBeChecked();
    expect(screen.getByLabelText("Select account 2")).not.toBeChecked();
    fireEvent.click(screen.getByLabelText("Select account 2"));
    await verify();
    save();
    await waitFor(() =>
      expect(zcodeAccountsApi.importBundle).toHaveBeenCalledWith(
        root,
        "catalog",
        "lease",
        [{ index: 1, updateDuplicate: false }],
        undefined,
      ),
    );
  });
  it("recovers a lost check response by querying its original preview without repeating checks", async () => {
    vi.mocked(zcodeAccountsApi.checkBundle).mockRejectedValue(
      new Error("private check response"),
    );
    mount();
    await preview();
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Allow official checks for the selected accounts",
      }),
    );
    const start = screen.getByRole("button", {
      name: "Verify selected accounts",
    });
    fireEvent.click(start);
    fireEvent.click(start);
    await screen.findByRole("alert");
    expect(start).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Query verification result" }),
    );
    await screen.findByText(
      "Official checks finished. Review each capability before saving.",
    );
    expect(zcodeAccountsApi.checkBundle).toHaveBeenCalledTimes(1);
    expect(zcodeAccountsApi.bundleCheck).toHaveBeenCalledExactlyOnceWith(
      "lease",
    );
    expect(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    ).toBeEnabled();
  });
  it("allows saving a completed check with an unknown per-account result without calling it invalid", async () => {
    vi.mocked(zcodeAccountsApi.checkBundle).mockResolvedValue({
      ...progress(),
      rows: [
        {
          index: 0,
          capabilities: null,
          error: {
            code: "zcode.account.official_unavailable",
            remedy: "queryOriginal",
            committed: false,
          },
        },
      ],
    });
    mount();
    await preview();
    await verify();
    expect(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    ).toBeEnabled();
    expect(
      screen.queryByText(/require official local sign-in verification/),
    ).not.toBeInTheDocument();
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
  });
  it.each(["close", "unmount", "source", "catalog"])(
    "cancels the exact preview on %s and ignores a late check result",
    async (action) => {
      const pending = deferred<BundleCheckProgress>();
      vi.mocked(zcodeAccountsApi.checkBundle).mockReturnValue(pending.promise);
      const { unmount, rerender, onClose, onImported } = mount();
      await preview();
      fireEvent.click(
        screen.getByRole("checkbox", {
          name: "Allow official checks for the selected accounts",
        }),
      );
      fireEvent.click(
        screen.getByRole("button", { name: "Verify selected accounts" }),
      );
      if (action === "close")
        fireEvent.click(
          screen.getByRole("button", { name: "Close bundle import" }),
        );
      if (action === "unmount") unmount();
      if (action === "source")
        rerender(
          <ZCodeBundleImport
            open
            onClose={onClose}
            libraryDataRoot="/different"
            catalogRevision="catalog"
            onImported={onImported}
          />,
        );
      if (action === "catalog")
        rerender(
          <ZCodeBundleImport
            open
            onClose={onClose}
            libraryDataRoot={root}
            catalogRevision="new-catalog"
            onImported={onImported}
          />,
        );
      await act(async () => pending.resolve(progress()));
      expect(zcodeAccountsApi.cancelBundle).toHaveBeenCalledExactlyOnceWith(
        "lease",
      );
      expect(
        screen.queryByText(
          "Official checks finished. Review each capability before saving.",
        ),
      ).not.toBeInTheDocument();
      expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
    },
  );
  it("cancels a late preview response after dismissal without sending credentials for checks", async () => {
    const pending = deferred<BundlePreview>();
    vi.mocked(zcodeAccountsApi.previewBundle).mockReturnValue(pending.promise);
    mount();
    fireEvent.change(screen.getByLabelText("Account bundle (.zsb)"), {
      target: { files: [new File(["encrypted"], "synthetic.zsb")] },
    });
    fireEvent.change(screen.getByLabelText("Bundle password"), {
      target: { value: "password" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Preview account bundle" }),
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.previewBundle).toHaveBeenCalledTimes(1),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Close bundle import" }),
    );
    await act(async () => pending.resolve(previewResult));
    expect(zcodeAccountsApi.cancelBundle).toHaveBeenCalledExactlyOnceWith(
      "lease",
    );
    expect(zcodeAccountsApi.checkBundle).not.toHaveBeenCalled();
  });
});

it("headless recovery acknowledges only an authoritative original result and never replays import", async () => {
  const { renderHook } = await import("@testing-library/react");
  const { useZCodeBundleImport } =
    await import("@/components/zcode/viewmodels/useZCodeBundleImport");
  vi.mocked(zcodeAccountsApi.pendingBundleImport).mockReturnValue(
    "older-request",
  );
  const { result: vm } = renderHook(() =>
    useZCodeBundleImport({
      libraryDataRoot: root,
      catalogRevision: "catalog",
      onClose: vi.fn(),
      onImported: vi.fn(async () => {}),
    }),
  );
  await act(() => vm.current.queryOriginal());
  expect(vm.current.recoveryUnknown).toBe(true);
  expect(vm.current.originalRequest).toBe("older-request");
  expect(zcodeAccountsApi.acknowledgeBundleImport).not.toHaveBeenCalled();
  vi.mocked(zcodeAccountsApi.bundleImportResult).mockResolvedValue({
    requestId: "older-request",
    status: "notRecorded",
    rows: [],
  });
  await act(() => vm.current.queryOriginal());
  expect(vm.current.notRecorded).toBe(true);
  expect(vm.current.originalRequest).toBeNull();
  expect(zcodeAccountsApi.acknowledgeBundleImport).toHaveBeenCalledWith(
    root,
    "older-request",
  );
  expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
});

it("headless bundle projects authoritative receipt counts without treating unknown recovery as completion", async () => {
  const { renderHook } = await import("@testing-library/react");
  const { useZCodeBundleImport } =
    await import("@/components/zcode/viewmodels/useZCodeBundleImport");
  vi.mocked(zcodeAccountsApi.pendingBundleImport).mockReturnValue("original");
  const { result } = renderHook(() =>
    useZCodeBundleImport({
      libraryDataRoot: root,
      catalogRevision: "catalog",
      onClose: vi.fn(),
      onImported: vi.fn(async () => {}),
    }),
  );
  expect(result.current.resultSummary).toBeNull();
  await act(() => result.current.queryOriginal());
  expect(result.current.resultSummary).toBeNull();
  expect(result.current.originalRequest).toBe("original");
  vi.mocked(zcodeAccountsApi.bundleImportResult).mockResolvedValue({
    requestId: "original",
    status: "completed",
    rows: [
      { index: 0, id: "a".repeat(64), family: "zai", outcome: "saved" },
      {
        index: 1,
        id: "b".repeat(64),
        family: "bigmodel",
        outcome: "refreshed",
      },
      { index: 2, id: "c".repeat(64), family: "zai", outcome: "kept" },
    ],
  });
  await act(() => result.current.queryOriginal());
  expect(result.current.resultSummary).toEqual({
    saved: 1,
    updated: 1,
    kept: 1,
  });
  expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
});

it("headless bundle binds row evidence and eligibility and suppresses a lost-save completion summary", async () => {
  const { renderHook } = await import("@testing-library/react");
  const { useZCodeBundleImport } =
    await import("@/components/zcode/viewmodels/useZCodeBundleImport");
  const { result } = renderHook(() =>
    useZCodeBundleImport({
      libraryDataRoot: root,
      catalogRevision: "catalog",
      onClose: vi.fn(),
      onImported: vi.fn(async () => {}),
    }),
  );
  act(() => {
    result.current.chooseBundle(new File(["encrypted"], "synthetic.zsb"));
    result.current.setPassword("synthetic-password");
  });
  await act(() => result.current.preview());
  expect(result.current.reviewRows[0]).toMatchObject({
    row: previewResult.rows[0],
    selected: true,
    updateDuplicate: false,
    selectionDisabled: false,
    updateDisabled: false,
  });
  act(() => result.current.setConsent(true));
  await act(() => result.current.check());
  expect(result.current.reviewRows[0].checked).toEqual(progress().rows[0]);
  vi.mocked(zcodeAccountsApi.importBundle).mockRejectedValue(
    new Error("lost reply"),
  );
  await act(() => result.current.commit());
  expect(result.current.resultRows[0].outcome).toBe("unknown");
  expect(result.current.resultSummary).toBeNull();
  expect(result.current.reviewRows[0].selectionDisabled).toBe(true);
  expect(result.current.originalRequest).toBe("lease");
  expect(zcodeAccountsApi.acknowledgeBundleImport).not.toHaveBeenCalled();
  expect(zcodeAccountsApi.importBundle).toHaveBeenCalledTimes(1);
});
