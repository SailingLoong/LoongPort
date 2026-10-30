import { Button } from "@/components/ui/button";
import { useOpenCodeLocalImportViewModel } from "./useOpenCodeLocalImportViewModel";
/** Only explicit clicks may read or save. No key ever crosses the IPC boundary. */
export function OpenCodeLocalImportForm({
  onSaved,
}: {
  onSaved: (rowId: number) => void;
}) {
  const { preview, selected, setSelected, busy, message, read, cancel, save } =
    useOpenCodeLocalImportViewModel(onSaved);
  return (
    <section
      className="space-y-3 rounded-lg border border-border-default p-4"
      aria-label="从本机 OpenCode 导入"
    >
      <p className="text-sm font-medium">从本机 OpenCode 导入</p>
      <p className="text-xs text-muted-foreground">
        仅在你同意后，只读本机 OpenCode V2 数据库中的 OpenCode API
        key，预览不会显示完整密钥。 选择并再次确认后，才加密保存到
        LoongPort。不会读取其他服务或 OAuth
        token，不会创建密钥、发起消费或更改官方当前登录。
      </p>
      <Button
        type="button"
        variant="outline"
        disabled={busy !== null}
        onClick={() => void read()}
      >
        同意只读预览本机 OpenCode 凭据
      </Button>
      {preview && (
        <div className="space-y-2">
          {preview.entries.length === 0 && (
            <p className="text-xs">
              未找到可导入的完整 Console API key。仅在官网或 CLI 通过 OAuth
              登录，不代表本机已有可导入 API key。
            </p>
          )}
          {preview.entries.map((entry) => (
            <label key={entry.id} className="flex items-center gap-2 text-sm">
              <input
                type="radio"
                name={`opencode-local-${preview.previewId}`}
                value={entry.id}
                checked={selected === entry.id}
                disabled={busy !== null}
                onChange={() => setSelected(entry.id)}
              />
              {entry.label}
              {entry.isActive ? "（官方当前选中）" : ""}
            </label>
          ))}
          {preview.notices.includes("oauth_not_imported") && (
            <p className="text-xs text-muted-foreground">
              OAuth 登录凭据未导入：其身份与安全续期尚未接通，过期或有效的 OAuth
              token 都不会复制。
            </p>
          )}
          {preview.notices.includes("unsupported_credential") && (
            <p className="text-xs text-muted-foreground">
              有凭据格式或来源不受支持，已安全跳过。不会猜测身份或权限。
            </p>
          )}
          {preview.entries.length > 0 && (
            <Button
              type="button"
              disabled={busy !== null || !selected}
              onClick={() => void save()}
            >
              同意加密保存所选 API key
            </Button>
          )}
          <Button
            type="button"
            variant="ghost"
            disabled={busy === "save"}
            onClick={cancel}
          >
            取消本次导入
          </Button>
        </div>
      )}
      {message && (
        <p role="status" className="text-xs">
          {message}
        </p>
      )}
    </section>
  );
}
