import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "i18next";
import en from "@/i18n/locales/en.json";
import { feedbackSubmission } from "@/components/settings/feedbackSubmission";
import { FeedbackSubmissionIndicator } from "@/components/settings/FeedbackSubmissionStatus";
import { APP_IDS, getAppDisplayName } from "@/config/appConfig";

const mocks = vi.hoisted(() => ({
  submit: vi.fn(),
  prepare: vi.fn(),
  cancelPreview: vi.fn(),
  getUsageSummaryByApp: vi.fn(),
  getFormCapabilities: vi.fn(),
  getLastReport: vi.fn(),
  exportLastDiagnostics: vi.fn(),
  deleteLocalReport: vi.fn(),
  readClipboardImage: vi.fn(),
  copyText: vi.fn(),
}));

vi.mock("@/lib/api", () => ({ feedbackApi: mocks }));
vi.mock("@/lib/clipboard", () => ({ copyText: mocks.copyText }));
vi.mock("@/lib/api/usage", () => ({ usageApi: mocks }));

import { FeedbackDialog } from "@/components/settings/FeedbackDialog";
import { useFeedbackForm } from "@/components/settings/useFeedbackForm";

const receipt = {
  reportId: "report-synthetic-0001",
  identity: { kind: "report", id: "report-synthetic-0001" },
  uploadedAt: "2026-10-18T00:00:00Z",
  summary: {
    descriptionIncluded: true,
    contactIncluded: false,
    screenshotCount: 2,
    screenshotBytes: 8192,
    diagnosticsIncluded: true,
    diagnosticsBytes: 1234,
  },
  localCopy: {
    status: "available",
    fileName: "diagnostics.zip",
    bytes: 1234,
  },
};

const prepared = {
  previewId: "synthetic-preview-1",
  expiresInSeconds: 600,
  files: [
    {
      name: "synthetic.png",
      kind: "screenshot",
      bytes: 4,
      sha256: "a".repeat(64),
    },
    {
      name: "diagnostics.zip",
      kind: "diagnostics",
      bytes: 1234,
      sha256: "b".repeat(64),
    },
  ],
  totalBytes: 1238,
  summary: receipt.summary,
  applicationConfigs: {
    codex: { mode: "safe_summary", value: "<img src=x onerror=synthetic>" },
  },
};

beforeEach(() => {
  Object.values(mocks).forEach((mock) => mock.mockReset());
  mocks.getLastReport.mockResolvedValue(null);
  mocks.prepare.mockResolvedValue(prepared);
  mocks.cancelPreview.mockResolvedValue(undefined);
  mocks.getUsageSummaryByApp.mockResolvedValue([]);
  mocks.getFormCapabilities.mockResolvedValue({ extendedFields: true });
  mocks.submit.mockResolvedValue({
    status: "accepted",
    localSave: "stored",
    receipt,
  });
});

describe("FeedbackDialog last successful report", () => {
  it("keeps the dialog open to show the confirmed upload receipt", async () => {
    const onClose = vi.fn();
    render(<FeedbackDialog open onClose={onClose} />);
    await submitDescription("Synthetic feedback only");

    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
    expect(await screen.findByText(receipt.reportId)).toBeVisible();
    expect(screen.getByText("diagnostics.zip")).toBeVisible();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("loads the last successful report when the dialog is reopened", async () => {
    mocks.getLastReport.mockResolvedValue(receipt);
    const { rerender } = render(
      <FeedbackDialog open={false} onClose={vi.fn()} />,
    );
    expect(mocks.getLastReport).not.toHaveBeenCalled();
    rerender(<FeedbackDialog open onClose={vi.fn()} />);

    expect(await screen.findByText(receipt.reportId)).toBeVisible();
    expect(
      screen.getByRole("button", { name: "settings.feedbackDeleteLocalCopy" }),
    ).toBeEnabled();
    expect(
      screen.getByText("settings.feedbackDeleteLocalOnlyHint"),
    ).toBeVisible();
  });
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

async function renderWithReceipt() {
  mocks.getLastReport.mockResolvedValue(receipt);
  const view = render(<FeedbackDialog open onClose={vi.fn()} />);
  await screen.findByText(receipt.reportId);
  return view;
}

async function submitDescription(description = "Another synthetic feedback") {
  fireEvent.change(
    screen.getByRole("textbox", { name: "settings.feedbackDescriptionLabel" }),
    { target: { value: description } },
  );
  const calls = mocks.prepare.mock.calls.length;
  fireEvent.click(previewButton());
  // Preflight validation deliberately leaves the draft without a native preview.
  if (mocks.prepare.mock.calls.length === calls) return;
  await screen.findByRole("region", { name: "settings.feedbackPreviewTitle" });
  fireEvent.click(
    screen.getByRole("button", { name: "settings.feedbackConfirmSubmit" }),
  );
}

describe("FeedbackDialog receipt safety", () => {
  it("preserves the previous receipt and draft after an upload failure", async () => {
    mocks.submit.mockRejectedValue(new Error("synthetic connection failure"));
    await renderWithReceipt();
    await submitDescription();

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "settings.feedbackSubmit" }),
      ).toBeEnabled(),
    );
    expect(screen.getByText(receipt.reportId)).toBeVisible();
    expect(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
    ).toHaveValue("Another synthetic feedback");
  });

  it("preserves the previous receipt when the upload result is unknown", async () => {
    mocks.submit.mockResolvedValue({ status: "unknown" });
    await renderWithReceipt();
    await submitDescription();

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "settings.feedbackUploadUnknown",
    );
    expect(screen.getByText(receipt.reportId)).toBeVisible();
    expect(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
    ).toHaveValue("Another synthetic feedback");
  });

  it("does not submit twice on repeated clicks", async () => {
    const upload = deferred<unknown>();
    mocks.submit.mockReturnValue(upload.promise);
    await renderWithReceipt();
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      {
        target: { value: "Synthetic description" },
      },
    );
    fireEvent.click(previewButton());
    await screen.findByRole("region", {
      name: "settings.feedbackPreviewTitle",
    });
    const submit = screen.getByRole("button", {
      name: "settings.feedbackConfirmSubmit",
    });
    fireEvent.click(submit);
    fireEvent.click(submit);

    expect(mocks.submit).toHaveBeenCalledOnce();
    expect(submit).toBeDisabled();
    await act(async () => upload.resolve({ status: "unknown" }));
  });

  it("does not let an older load replace a newly confirmed report", async () => {
    const initialLoad = deferred<unknown>();
    mocks.getLastReport.mockReturnValue(initialLoad.promise);
    const nextReceipt = {
      ...receipt,
      reportId: "report-synthetic-0002",
      identity: { kind: "report", id: "report-synthetic-0002" },
    };
    mocks.submit.mockResolvedValue({
      status: "accepted",
      localSave: "stored",
      receipt: nextReceipt,
    });
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await submitDescription();
    expect(await screen.findByText(nextReceipt.reportId)).toBeVisible();

    await act(async () => initialLoad.resolve(receipt));
    expect(screen.getByText(nextReceipt.reportId)).toBeVisible();
    expect(screen.queryByText(receipt.reportId)).not.toBeInTheDocument();
  });

  it("ignores a late receipt load from a closed dialog session", async () => {
    const oldLoad = deferred<unknown>();
    mocks.getLastReport
      .mockReturnValueOnce(oldLoad.promise)
      .mockResolvedValue(receipt);
    const { rerender } = render(<FeedbackDialog open onClose={vi.fn()} />);
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    expect(await screen.findByText(receipt.reportId)).toBeVisible();

    await act(async () =>
      oldLoad.resolve({
        ...receipt,
        reportId: "stale-report",
        identity: { kind: "report", id: "stale-report" },
      }),
    );
    expect(screen.queryByText("stale-report")).not.toBeInTheDocument();
    expect(screen.getByText(receipt.reportId)).toBeVisible();
  });

  it("does not reset a reopened draft when an earlier upload returns late", async () => {
    const upload = deferred<unknown>();
    mocks.submit.mockReturnValue(upload.promise);
    const { rerender } = await renderWithReceipt();
    await submitDescription("First synthetic draft");
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      {
        target: { value: "Reopened synthetic draft" },
      },
    );

    await act(async () =>
      upload.resolve({
        status: "accepted",
        localSave: "stored",
        receipt: {
          ...receipt,
          reportId: "late-report",
          identity: { kind: "report", id: "late-report" },
        },
      }),
    );
    expect(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
    ).toHaveValue("Reopened synthetic draft");
    expect(screen.queryByText("late-report")).not.toBeInTheDocument();
    expect(screen.getByText(receipt.reportId)).toBeVisible();
  });

  it("preserves the receipt after cancel and reloads it on reopening", async () => {
    const onClose = vi.fn();
    mocks.getLastReport.mockResolvedValue(receipt);
    const { rerender } = render(<FeedbackDialog open onClose={onClose} />);
    expect(await screen.findByText(receipt.reportId)).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    expect(onClose).toHaveBeenCalledOnce();
    rerender(<FeedbackDialog open={false} onClose={onClose} />);
    rerender(<FeedbackDialog open onClose={onClose} />);
    expect(await screen.findByText(receipt.reportId)).toBeVisible();
    expect(mocks.getLastReport).toHaveBeenCalledTimes(2);
  });

  it("keeps the local receipt when deletion fails", async () => {
    mocks.deleteLocalReport.mockRejectedValue(
      new Error("synthetic local storage failure"),
    );
    await renderWithReceipt();
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackDeleteLocalCopy" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "settings.feedbackDeleteLocalFailed",
    );
    expect(screen.getByText(receipt.reportId)).toBeVisible();
    expect(
      screen.getByRole("button", { name: "settings.feedbackDeleteLocalCopy" }),
    ).toBeEnabled();
  });

  it("removes the card only after the backend deletes its local record and copy", async () => {
    const deletion = deferred<unknown>();
    mocks.deleteLocalReport.mockReturnValue(deletion.promise);
    await renderWithReceipt();
    const button = screen.getByRole("button", {
      name: "settings.feedbackDeleteLocalCopy",
    });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(mocks.deleteLocalReport).toHaveBeenCalledOnce();
    expect(mocks.deleteLocalReport).toHaveBeenCalledWith(receipt.reportId);
    expect(screen.getByText(receipt.reportId)).toBeVisible();

    await act(async () => deletion.resolve(null));
    expect(screen.queryByText(receipt.reportId)).not.toBeInTheDocument();
    expect(mocks.submit).not.toHaveBeenCalled();
  });

  it.each(["missing", "notAttached"])(
    "shows the report ID but disables package actions when local status is %s",
    async (status) => {
      mocks.getLastReport.mockResolvedValue({
        ...receipt,
        localCopy: { status },
      });
      render(<FeedbackDialog open onClose={vi.fn()} />);
      expect(await screen.findByText(receipt.reportId)).toBeVisible();
      expect(
        screen.getByRole("button", {
          name: "settings.feedbackExportLocalCopy",
        }),
      ).toBeDisabled();
      expect(
        screen.getByRole("button", {
          name: "settings.feedbackDeleteLocalCopy",
        }),
      ).toBeEnabled();
    },
  );

  it("leaves the card intact when the user cancels export", async () => {
    mocks.exportLastDiagnostics.mockResolvedValue(false);
    await renderWithReceipt();
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackExportLocalCopy" }),
    );
    await waitFor(() =>
      expect(mocks.exportLastDiagnostics).toHaveBeenCalledWith(
        receipt.reportId,
      ),
    );
    expect(screen.getByText(receipt.reportId)).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("omits site details when diagnostic consent is off", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    fireEvent.click(
      screen.getByRole("checkbox", { name: /settings.feedbackIncludeSites/ }),
    );
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: /settings.feedbackIncludeDiagnostics/,
      }),
    );
    await submitDescription();
    await waitFor(() =>
      expect(mocks.prepare).toHaveBeenCalledWith(
        expect.objectContaining({
          includeDiagnostics: false,
          includeSites: false,
        }),
      ),
    );
  });
});

describe("FeedbackDialog accepted upload compatibility", () => {
  it.each(["failed", "unavailable"])(
    "does not replace the previous card when local save is %s",
    async (localSave) => {
      mocks.submit.mockResolvedValue({
        status: "accepted",
        receipt: null,
        localSave,
      });
      await renderWithReceipt();
      await submitDescription();
      expect(await screen.findByRole("alert")).toHaveTextContent(
        localSave === "failed"
          ? "settings.feedbackLocalSaveFailed"
          : "settings.feedbackReceiptUnavailable",
      );
      expect(screen.getByText(receipt.reportId)).toBeVisible();
      expect(
        screen.getByRole("textbox", {
          name: "settings.feedbackDescriptionLabel",
        }),
      ).toHaveValue("");
    },
  );

  it("shows safety counters only when the user opens the summary", async () => {
    await renderWithReceipt();
    expect(
      screen.queryByText("settings.feedbackSummaryScreenshots"),
    ).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackViewSummary" }),
    );
    expect(
      screen.getByText("settings.feedbackSummaryScreenshots"),
    ).toBeVisible();
    expect(
      screen.getByText("settings.feedbackSummaryDiagnostics"),
    ).toBeVisible();
  });
});

describe("FeedbackDialog attachment lifecycle", () => {
  it("does not append a file read from an earlier dialog session", async () => {
    let reader: FileReader | undefined;
    const read = vi
      .spyOn(FileReader.prototype, "readAsDataURL")
      .mockImplementation(function (this: FileReader) {
        reader = this;
      });
    const { rerender } = render(<FeedbackDialog open onClose={vi.fn()} />);
    const input =
      document.querySelector<HTMLInputElement>('input[type="file"]')!;
    fireEvent.change(input, {
      target: {
        files: [
          new File(["synthetic"], "synthetic.png", { type: "image/png" }),
        ],
      },
    });
    expect(reader).toBeDefined();
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    Object.defineProperty(reader!, "result", {
      value: "data:image/png;base64,AAAA",
    });
    await act(async () =>
      reader!.onload?.call(
        reader!,
        new ProgressEvent("load") as ProgressEvent<FileReader>,
      ),
    );
    expect(
      screen.queryByRole("img", { name: "synthetic.png" }),
    ).not.toBeInTheDocument();
    read.mockRestore();
  });
});

describe("FeedbackDialog real upload identity", () => {
  it("shows a legacy GitHub feedback number without displaying the internal comparison key", async () => {
    mocks.getLastReport.mockResolvedValue({
      ...receipt,
      reportId: "github_issue:42",
      identity: { kind: "github_issue", number: 42 },
    });
    render(<FeedbackDialog open onClose={vi.fn()} />);
    expect(
      await screen.findByText("settings.feedbackIssueNumber"),
    ).toBeVisible();
    expect(screen.queryByText("github_issue:42")).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackDeleteLocalCopy" }),
    );
    await waitFor(() =>
      expect(mocks.deleteLocalReport).toHaveBeenCalledWith("github_issue:42"),
    );
  });

  it("displays the backend report identity rather than an internal comparison key", async () => {
    mocks.getLastReport.mockResolvedValue({
      ...receipt,
      reportId: "synthetic-cas-key",
      identity: { kind: "report", id: "report-visible-0001" },
    });
    render(<FeedbackDialog open onClose={vi.fn()} />);
    expect(await screen.findByText("report-visible-0001")).toBeVisible();
    expect(screen.queryByText("synthetic-cas-key")).not.toBeInTheDocument();
  });
});

describe("FeedbackDialog pending attachment uploads", () => {
  it("waits for a selected screenshot before preparing an upload", async () => {
    let reader: FileReader | undefined;
    const read = vi
      .spyOn(FileReader.prototype, "readAsDataURL")
      .mockImplementation(function (this: FileReader) {
        reader = this;
      });
    try {
      render(<FeedbackDialog open onClose={vi.fn()} />);
      fireEvent.change(
        screen.getByRole("textbox", {
          name: "settings.feedbackDescriptionLabel",
        }),
        {
          target: { value: "Synthetic screenshot feedback" },
        },
      );
      fireEvent.change(
        document.querySelector<HTMLInputElement>('input[type="file"]')!,
        {
          target: {
            files: [
              new File(["synthetic"], "synthetic.png", { type: "image/png" }),
            ],
          },
        },
      );
      const submit = screen.getByRole("button", {
        name: "settings.feedbackSubmit",
      });
      expect(submit).toBeDisabled();
      fireEvent.click(submit);
      expect(mocks.submit).not.toHaveBeenCalled();
      Object.defineProperty(reader!, "result", {
        value: "data:image/png;base64,AAAA",
      });
      await act(async () =>
        reader!.onload?.call(
          reader!,
          new ProgressEvent("load") as ProgressEvent<FileReader>,
        ),
      );
      expect(submit).toBeEnabled();
      fireEvent.click(submit);
      await waitFor(() =>
        expect(mocks.prepare).toHaveBeenCalledWith(
          expect.objectContaining({
            screenshots: [
              { name: "synthetic.png", base64: "data:image/png;base64,AAAA" },
            ],
          }),
        ),
      );
    } finally {
      read.mockRestore();
    }
  });

  it("does not let an old file read unlock a reopened session's pending screenshot", async () => {
    const readers: FileReader[] = [];
    const read = vi
      .spyOn(FileReader.prototype, "readAsDataURL")
      .mockImplementation(function (this: FileReader) {
        readers.push(this);
      });
    try {
      const { rerender } = render(<FeedbackDialog open onClose={vi.fn()} />);
      const choose = (name: string) =>
        fireEvent.change(
          document.querySelector<HTMLInputElement>('input[type="file"]')!,
          {
            target: {
              files: [new File(["synthetic"], name, { type: "image/png" })],
            },
          },
        );
      fireEvent.change(
        screen.getByRole("textbox", {
          name: "settings.feedbackDescriptionLabel",
        }),
        { target: { value: "Screenshot feedback" } },
      );
      choose("old.png");
      rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
      rerender(<FeedbackDialog open onClose={vi.fn()} />);
      choose("new.png");
      expect(readers).toHaveLength(2);
      Object.defineProperty(readers[0], "result", {
        value: "data:image/png;base64,AAAA",
      });
      await act(async () =>
        readers[0].onload?.call(
          readers[0],
          new ProgressEvent("load") as ProgressEvent<FileReader>,
        ),
      );
      expect(
        screen.queryByRole("img", { name: "old.png" }),
      ).not.toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "settings.feedbackSubmit" }),
      ).toBeDisabled();
      Object.defineProperty(readers[1], "result", {
        value: "data:image/png;base64,AAAA",
      });
      await act(async () =>
        readers[1].onload?.call(
          readers[1],
          new ProgressEvent("load") as ProgressEvent<FileReader>,
        ),
      );
      expect(screen.getByRole("img", { name: "new.png" })).toBeVisible();
      expect(
        screen.getByRole("button", { name: "settings.feedbackSubmit" }),
      ).toBeEnabled();
    } finally {
      read.mockRestore();
    }
  });
});

describe("FeedbackDialog translated receipt", () => {
  it("renders the real feedback number and safe byte counters with the English copy", async () => {
    i18n.addResourceBundle("en", "translation", en, true, true);
    await i18n.changeLanguage("en");
    try {
      mocks.getLastReport.mockResolvedValue({
        ...receipt,
        reportId: "github_issue:42",
        identity: { kind: "github_issue", number: 42 },
      });
      render(<FeedbackDialog open onClose={vi.fn()} />);
      expect(await screen.findByText("Feedback #42")).toBeVisible();
      expect(screen.queryByText("github_issue:42")).not.toBeInTheDocument();
      fireEvent.click(
        screen.getByRole("button", { name: "View upload summary" }),
      );
      expect(screen.getByText("Screenshots: 2, 8192 bytes")).toBeVisible();
      expect(screen.getByText("Diagnostics package: 1234 bytes")).toBeVisible();
      expect(
        screen.getByText(
          "Your description and screenshots are uploaded as provided. Check them for private information before submitting.",
        ),
      ).toBeVisible();
      expect(
        screen.getByText(
          "Deletes the local diagnostics package and this receipt. The uploaded report is not deleted.",
        ),
      ).toBeVisible();
    } finally {
      await i18n.changeLanguage("zh");
      i18n.removeResourceBundle("en", "translation");
    }
  });
});

describe("FeedbackDialog copies only real report identities", () => {
  it.each([
    {
      reportId: "synthetic-cas-key",
      identity: { kind: "report", id: "report-visible-0001" },
      expected: "report-visible-0001",
    },
    {
      reportId: "github_issue:42",
      identity: { kind: "github_issue", number: 42 },
      expected: "#42",
    },
  ])(
    "copies $expected without copying the internal comparison key",
    async ({ reportId, identity, expected }) => {
      mocks.getLastReport.mockResolvedValue({ ...receipt, reportId, identity });
      render(<FeedbackDialog open onClose={vi.fn()} />);
      const card = await screen.findByRole("region", {
        name: "settings.feedbackLastReport",
      });
      fireEvent.click(
        within(card).getByRole("button", {
          name: "settings.feedbackCopyIdentity",
        }),
      );
      await waitFor(() =>
        expect(mocks.copyText).toHaveBeenCalledWith(expected),
      );
      expect(await within(card).findByRole("status")).toHaveTextContent(
        "settings.feedbackIdentityCopied",
      );
      expect(mocks.copyText).not.toHaveBeenCalledWith(reportId);
    },
  );

  it("keeps the receipt and reports a safe copy failure", async () => {
    mocks.copyText.mockRejectedValue(new Error("synthetic-private-detail"));
    await renderWithReceipt();
    const card = screen.getByRole("region", {
      name: "settings.feedbackLastReport",
    });
    fireEvent.click(
      within(card).getByRole("button", {
        name: "settings.feedbackCopyIdentity",
      }),
    );
    expect(await within(card).findByRole("alert")).toHaveTextContent(
      "settings.feedbackIdentityCopyFailed",
    );
    expect(screen.getByText(receipt.identity.id)).toBeVisible();
    expect(
      screen.queryByText("synthetic-private-detail"),
    ).not.toBeInTheDocument();
  });

  it("blocks duplicate clipboard writes while copying", async () => {
    const copy = deferred<void>();
    mocks.copyText.mockReturnValue(copy.promise);
    await renderWithReceipt();
    const button = screen.getByRole("button", {
      name: "settings.feedbackCopyIdentity",
    });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(button).toBeDisabled();
    expect(mocks.copyText).toHaveBeenCalledOnce();
    await act(async () => copy.resolve());
    expect(button).toBeEnabled();
  });

  it("does not apply a late clipboard failure to a reopened dialog", async () => {
    const copy = deferred<void>();
    mocks.copyText.mockReturnValue(copy.promise);
    const { rerender } = await renderWithReceipt();
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackCopyIdentity" }),
    );
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    await screen.findByText(receipt.identity.id);
    await act(async () =>
      copy.reject(new Error("late synthetic copy failure")),
    );
    expect(
      screen.queryByText("settings.feedbackIdentityCopyFailed"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "settings.feedbackCopyIdentity" }),
    ).toBeEnabled();
  });
});

describe("FeedbackDialog accepted but unsaved receipt", () => {
  it.each(["missing", "notAttached"])(
    "keeps a copyable current upload identity apart from the persisted receipt when its local copy is %s",
    async (status) => {
      const acceptedReceipt = {
        ...receipt,
        reportId: "github_issue:42",
        identity: { kind: "github_issue", number: 42 },
        uploadedAt: "2026-10-18T01:00:00Z",
        localCopy:
          status === "notAttached"
            ? { status }
            : { status, fileName: "diagnostics.zip", bytes: 1234 },
      };
      mocks.submit.mockResolvedValue({
        status: "accepted",
        localSave: "failed",
        receipt: acceptedReceipt,
      });
      await renderWithReceipt();
      await submitDescription();
      const unsaved = await screen.findByRole("region", {
        name: "settings.feedbackUnsavedUpload",
      });
      expect(
        within(unsaved).getByText("settings.feedbackIssueNumber"),
      ).toBeVisible();
      expect(
        within(unsaved).getByText(acceptedReceipt.uploadedAt),
      ).toBeVisible();
      expect(within(unsaved).getByRole("alert")).toHaveTextContent(
        "settings.feedbackLocalSaveFailed",
      );
      expect(
        within(unsaved).queryByRole("button", {
          name: "settings.feedbackExportLocalCopy",
        }),
      ).not.toBeInTheDocument();
      expect(
        within(unsaved).queryByRole("button", {
          name: "settings.feedbackDeleteLocalCopy",
        }),
      ).not.toBeInTheDocument();
      expect(screen.getByText(receipt.identity.id)).toBeVisible();
      fireEvent.click(
        within(unsaved).getByRole("button", {
          name: "settings.feedbackCopyIdentity",
        }),
      );
      await waitFor(() => expect(mocks.copyText).toHaveBeenCalledWith("#42"));
      expect(mocks.copyText).not.toHaveBeenCalledWith("github_issue:42");
    },
  );

  it("preserves the unsaved uploaded identity when deleting the older persisted receipt", async () => {
    mocks.submit.mockResolvedValue({
      status: "accepted",
      localSave: "failed",
      receipt: {
        ...receipt,
        reportId: "new-cas-key",
        identity: { kind: "report", id: "new-uploaded-id" },
        localCopy: {
          status: "missing",
          fileName: "diagnostics.zip",
          bytes: 1234,
        },
      },
    });
    await renderWithReceipt();
    await submitDescription();
    const unsaved = await screen.findByRole("region", {
      name: "settings.feedbackUnsavedUpload",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackDeleteLocalCopy" }),
    );
    await waitFor(() =>
      expect(screen.queryByText(receipt.identity.id)).not.toBeInTheDocument(),
    );
    expect(within(unsaved).getByText("new-uploaded-id")).toBeVisible();
    expect(within(unsaved).getByRole("alert")).toHaveTextContent(
      "settings.feedbackLocalSaveFailed",
    );
    expect(mocks.deleteLocalReport).toHaveBeenCalledWith(receipt.reportId);
    expect(mocks.deleteLocalReport).not.toHaveBeenCalledWith("new-cas-key");
  });
});

function relatedAppGroup() {
  return screen.getByRole("group", { name: "settings.feedbackRelatedApps" });
}

function appNames() {
  return within(relatedAppGroup())
    .getAllByRole("checkbox")
    .map((box) => box.getAttribute("aria-label"));
}

const defaultAppNames = () => [
  ...APP_IDS.map((app) => getAppDisplayName(app, i18n.t.bind(i18n))),
  "settings.feedbackOtherApp",
];

function contactInput() {
  return screen.getByRole("textbox", { name: "settings.feedbackContactLabel" });
}

describe("FeedbackDialog related apps and optional contact", () => {
  it("reuses all registry labels in stable order without preselecting apps or contact", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    await waitFor(() =>
      expect(mocks.getUsageSummaryByApp).toHaveBeenCalledOnce(),
    );
    expect(appNames()).toEqual(defaultAppNames());
    for (const box of within(relatedAppGroup()).getAllByRole("checkbox"))
      expect(box).not.toBeChecked();
    expect(contactInput()).toHaveValue("");
    expect(contactInput()).toHaveAttribute("autocomplete", "off");
    expect(screen.getByText("settings.feedbackContactHint")).toBeVisible();
    await submitDescription();
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
    expect(mocks.prepare.mock.calls[0][0]).toMatchObject({ relatedApps: [] });
    expect(mocks.prepare.mock.calls[0][0]).not.toHaveProperty("contact");
  });

  it("orders by available local request frequency, not token totals or folded Claude usage", async () => {
    mocks.getUsageSummaryByApp.mockResolvedValue([
      {
        appType: "codex",
        summary: { totalRequests: 3, realTotalTokens: 9000 },
      },
      { appType: "pi", summary: { totalRequests: 8, realTotalTokens: 1 } },
      { appType: "claude", summary: { totalRequests: 100 } },
      { appType: "claude-desktop", summary: { totalRequests: 100 } },
      { appType: "gemini", summary: { totalRequests: 3 } },
      { appType: "unregistered", summary: { totalRequests: 10000 } },
    ]);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    await waitFor(() =>
      expect(appNames().slice(0, 3)).toEqual(["Pi", "Codex", "Gemini"]),
    );
    expect(appNames().slice(3)).toEqual(
      defaultAppNames().filter(
        (name) => !["Pi", "Codex", "Gemini"].includes(name),
      ),
    );
    expect(mocks.getUsageSummaryByApp).toHaveBeenCalledWith();
    expect(
      screen.queryByText("settings.feedbackAppOrderHint"),
    ).not.toBeInTheDocument();
  });

  it("keeps registry order when request counts are unavailable or invalid", async () => {
    mocks.getUsageSummaryByApp.mockResolvedValue([
      { appType: "pi", summary: { totalRequests: -1 } },
      { appType: "codex", summary: { totalRequests: Number.NaN } },
      {
        appType: "gemini",
        summary: { totalRequests: Number.MAX_SAFE_INTEGER + 1 },
      },
      { appType: "grokbuild", summary: { totalRequests: 1.5 } },
    ]);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    await act(async () => {});
    expect(appNames()).toEqual(defaultAppNames());
  });

  it("allows multiple apps, other, and deselection; sends only their stable IDs", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
    );
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    );
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Claude Code" }),
    );
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Claude Code" }),
    );
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", {
        name: "settings.feedbackOtherApp",
      }),
    );
    await submitDescription();
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
    expect(mocks.prepare.mock.calls[0][0]).toMatchObject({
      relatedApps: ["codex", "pi", "other"],
    });
    expect(mocks.prepare.mock.calls[0][0]).not.toHaveProperty("usage");
  });

  it("does not let a late frequency response clear the user's selection", async () => {
    const usage = deferred<unknown>();
    mocks.getUsageSummaryByApp.mockReturnValue(usage.promise);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
    );
    await act(async () =>
      usage.resolve([{ appType: "pi", summary: { totalRequests: 9 } }]),
    );
    expect(appNames()[0]).toBe("Pi");
    expect(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
    ).toBeChecked();
    expect(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    ).not.toBeChecked();
  });

  it("ignores an old session's late frequency response", async () => {
    const oldUsage = deferred<unknown>();
    mocks.getUsageSummaryByApp
      .mockReturnValueOnce(oldUsage.promise)
      .mockResolvedValueOnce([]);
    const { rerender } = render(<FeedbackDialog open onClose={vi.fn()} />);
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() =>
      expect(mocks.getUsageSummaryByApp).toHaveBeenCalledTimes(2),
    );
    await act(async () =>
      oldUsage.resolve([{ appType: "pi", summary: { totalRequests: 9 } }]),
    );
    expect(appNames()).toEqual(defaultAppNames());
  });

  it("keeps stable options and allows submission when local usage cannot be read", async () => {
    mocks.getUsageSummaryByApp.mockRejectedValue(
      new Error("synthetic-private-usage-error"),
    );
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    await act(async () => {});
    expect(appNames()).toEqual(defaultAppNames());
    expect(
      screen.queryByText("synthetic-private-usage-error"),
    ).not.toBeInTheDocument();
    await submitDescription();
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
  });

  it("sends an explicitly supplied contact separately and trimmed, then clears the sent draft", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.change(contactInput(), {
      target: { value: "  followup@example.invalid  " },
    });
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    );
    await submitDescription("Synthetic issue description");
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
    expect(mocks.prepare.mock.calls[0][0]).toMatchObject({
      description: "Synthetic issue description",
      relatedApps: ["pi"],
      contact: "followup@example.invalid",
    });
    await screen.findByText(receipt.identity.id);
    expect(contactInput()).toHaveValue("");
    expect(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    ).not.toBeChecked();
    expect(
      screen.queryByText("followup@example.invalid"),
    ).not.toBeInTheDocument();
  });

  it("omits whitespace-only contact", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.change(contactInput(), { target: { value: "   " } });
    await submitDescription();
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
    expect(mocks.prepare.mock.calls[0][0]).not.toHaveProperty("contact");
  });

  it("accepts 200 Unicode characters after trimming instead of using UTF-16 length", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.change(contactInput(), {
      target: { value: ` ${"🧪".repeat(200)} ` },
    });
    await submitDescription();
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
    expect(mocks.prepare.mock.calls[0][0].contact).toBe("🧪".repeat(200));
  });

  it("blocks overlong contact before invoking upload and retains the draft", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.change(contactInput(), { target: { value: "🧪".repeat(201) } });
    await submitDescription();
    expect(mocks.submit).not.toHaveBeenCalled();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "settings.feedbackContactTooLong",
    );
    expect(contactInput()).toHaveValue("🧪".repeat(201));
  });

  it.each(["rejected", "unknown", "contact_unsupported"])(
    "retains app and contact input after %s",
    async (kind) => {
      if (kind === "unknown")
        mocks.submit.mockResolvedValue({ status: "unknown" });
      else
        mocks.submit.mockRejectedValue(
          kind === "contact_unsupported"
            ? "feedback_contact_unsupported"
            : new Error("synthetic-private-error"),
        );
      await renderWithReceipt();
      await waitFor(() => expect(contactInput()).toBeEnabled());
      fireEvent.change(contactInput(), {
        target: { value: "followup@example.invalid" },
      });
      fireEvent.click(
        within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
      );
      await submitDescription();
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "settings.feedbackSubmit" }),
        ).toBeEnabled(),
      );
      expect(contactInput()).toHaveValue("followup@example.invalid");
      expect(
        within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
      ).toBeChecked();
      expect(screen.getByText(receipt.identity.id)).toBeVisible();
      if (kind === "contact_unsupported")
        expect(screen.getByRole("alert")).toHaveTextContent(
          "settings.feedbackContactUnsupported",
        );
      expect(
        screen.queryByText("synthetic-private-error"),
      ).not.toBeInTheDocument();
    },
  );

  it("shows only contact presence in a saved receipt summary", async () => {
    mocks.getLastReport.mockResolvedValue({
      ...receipt,
      summary: { ...receipt.summary, contactIncluded: true },
    });
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    await screen.findByText(receipt.identity.id);
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackViewSummary" }),
    );
    expect(screen.getByText("settings.feedbackSummaryContact")).toBeVisible();
    expect(contactInput()).toHaveValue("");
  });
});

describe("FeedbackDialog extended field capability gate", () => {
  it.each(["pending", "unsupported", "failed"])(
    "hides optional fields for %s capability while allowing basic feedback",
    async (kind) => {
      const capability = deferred<unknown>();
      if (kind === "pending")
        mocks.getFormCapabilities.mockReturnValue(capability.promise);
      else if (kind === "failed")
        mocks.getFormCapabilities.mockRejectedValue(
          new Error("synthetic-capability-error"),
        );
      else
        mocks.getFormCapabilities.mockResolvedValue({ extendedFields: false });
      render(<FeedbackDialog open onClose={vi.fn()} />);
      await act(async () => {});
      expect(
        screen.queryByRole("group", { name: "settings.feedbackRelatedApps" }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("textbox", {
          name: "settings.feedbackContactLabel",
        }),
      ).not.toBeInTheDocument();
      expect(
        screen.getByText(
          kind === "pending"
            ? "settings.feedbackCapabilitiesLoading"
            : kind === "failed"
              ? "settings.feedbackCapabilitiesFailed"
              : "settings.feedbackExtendedFieldsUnsupported",
        ),
      ).toBeVisible();
      await submitDescription();
      await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
      expect(mocks.prepare.mock.calls[0][0]).toMatchObject({ relatedApps: [] });
      expect(mocks.prepare.mock.calls[0][0]).not.toHaveProperty("contact");
      expect(
        screen.queryByText("synthetic-capability-error"),
      ).not.toBeInTheDocument();
      if (kind === "pending")
        await act(async () => capability.resolve({ extendedFields: false }));
    },
  );

  it("retries a failed capability read without losing the draft or submitting", async () => {
    const retry = deferred<unknown>();
    mocks.getFormCapabilities
      .mockRejectedValueOnce(new Error("private failure"))
      .mockReturnValueOnce(retry.promise);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    const description = screen.getByRole("textbox", {
      name: "settings.feedbackDescriptionLabel",
    });
    fireEvent.change(description, {
      target: { value: "Synthetic retained draft" },
    });
    const button = await screen.findByRole("button", {
      name: "settings.feedbackCapabilitiesRetry",
    });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(mocks.getFormCapabilities).toHaveBeenCalledTimes(2);
    expect(
      screen.getByText("settings.feedbackCapabilitiesLoading"),
    ).toBeVisible();
    await act(async () => retry.resolve({ extendedFields: true }));
    expect(contactInput()).toBeEnabled();
    expect(description).toHaveValue("Synthetic retained draft");
    expect(mocks.prepare).not.toHaveBeenCalled();
    expect(mocks.submit).not.toHaveBeenCalled();
  });

  it("ignores a retry result after reopening into an unsupported session", async () => {
    const retry = deferred<unknown>();
    mocks.getFormCapabilities
      .mockRejectedValueOnce(new Error("synthetic failure"))
      .mockReturnValueOnce(retry.promise)
      .mockResolvedValueOnce({ extendedFields: false });
    const { rerender } = render(<FeedbackDialog open onClose={vi.fn()} />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "settings.feedbackCapabilitiesRetry",
      }),
    );
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    await screen.findByText("settings.feedbackExtendedFieldsUnsupported");
    await act(async () => retry.resolve({ extendedFields: true }));
    expect(
      screen.queryByRole("textbox", { name: "settings.feedbackContactLabel" }),
    ).not.toBeInTheDocument();
    expect(mocks.getFormCapabilities).toHaveBeenCalledTimes(3);
  });

  it("does not enable a new dialog session from an old capability response", async () => {
    const capability = deferred<unknown>();
    mocks.getFormCapabilities
      .mockReturnValueOnce(capability.promise)
      .mockResolvedValueOnce({ extendedFields: false });
    const { rerender } = render(<FeedbackDialog open onClose={vi.fn()} />);
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() =>
      expect(mocks.getFormCapabilities).toHaveBeenCalledTimes(2),
    );
    await act(async () => capability.resolve({ extendedFields: true }));
    expect(
      screen.queryByRole("textbox", { name: "settings.feedbackContactLabel" }),
    ).not.toBeInTheDocument();
  });

  it("retains optional draft fields if capability disappears and requires explicit clearing before basic upload", async () => {
    mocks.getFormCapabilities
      .mockResolvedValueOnce({ extendedFields: true })
      .mockResolvedValueOnce({ extendedFields: false });
    const { rerender } = render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.change(contactInput(), {
      target: { value: "followup@example.invalid" },
    });
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
    );
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() =>
      expect(mocks.getFormCapabilities).toHaveBeenCalledTimes(2),
    );
    await act(async () => {});
    expect(contactInput()).toBeEnabled();
    expect(contactInput()).toHaveValue("followup@example.invalid");
    expect(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
    ).toBeChecked();
    await submitDescription();
    expect(mocks.submit).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "settings.feedbackExtendedFieldsUnsupported",
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "settings.feedbackClearOptionalFields",
      }),
    );
    expect(
      screen.queryByRole("group", { name: "settings.feedbackRelatedApps" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "settings.feedbackContactLabel" }),
    ).not.toBeInTheDocument();
    await submitDescription();
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
  });

  it("preserves input and explains the native capability rejection without showing raw details", async () => {
    mocks.submit.mockRejectedValue("feedback_extended_fields_unsupported");
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    );
    await submitDescription();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "settings.feedbackExtendedFieldsUnsupported",
    );
    expect(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    ).toBeChecked();
    expect(
      screen.queryByText("feedback_extended_fields_unsupported"),
    ).not.toBeInTheDocument();
  });
});

describe("FeedbackDialog automatic app default", () => {
  it("checks only the most frequent app on the first new form", async () => {
    mocks.getUsageSummaryByApp.mockResolvedValue([
      { appType: "codex", summary: { totalRequests: 2 } },
      { appType: "pi", summary: { totalRequests: 8 } },
    ]);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() =>
      expect(
        within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
      ).toBeChecked(),
    );
    expect(
      within(relatedAppGroup()).getAllByRole("checkbox", { checked: true }),
    ).toHaveLength(1);
    await submitDescription();
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
    expect(mocks.prepare.mock.calls[0][0]).toMatchObject({
      relatedApps: ["pi"],
    });
  });

  it("breaks equal positive counts by registry order", async () => {
    mocks.getUsageSummaryByApp.mockResolvedValue([
      { appType: "pi", summary: { totalRequests: 5 } },
      { appType: "codex", summary: { totalRequests: 5 } },
    ]);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() =>
      expect(
        within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
      ).toBeChecked(),
    );
    expect(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    ).not.toBeChecked();
  });

  it("waits for confirmed extended-field capability before applying the default", async () => {
    const capability = deferred<unknown>();
    mocks.getFormCapabilities.mockReturnValue(capability.promise);
    mocks.getUsageSummaryByApp.mockResolvedValue([
      { appType: "pi", summary: { totalRequests: 5 } },
    ]);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await act(async () => {});
    expect(
      screen.queryByRole("checkbox", { name: "Pi" }),
    ).not.toBeInTheDocument();
    await act(async () => capability.resolve({ extendedFields: true }));
    expect(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    ).toBeChecked();
  });

  it("never checks an app when capability is unavailable", async () => {
    mocks.getFormCapabilities.mockResolvedValue({ extendedFields: false });
    mocks.getUsageSummaryByApp.mockResolvedValue([
      { appType: "pi", summary: { totalRequests: 5 } },
    ]);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await act(async () => {});
    expect(
      screen.queryByRole("group", { name: "settings.feedbackRelatedApps" }),
    ).not.toBeInTheDocument();
  });

  it("does not restore a default after the user unchecks it, rerenders, or reopens the same draft", async () => {
    mocks.getUsageSummaryByApp.mockResolvedValue([
      { appType: "pi", summary: { totalRequests: 5 } },
    ]);
    const { rerender } = render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() =>
      expect(
        within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
      ).toBeChecked(),
    );
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    );
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    expect(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    ).not.toBeChecked();
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    expect(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    ).not.toBeChecked();
  });

  it("does not override a manual select then deselect when usage arrives late", async () => {
    const usage = deferred<unknown>();
    mocks.getUsageSummaryByApp.mockReturnValue(usage.promise);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
    );
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
    );
    await act(async () =>
      usage.resolve([{ appType: "pi", summary: { totalRequests: 5 } }]),
    );
    for (const box of within(relatedAppGroup()).getAllByRole("checkbox"))
      expect(box).not.toBeChecked();
  });

  it("starts a fresh default only after an accepted upload resets the form", async () => {
    mocks.getUsageSummaryByApp.mockResolvedValue([
      { appType: "pi", summary: { totalRequests: 5 } },
    ]);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() =>
      expect(
        within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
      ).toBeChecked(),
    );
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    );
    await submitDescription();
    await screen.findByText(receipt.identity.id);
    expect(mocks.prepare.mock.calls[0][0].relatedApps).toEqual([]);
    expect(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    ).toBeChecked();
  });
});

describe("FeedbackDialog extended field preflight", () => {
  it.each([
    "\tfollowup@example.invalid",
    "followup@example.invalid\t",
    "prefix\u0000suffix",
    "\u0085",
  ])(
    "rejects a contact containing control characters before trimming: %j",
    async (contact) => {
      render(<FeedbackDialog open onClose={vi.fn()} />);
      await waitFor(() => expect(contactInput()).toBeEnabled());
      fireEvent.change(contactInput(), { target: { value: contact } });
      await submitDescription();
      expect(mocks.submit).not.toHaveBeenCalled();
      expect(await screen.findByRole("alert")).toHaveTextContent(
        "settings.feedbackContactInvalid",
      );
      expect(contactInput()).toHaveValue(contact);
    },
  );

  it("sends selections in stable registry order even when clicked in another order", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", {
        name: "settings.feedbackOtherApp",
      }),
    );
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
    );
    fireEvent.click(
      within(relatedAppGroup()).getByRole("checkbox", { name: "Codex" }),
    );
    await submitDescription();
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
    expect(mocks.prepare.mock.calls[0][0].relatedApps).toEqual([
      "codex",
      "pi",
      "other",
    ]);
  });
});

it("does not change submitted app consent when frequency arrives during an upload", async () => {
  const usage = deferred<unknown>();
  const upload = deferred<unknown>();
  mocks.getUsageSummaryByApp.mockReturnValue(usage.promise);
  mocks.submit.mockReturnValue(upload.promise);
  render(<FeedbackDialog open onClose={vi.fn()} />);
  await waitFor(() => expect(contactInput()).toBeEnabled());
  await submitDescription();
  expect(mocks.prepare.mock.calls[0][0].relatedApps).toEqual([]);
  await act(async () =>
    usage.resolve([{ appType: "pi", summary: { totalRequests: 5 } }]),
  );
  expect(
    within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
  ).not.toBeChecked();
  await act(async () => upload.resolve({ status: "unknown" }));
  expect(
    within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
  ).not.toBeChecked();
});

describe("FeedbackDialog contact Unicode whitespace contract", () => {
  it("preserves BOM because it is not Unicode White_Space", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.change(contactInput(), {
      target: { value: "\uFEFFfollowup@example.invalid\uFEFF" },
    });
    await submitDescription();
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
    expect(mocks.prepare.mock.calls[0][0].contact).toBe(
      "\uFEFFfollowup@example.invalid\uFEFF",
    );
  });

  it("includes non-whitespace BOM in the contact character limit", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.change(contactInput(), {
      target: { value: "\uFEFF".repeat(201) },
    });
    await submitDescription();
    expect(mocks.submit).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "settings.feedbackContactTooLong",
    );
  });
});

function previewButton() {
  return screen.getByRole("button", { name: "settings.feedbackSubmit" });
}

async function prepareDraft() {
  fireEvent.change(
    screen.getByRole("textbox", { name: "settings.feedbackDescriptionLabel" }),
    { target: { value: "Synthetic previewed description" } },
  );
  fireEvent.click(previewButton());
  return await screen.findByRole("region", {
    name: "settings.feedbackPreviewTitle",
  });
}

describe("FeedbackDialog frozen upload preview", () => {
  it("shows native file names, byte sizes, hashes and safe JSON before token-only submission", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    expect(
      screen.queryByRole("button", { name: "settings.feedbackConfirmSubmit" }),
    ).not.toBeInTheDocument();
    const preview = await prepareDraft();
    expect(within(preview).getByText("synthetic.png")).toBeVisible();
    expect(within(preview).getByText("diagnostics.zip")).toBeVisible();
    expect(within(preview).getByText("a".repeat(64))).toBeVisible();
    expect(within(preview).getByText("b".repeat(64))).toBeVisible();
    expect(within(preview).getByText(/safe_summary/)).toHaveTextContent(
      "<img src=x onerror=synthetic>",
    );
    expect(within(preview).queryByRole("img")).not.toBeInTheDocument();
    expect(mocks.submit).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackConfirmSubmit" }),
    );
    await waitFor(() =>
      expect(mocks.submit).toHaveBeenCalledWith({
        previewId: prepared.previewId,
      }),
    );
    expect(mocks.prepare).toHaveBeenCalledOnce();
  });

  it.each(["description", "contact", "app", "diagnostics", "sites"])(
    "invalidates and cancels the preview when %s changes",
    async (field) => {
      render(<FeedbackDialog open onClose={vi.fn()} />);
      await waitFor(() => expect(contactInput()).toBeEnabled());
      await prepareDraft();
      if (field === "description")
        fireEvent.change(
          screen.getByRole("textbox", {
            name: "settings.feedbackDescriptionLabel",
          }),
          { target: { value: "Edited draft" } },
        );
      if (field === "contact")
        fireEvent.change(contactInput(), {
          target: { value: "followup@example.invalid" },
        });
      if (field === "app")
        fireEvent.click(
          within(relatedAppGroup()).getByRole("checkbox", { name: "Pi" }),
        );
      if (field === "diagnostics")
        fireEvent.click(
          screen.getByRole("checkbox", {
            name: /settings.feedbackIncludeDiagnostics/,
          }),
        );
      if (field === "sites")
        fireEvent.click(
          screen.getByRole("checkbox", {
            name: /settings.feedbackIncludeSites/,
          }),
        );
      expect(
        screen.queryByRole("region", { name: "settings.feedbackPreviewTitle" }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", {
          name: "settings.feedbackConfirmSubmit",
        }),
      ).not.toBeInTheDocument();
      await waitFor(() =>
        expect(mocks.cancelPreview).toHaveBeenCalledWith(prepared.previewId),
      );
      expect(mocks.submit).not.toHaveBeenCalled();
    },
  );

  it("cancels a late prepare result instead of attaching it to an edited draft", async () => {
    const pending = deferred<unknown>();
    mocks.prepare.mockReturnValue(pending.promise);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "Before prepare" } },
    );
    fireEvent.click(previewButton());
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "After prepare" } },
    );
    await act(async () => pending.resolve(prepared));
    expect(
      screen.queryByRole("region", { name: "settings.feedbackPreviewTitle" }),
    ).not.toBeInTheDocument();
    expect(mocks.cancelPreview).toHaveBeenCalledWith(prepared.previewId);
    expect(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
    ).toHaveValue("After prepare");
  });

  it("cancels a returned preview after closing or unmounting its session", async () => {
    const pending = deferred<unknown>();
    mocks.prepare.mockReturnValue(pending.promise);
    const { unmount } = render(<FeedbackDialog open onClose={vi.fn()} />);
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "Closing draft" } },
    );
    fireEvent.click(previewButton());
    unmount();
    await act(async () => pending.resolve(prepared));
    expect(mocks.cancelPreview).toHaveBeenCalledWith(prepared.previewId);
    expect(mocks.submit).not.toHaveBeenCalled();
  });

  it("cancels the active preview when the user closes the dialog", async () => {
    const onClose = vi.fn();
    render(<FeedbackDialog open onClose={onClose} />);
    await prepareDraft();
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    expect(onClose).toHaveBeenCalledOnce();
    expect(mocks.cancelPreview).toHaveBeenCalledWith(prepared.previewId);
  });

  it("preserves the draft but clears an older preview if preparing again fails", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await prepareDraft();
    mocks.prepare.mockRejectedValueOnce("synthetic-private-prepare-error");
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: /settings.feedbackIncludeDiagnostics/,
      }),
    );
    fireEvent.click(previewButton());
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "settings.feedbackPrepareFailed",
    );
    expect(
      screen.queryByRole("region", { name: "settings.feedbackPreviewTitle" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
    ).toHaveValue("Synthetic previewed description");
    expect(
      screen.queryByText("synthetic-private-prepare-error"),
    ).not.toBeInTheDocument();
  });

  it.each(["feedback_preview_changed", "feedback_preview_unavailable"])(
    "requires a new preview after %s without retrying upload",
    async (error) => {
      mocks.submit.mockRejectedValue(error);
      render(<FeedbackDialog open onClose={vi.fn()} />);
      await prepareDraft();
      fireEvent.click(
        screen.getByRole("button", { name: "settings.feedbackConfirmSubmit" }),
      );
      expect(await screen.findByRole("alert")).toHaveTextContent(
        error === "feedback_preview_changed"
          ? "settings.feedbackPreviewChanged"
          : "settings.feedbackPreviewUnavailable",
      );
      expect(
        screen.queryByRole("region", { name: "settings.feedbackPreviewTitle" }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", {
          name: "settings.feedbackConfirmSubmit",
        }),
      ).not.toBeInTheDocument();
      expect(mocks.prepare).toHaveBeenCalledOnce();
      expect(mocks.submit).toHaveBeenCalledOnce();
    },
  );

  it("does not invent configuration JSON or diagnostics files when the native preview has none", async () => {
    mocks.prepare.mockResolvedValue({
      ...prepared,
      files: [],
      totalBytes: 0,
      applicationConfigs: null,
      summary: {
        ...receipt.summary,
        diagnosticsIncluded: false,
        diagnosticsBytes: 0,
      },
    });
    render(<FeedbackDialog open onClose={vi.fn()} />);
    const preview = await prepareDraft();
    expect(
      within(preview).queryByText("diagnostics.zip"),
    ).not.toBeInTheDocument();
    expect(
      within(preview).getByText("settings.feedbackPreviewNoFiles"),
    ).toBeVisible();
    expect(within(preview).queryByText(/safe_summary/)).not.toBeInTheDocument();
  });
});

describe("FeedbackDialog preview resource and lifecycle boundaries", () => {
  it("focuses the completed preview so the user can inspect its actual contents", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    const preview = await prepareDraft();
    await waitFor(() => expect(preview).toHaveFocus());
  });

  it.each([
    " ".repeat(1025),
    " ".repeat(825) + "a".repeat(200),
    "🧪".repeat(513),
  ])(
    "rejects oversized raw contact before trimming or code-point counting",
    async (contact) => {
      render(<FeedbackDialog open onClose={vi.fn()} />);
      await waitFor(() => expect(contactInput()).toBeEnabled());
      expect(contactInput()).toHaveAttribute("maxlength", "1024");
      fireEvent.change(contactInput(), { target: { value: contact } });
      await submitDescription();
      expect(mocks.prepare).not.toHaveBeenCalled();
      expect(mocks.submit).not.toHaveBeenCalled();
      expect(screen.getByRole("alert")).toHaveTextContent(
        "settings.feedbackContactInvalid",
      );
      expect(contactInput()).toHaveValue(contact);
    },
  );

  it("accepts the exact raw UTF-16 limit with a valid trimmed contact", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    fireEvent.change(contactInput(), {
      target: { value: " ".repeat(824) + "a".repeat(200) },
    });
    await submitDescription();
    expect(mocks.prepare.mock.calls[0][0].contact).toBe("a".repeat(200));
  });

  it("invalidates the frozen preview when screenshots are added or removed", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await prepareDraft();
    fireEvent.change(
      document.querySelector<HTMLInputElement>('input[type="file"]')!,
      {
        target: {
          files: [new File(["synthetic"], "added.png", { type: "image/png" })],
        },
      },
    );
    expect(mocks.cancelPreview).toHaveBeenCalledWith(prepared.previewId);
    expect(
      screen.queryByRole("region", { name: "settings.feedbackPreviewTitle" }),
    ).not.toBeInTheDocument();
    await screen.findByRole("img", { name: "added.png" });
    mocks.prepare.mockResolvedValueOnce({
      ...prepared,
      previewId: "synthetic-preview-2",
    });
    fireEvent.click(previewButton());
    await screen.findByRole("region", {
      name: "settings.feedbackPreviewTitle",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackRemoveScreenshot" }),
    );
    expect(mocks.cancelPreview).toHaveBeenCalledWith("synthetic-preview-2");
    expect(
      screen.queryByRole("button", { name: "settings.feedbackConfirmSubmit" }),
    ).not.toBeInTheDocument();
  });

  it("prevents repeated prepare clicks from starting duplicate work", async () => {
    const pending = deferred<unknown>();
    mocks.prepare.mockReturnValue(pending.promise);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "Synthetic draft" } },
    );
    const button = previewButton();
    fireEvent.click(button);
    fireEvent.click(button);
    expect(mocks.prepare).toHaveBeenCalledOnce();
    await act(async () => pending.resolve(prepared));
  });

  it("does not clear a newer preview when cancellation of an older one returns late", async () => {
    const cancellation = deferred<void>();
    mocks.cancelPreview.mockReturnValueOnce(cancellation.promise);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await prepareDraft();
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "Newer draft" } },
    );
    mocks.prepare.mockResolvedValueOnce({
      ...prepared,
      previewId: "synthetic-preview-2",
    });
    fireEvent.click(previewButton());
    await screen.findByRole("region", {
      name: "settings.feedbackPreviewTitle",
    });
    await act(async () => cancellation.resolve());
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackConfirmSubmit" }),
    );
    expect(mocks.submit).toHaveBeenCalledWith({
      previewId: "synthetic-preview-2",
    });
    expect(mocks.cancelPreview).not.toHaveBeenCalledWith("synthetic-preview-2");
  });

  it("does not resurrect a token or expose raw errors after cancellation fails", async () => {
    mocks.cancelPreview.mockRejectedValueOnce(
      new Error("synthetic-private-cancel-error"),
    );
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await prepareDraft();
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "Edited draft" } },
    );
    await act(async () => {});
    expect(
      screen.queryByRole("button", { name: "settings.feedbackConfirmSubmit" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("synthetic-private-cancel-error"),
    ).not.toBeInTheDocument();
    expect(mocks.submit).not.toHaveBeenCalled();
  });

  it("never automatically retries or restores the token after an unknown upload outcome", async () => {
    mocks.submit.mockResolvedValue({ status: "unknown" });
    await renderWithReceipt();
    await submitDescription();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "settings.feedbackUploadUnknown",
    );
    expect(
      screen.queryByRole("button", { name: "settings.feedbackConfirmSubmit" }),
    ).not.toBeInTheDocument();
    expect(mocks.submit).toHaveBeenCalledOnce();
    expect(mocks.prepare).toHaveBeenCalledOnce();
    expect(
      screen.getByRole("button", { name: "settings.feedbackSubmit" }),
    ).toBeEnabled();
    expect(screen.getByText(receipt.identity.id)).toBeVisible();
    expect(
      screen.queryByRole("region", { name: "settings.feedbackPreviewTitle" }),
    ).not.toBeInTheDocument();
  });

  it("cancels a closed session's late token before preparing the reopened draft", async () => {
    const pending = deferred<unknown>();
    mocks.prepare
      .mockReturnValueOnce(pending.promise)
      .mockResolvedValueOnce({ ...prepared, previewId: "reopened-preview" });
    const { rerender } = render(<FeedbackDialog open onClose={vi.fn()} />);
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "Old draft" } },
    );
    fireEvent.click(previewButton());
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "New draft" } },
    );
    await act(async () => pending.resolve(prepared));
    expect(mocks.cancelPreview).toHaveBeenCalledWith(prepared.previewId);
    expect(
      screen.queryByRole("region", { name: "settings.feedbackPreviewTitle" }),
    ).not.toBeInTheDocument();
    fireEvent.click(previewButton());
    await screen.findByRole("region", {
      name: "settings.feedbackPreviewTitle",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackConfirmSubmit" }),
    );
    expect(mocks.submit).toHaveBeenCalledWith({
      previewId: "reopened-preview",
    });
  });
});

describe("FeedbackDialog direct submission interaction", () => {
  it("prepares a two-character basic report from Submit without uploading until confirmation", async () => {
    mocks.getFormCapabilities.mockResolvedValue({ extendedFields: false });
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await act(async () => {});
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "测试" } },
    );
    const submit = screen.getByRole("button", {
      name: "settings.feedbackSubmit",
    });
    expect(submit).toBeEnabled();
    fireEvent.click(submit);
    await screen.findByRole("region", {
      name: "settings.feedbackPreviewTitle",
    });
    expect(mocks.prepare).toHaveBeenCalledWith(
      expect.objectContaining({ description: "测试", relatedApps: [] }),
    );
    expect(mocks.submit).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackConfirmSubmit" }),
    );
    await waitFor(() =>
      expect(mocks.submit).toHaveBeenCalledWith({
        previewId: prepared.previewId,
      }),
    );
  });

  it.each(["pending", "unsupported", "failed"])(
    "does not present dead optional controls when capability is %s",
    async (kind) => {
      if (kind === "pending")
        mocks.getFormCapabilities.mockReturnValue(new Promise(() => {}));
      else if (kind === "failed")
        mocks.getFormCapabilities.mockRejectedValue(new Error("unavailable"));
      else
        mocks.getFormCapabilities.mockResolvedValue({ extendedFields: false });
      render(<FeedbackDialog open onClose={vi.fn()} />);
      await act(async () => {});
      expect(
        screen.queryByRole("checkbox", { name: "Codex" }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("textbox", {
          name: "settings.feedbackContactLabel",
        }),
      ).not.toBeInTheDocument();
      expect(
        screen.getByText(
          kind === "pending"
            ? "settings.feedbackCapabilitiesLoading"
            : kind === "failed"
              ? "settings.feedbackCapabilitiesFailed"
              : "settings.feedbackExtendedFieldsUnsupported",
        ),
      ).toBeVisible();
    },
  );

  it("toggles Codex on and off and submits with no applications", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    const codex = await screen.findByRole("checkbox", { name: "Codex" });
    fireEvent.click(codex);
    expect(codex).toBeChecked();
    fireEvent.click(codex);
    expect(codex).not.toBeChecked();
    expect(
      screen.queryByText("settings.feedbackAppOrderHint"),
    ).not.toBeInTheDocument();
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "测试" } },
    );
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackSubmit" }),
    );
    await screen.findByRole("button", {
      name: "settings.feedbackConfirmSubmit",
    });
    expect(mocks.prepare).toHaveBeenCalledWith(
      expect.objectContaining({ relatedApps: [] }),
    );
  });

  it("recovers after a failed preparation and admits only one repeated click per operation", async () => {
    const preparation = deferred<typeof prepared>();
    mocks.prepare
      .mockReturnValueOnce(preparation.promise)
      .mockResolvedValueOnce(prepared);
    render(<FeedbackDialog open onClose={vi.fn()} />);
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "测试" } },
    );
    const submit = screen.getByRole("button", {
      name: "settings.feedbackSubmit",
    });
    fireEvent.click(submit);
    fireEvent.click(submit);
    expect(mocks.prepare).toHaveBeenCalledOnce();
    await act(async () => preparation.reject(new Error("failed")));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "settings.feedbackPrepareFailed",
    );
    expect(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
    ).toHaveValue("测试");
    fireEvent.click(
      screen.getByRole("button", { name: "settings.feedbackSubmit" }),
    );
    const confirm = await screen.findByRole("button", {
      name: "settings.feedbackConfirmSubmit",
    });
    fireEvent.click(confirm);
    fireEvent.click(confirm);
    await waitFor(() => expect(mocks.submit).toHaveBeenCalledOnce());
  });
});

describe("FeedbackDialog rendered confirmation boundary", () => {
  it("does not upgrade the old Submit action before confirmation renders", async () => {
    const pending = deferred<typeof prepared>();
    mocks.prepare.mockReturnValue(pending.promise);
    const { result } = renderHook(() => useFeedbackForm(true, vi.fn()));
    act(() => result.current.changeDescription("测试"));
    const initialSubmit = result.current.handlePrimaryAction;
    act(() => {
      void initialSubmit();
    });
    await act(async () => {
      pending.resolve(prepared);
      await Promise.resolve();
      void initialSubmit();
    });
    expect(mocks.submit).not.toHaveBeenCalled();
    expect(mocks.prepare).toHaveBeenCalledOnce();
    expect(result.current.preview?.previewId).toBe(prepared.previewId);
    await act(async () => {
      await result.current.handlePrimaryAction();
    });
    expect(mocks.submit).toHaveBeenCalledOnce();
  });

  it("does not let an older confirmation action submit a newer preview token", async () => {
    const { result } = renderHook(() => useFeedbackForm(true, vi.fn()));
    act(() => result.current.changeDescription("First draft"));
    await act(async () => {
      await result.current.handlePrimaryAction();
    });
    const oldConfirmation = result.current.handlePrimaryAction;
    act(() => result.current.changeDescription("Second draft"));
    mocks.prepare.mockResolvedValueOnce({
      ...prepared,
      previewId: "new-token",
    });
    await act(async () => {
      await result.current.handlePrimaryAction();
    });
    await act(async () => {
      await oldConfirmation();
    });
    expect(mocks.submit).not.toHaveBeenCalled();
    expect(result.current.preview?.previewId).toBe("new-token");
    await act(async () => {
      await result.current.handlePrimaryAction();
    });
    expect(mocks.submit).toHaveBeenCalledWith({ previewId: "new-token" });
  });

  it("shows contact transmission details only in the existing upload confirmation", async () => {
    mocks.prepare.mockResolvedValue({
      ...prepared,
      summary: { ...prepared.summary, contactIncluded: true },
    });
    render(<FeedbackDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(contactInput()).toBeEnabled());
    expect(
      screen.queryByText("settings.feedbackPreviewContactNotice"),
    ).not.toBeInTheDocument();
    fireEvent.change(contactInput(), {
      target: { value: "reply@example.invalid" },
    });
    const preview = await prepareDraft();
    expect(
      within(preview).getByText("settings.feedbackPreviewContactNotice"),
    ).toBeVisible();
  });
});

describe("FeedbackDialog local paste", () => {
  const field = () =>
    screen.getByRole("textbox", { name: "settings.feedbackDescriptionLabel" });
  const paste = (files: File[], text = "") =>
    fireEvent.paste(field(), {
      clipboardData: {
        files,
        items: files.map((file) => ({
          kind: "file",
          type: file.type,
          getAsFile: () => file,
        })),
        getData: () => text,
      },
    });
  it("attaches pasted images, preserves mixed text default behavior and does not upload", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    expect(
      paste(
        [new File(["synthetic"], "paste.png", { type: "image/png" })],
        "text",
      ),
    ).toBe(true);
    expect(await screen.findByRole("img", { name: "paste.png" })).toBeVisible();
    expect(mocks.readClipboardImage).not.toHaveBeenCalled();
    expect(mocks.prepare).not.toHaveBeenCalled();
    expect(mocks.submit).not.toHaveBeenCalled();
  });
  it("leaves plain text and non-image file pastes untouched", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    expect(paste([], "text")).toBe(true);
    expect(
      paste([new File(["text"], "text.txt", { type: "text/plain" })]),
    ).toBe(true);
    expect(screen.queryAllByRole("img")).toHaveLength(0);
    expect(mocks.readClipboardImage).not.toHaveBeenCalled();
  });
  it("uses the event files fallback and rejects oversized images before reading", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    const file = new File(["synthetic"], "fallback.png", { type: "image/png" });
    fireEvent.paste(field(), { clipboardData: { items: [], files: [file] } });
    expect(
      await screen.findByRole("img", { name: "fallback.png" }),
    ).toBeVisible();
    const oversized = new File(["synthetic"], "large.png", {
      type: "image/png",
    });
    Object.defineProperty(oversized, "size", { value: 5 * 1024 * 1024 + 1 });
    const read = vi.spyOn(FileReader.prototype, "readAsDataURL");
    paste([oversized]);
    expect(read).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("img", { name: "large.png" }),
    ).not.toBeInTheDocument();
    read.mockRestore();
  });

  it("bounds repeated pastes at six images", async () => {
    render(<FeedbackDialog open onClose={vi.fn()} />);
    for (let i = 0; i < 7; i++) {
      paste([new File(["synthetic"], `paste-${i}.png`, { type: "image/png" })]);
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 10));
      });
    }
    expect(screen.getAllByRole("img")).toHaveLength(6);
    expect(mocks.submit).not.toHaveBeenCalled();
  });
  it("ignores an in-flight paste after close and reopen and refuses duplicate reads", async () => {
    let reader: FileReader | undefined;
    const read = vi
      .spyOn(FileReader.prototype, "readAsDataURL")
      .mockImplementation(function (this: FileReader) {
        reader = this;
      });
    const { rerender } = render(<FeedbackDialog open onClose={vi.fn()} />);
    const file = new File(["synthetic"], "old.png", { type: "image/png" });
    paste([file]);
    paste([file]);
    expect(read).toHaveBeenCalledTimes(1);
    rerender(<FeedbackDialog open={false} onClose={vi.fn()} />);
    rerender(<FeedbackDialog open onClose={vi.fn()} />);
    Object.defineProperty(reader!, "result", {
      value: "data:image/png;base64,AAAA",
    });
    await act(async () =>
      reader!.onload?.call(
        reader!,
        new ProgressEvent("load") as ProgressEvent<FileReader>,
      ),
    );
    expect(
      screen.queryByRole("img", { name: "old.png" }),
    ).not.toBeInTheDocument();
    read.mockRestore();
  });
});

describe("FeedbackDialog background submission entry", () => {
  it("closes immediately, survives unmount, records the actual attempt and clears the completion dot on view", async () => {
    const pending = deferred<typeof prepared>();
    mocks.prepare.mockReturnValueOnce(pending.promise);
    const close = vi.fn();
    const { unmount } = render(
      <FeedbackDialog open backgroundSubmission onClose={close} />,
    );
    fireEvent.change(
      screen.getByRole("textbox", {
        name: "settings.feedbackDescriptionLabel",
      }),
      { target: { value: "Synthetic background report" } },
    );
    const submit = screen.getByRole("button", {
      name: "settings.feedbackSubmit",
    });
    fireEvent.click(submit);
    fireEvent.click(submit);
    expect(close).toHaveBeenCalledOnce();
    expect(mocks.prepare).toHaveBeenCalledOnce();
    expect(mocks.submit).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("region", { name: "settings.feedbackPreviewTitle" }),
    ).not.toBeInTheDocument();
    unmount();
    render(<FeedbackSubmissionIndicator />);
    expect(screen.getByRole("status")).toHaveTextContent(
      "settings.feedbackSubmitting",
    );
    await act(async () => pending.resolve(prepared));
    await waitFor(() =>
      expect(mocks.submit).toHaveBeenCalledExactlyOnceWith({
        previewId: prepared.previewId,
      }),
    );
    await screen.findByRole("status", {
      name: "settings.feedbackBackgroundUnread",
    });
    act(() => feedbackSubmission.markRead());
    expect(
      screen.queryByRole("status", {
        name: "settings.feedbackBackgroundUnread",
      }),
    ).not.toBeInTheDocument();
    render(<FeedbackDialog open backgroundSubmission onClose={vi.fn()} />);
    expect(screen.getByText("Synthetic background report")).toBeInTheDocument();
    expect(mocks.cancelPreview).not.toHaveBeenCalled();
  });
});
