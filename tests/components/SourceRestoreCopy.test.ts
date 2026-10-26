import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const expected = {
  zh: {
    protection:
      "已选择的本机保护方式与密钥会保留；尚未选择保护方式的旧版配置按原恢复规则处理。需要输入时，仅用于验证本次恢复的来源。",
    importNotice:
      "导入会替换配置，保留本机登录和同步凭据。当前保护方式和已有密钥保持不变。旧版未加密备份无需口令。",
  },
  "zh-TW": {
    protection:
      "已選擇的本機保護方式與密鑰會保留；尚未選擇保護方式的舊版設定按原還原規則處理。需要輸入時，僅用於驗證本次還原的來源。",
    importNotice:
      "匯入會取代設定，保留本機登入與同步憑證。目前的保護方式和既有密鑰保持不變。舊版未加密備份無需密碼。",
  },
  en: {
    protection:
      "Your selected local protection mode and key are retained. Legacy configurations without a chosen mode follow the original restore rules. When input is required, it only verifies this restore’s source.",
    importNotice:
      "Import replaces configuration and retains local sign-in and sync credentials. The current protection mode and any existing key stay unchanged. Older unencrypted backups do not need a password.",
  },
  ja: {
    protection:
      "選択済みの端末の保護方式とキーは保持されます。保護方式を未選択の旧版設定には従来の復元規則が適用されます。入力が必要な場合は、今回の復元元の検証にのみ使用します。",
    importNotice:
      "設定を置き換え、端末のログイン情報と同期認証情報を保持します。現在の保護方式と既存のキーは変更されません。以前の暗号化されていないバックアップにはパスワードは不要です。",
  },
};

describe("source recovery keeps native-owned target protection", () => {
  for (const [locale, copy] of Object.entries(expected)) {
    const data = JSON.parse(
      readFileSync(`${process.cwd()}/src/i18n/locales/${locale}.json`, "utf8"),
    );
    it(`${locale}: chosen target protection is retained; legacy uses its original rules without assuming source adoption`, () => {
      expect(data.syncRestore.protection).toBe(copy.protection);
    });
    it(`${locale}: portable import does not imply a plaintext target has a key`, () => {
      expect(data.portableBackup.importNotice).toBe(copy.importNotice);
    });
  }
});
