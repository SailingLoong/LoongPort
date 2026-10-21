import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";

const keys = [
  "feedbackPrepare",
  "feedbackPasteHint",
  "feedbackBackgroundUnread",
  "feedbackBackgroundDetails",
  "feedbackBackgroundContinue",
  "feedbackBackgroundRetry",
  "feedbackCapabilitiesLoading",
  "feedbackCapabilitiesFailed",
  "feedbackCapabilitiesRetry",
  "feedbackPreparing",
  "feedbackPrepareFailed",
  "feedbackPreviewTitle",
  "feedbackPreviewHint",
  "feedbackPreviewFileBytes",
  "feedbackPreviewTotalBytes",
  "feedbackPreviewExpires",
  "feedbackPreviewApplicationConfigs",
  "feedbackPreviewNoFiles",
  "feedbackPreviewChanged",
  "feedbackPreviewUnavailable",
  "feedbackContactInvalid",
  "feedbackExtendedFieldsUnsupported",
  "feedbackClearOptionalFields",
  "feedbackRelatedApps",
  "feedbackConfirmSubmit",
  "feedbackOtherApp",
  "feedbackContactLabel",
  "feedbackContactHint",
  "feedbackPreviewContactNotice",
  "feedbackContactLength",
  "feedbackContactTooLong",
  "feedbackContactUnsupported",
  "feedbackSummaryContact",
  "feedbackSummaryNoContact",
  "feedbackCopyIdentity",
  "feedbackIdentityCopied",
  "feedbackIdentityCopyFailed",
  "feedbackUnsavedUpload",
  "feedbackIssueLabel",
  "feedbackIssueNumber",
  "feedbackLastReport",
  "feedbackReportId",
  "feedbackUploadedAt",
  "feedbackLocalPackage",
  "feedbackViewSummary",
  "feedbackExportLocalCopy",
  "feedbackDeleteLocalCopy",
  "feedbackDeleteLocalOnlyHint",
  "feedbackLocalMissing",
  "feedbackDiagnosticsNotAttached",
  "feedbackSummaryDescription",
  "feedbackSummaryNoDescription",
  "feedbackSummaryScreenshots",
  "feedbackSummaryDiagnostics",
  "feedbackSummaryScope",
  "feedbackUploadUnknown",
  "feedbackUploadFailed",
  "feedbackLocalSaveFailed",
  "feedbackReceiptUnavailable",
  "feedbackReceiptLoadFailed",
  "feedbackDeleteLocalFailed",
  "feedbackExportLocalFailed",
  "feedbackLocalDeleted",
  "feedbackLocalExported",
  "feedbackDiagnosticsDetails",
  "feedbackUserContentNotice",
] as const;

describe("feedback receipt translations", () => {
  it.each(Object.entries({ en, ja, zh, "zh-TW": zhTW }))(
    "provides the complete receipt and local-only deletion copy in %s",
    (_locale, resource) => {
      const settings = resource.settings;
      for (const key of keys)
        expect(settings[key], key).toEqual(expect.any(String));
      expect(settings.feedbackPasteHint).toContain("{{shortcut}}");
      expect(settings.feedbackIssueNumber).toContain("{{number}}");
      expect(settings.feedbackSummaryScreenshots).toContain("{{count}}");
      expect(settings.feedbackSummaryScreenshots).toContain("{{bytes}}");
      expect(settings.feedbackSummaryDiagnostics).toContain("{{bytes}}");
    },
  );
  it.each([
    [en, "After submission", "safe summary", "upload preview"],
    [ja, "送信後", "安全な要約", "送信プレビュー"],
    [zh, "提交后", "安全摘要", "上传预览"],
    [zhTW, "提交後", "安全摘要", "上傳預覽"],
  ] as const)(
    "describes post-submit safe summaries without a mandatory preview",
    (resource, afterSubmit, safeSummary, oldPreview) => {
      const copy = resource.settings.feedbackDiagnosticsDetails;
      expect(copy).toContain(afterSubmit);
      expect(copy).toContain(safeSummary);
      expect(copy).not.toContain(oldPreview);
    },
  );
});
