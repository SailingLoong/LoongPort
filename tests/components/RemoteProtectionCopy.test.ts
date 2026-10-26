import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";

describe.each([en, zh, ja, zhTW])(
  "remote protection consequences",
  (locale) => {
    it("explains plaintext source recovery without inventing a missing source key", () => {
      const copy = locale.syncRestore as Record<string, string>;
      expect(copy.descriptionPlain).toMatch(/plaintext|明文|平文/);
      expect(copy.noSourceInput).toMatch(/no source key|没有|沒有|ありません/);
      expect(copy.failed).not.toMatch(
        /检查云端保护口令|檢查雲端保護密碼|check the cloud protection password/i,
      );
    });
    it("keeps plaintext remote credential consequences visible before manual or automatic upload", () => {
      for (const transport of [
        locale.settings.webdavSync,
        locale.settings.s3Sync,
      ]) {
        expect(transport.autoSyncHint).toMatch(/plaintext|明文|平文/);
        expect(transport.confirmUpload.warning).toMatch(
          /credentials|凭据|憑證|認証情報/,
        );
        expect(transport.confirmDownload.warning).toMatch(
          /protection|保护|保護/,
        );
        expect(transport.incompatibleVersion).not.toContain("v2 / db-v6");
      }
    });
  },
);

it.each([
  [zh, "保护方式只影响新备份；已有远端历史快照不会自动转换或删除。"],
  [zhTW, "保護方式只影響新備份；既有遠端歷史快照不會自動轉換或刪除。"],
  [
    en,
    "Protection changes affect new backups only; existing remote history is not automatically converted or deleted.",
  ],
  [
    ja,
    "保護方式の変更は新しいバックアップにのみ適用されます。既存のリモート履歴は自動変換・削除されません。",
  ],
] as const)(
  "does not imply existing remote history changes when local protection changes (%#)",
  (locale, boundary) => {
    for (const disclosure of [
      locale.settings.webdavSync.description,
      locale.settings.webdavSync.autoSyncHint,
      locale.settings.s3Sync.autoSyncHint,
      locale.settings.webdavSync.confirmUpload.warning,
      locale.settings.s3Sync.confirmUpload.warning,
    ]) {
      expect(disclosure).toContain(boundary);
    }
  },
);
