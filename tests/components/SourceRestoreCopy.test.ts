import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const expected = {
  zh: {
    protection:
      "输入用于验证云端快照来源；已选择的本机保护方式会保留。尚未选择保护方式的旧版配置仍按原恢复规则处理，成功恢复时会采用来源保护信息。",
    importNotice:
      "导入会替换配置，保留本机登录和同步凭据。当前保护方式和已有密钥保持不变。旧版未加密备份无需口令。",
  },
  "zh-TW": {
    protection:
      "輸入用於驗證雲端快照來源；已選擇的本機保護方式會保留。尚未選擇保護方式的舊版設定仍按原還原規則處理，成功還原時會採用來源保護資訊。",
    importNotice:
      "匯入會取代設定，保留本機登入與同步憑證。目前的保護方式和既有密鑰保持不變。舊版未加密備份無需密碼。",
  },
  en: {
    protection:
      "The input verifies the cloud snapshot’s source. A local protection mode you have already chosen is retained. Legacy configurations without a chosen mode still follow the original restore rules; a successful restore adopts the source’s protection information.",
    importNotice:
      "Import replaces configuration and retains local sign-in and sync credentials. The current protection mode and any existing key stay unchanged. Older unencrypted backups do not need a password.",
  },
  ja: {
    protection:
      "入力はクラウドスナップショットの復元元の検証に使用します。選択済みの端末の保護方式は保持されます。保護方式を未選択の旧版設定には従来の復元規則が適用され、復元に成功した場合は復元元の保護情報を採用します。",
    importNotice:
      "設定を置き換え、端末のログイン情報と同期認証情報を保持します。現在の保護方式と既存のキーは変更されません。以前の暗号化されていないバックアップにはパスワードは不要です。",
  },
};

describe("source recovery keeps native-owned target protection", () => {
  for (const [locale, copy] of Object.entries(expected)) {
    const data = JSON.parse(
      readFileSync(`${process.cwd()}/src/i18n/locales/${locale}.json`, "utf8"),
    );
    it(`${locale}: chosen target protection is retained; successful legacy restore uses source protection`, () => {
      expect(data.syncRestore.protection).toBe(copy.protection);
    });
    it(`${locale}: portable import does not imply a plaintext target has a key`, () => {
      expect(data.portableBackup.importNotice).toBe(copy.importNotice);
    });
  }
});
