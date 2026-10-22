import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { vendorApi, type OpenCodeLocalPreview } from "@/lib/api/vendor";

const errors: Record<string, string> = {
  source_locked: "OpenCode 数据库正忙，请稍后重新预览。没有保存任何凭据。",
  source_unavailable:
    "未找到可读取的 OpenCode V2 数据库，请先检查官方 OpenCode 的本机安装和数据库路径。不会读取旧 auth.json。",
  unsupported_schema: "当前 OpenCode 数据库格式不受支持，未导入任何凭据。",
  source_changed: "本机账号或凭据已变化，请重新预览并选择；没有保存旧凭据。",
  preview_expired: "预览已过期，请重新读取并确认。",
  preview_unavailable: "本次预览已取消或被替换，请重新读取并确认。",
  unsupported_credential:
    "所选凭据不支持安全导入，请使用完整的 Console API key。",
};

/** Only explicit clicks may read or save. No key ever crosses the IPC boundary. */
export function OpenCodeLocalImportForm({
  onSaved,
}: {
  onSaved: (rowId: number) => void;
}) {
  const [preview, setPreview] = useState<OpenCodeLocalPreview | null>(null);
  const [selected, setSelected] = useState("");
  const [busy, setBusy] = useState<"read" | "save" | null>(null);
  const [message, setMessage] = useState("");
  const generation = useRef(0);
  const alive = useRef(true);
  const pending = useRef(false);
  const operation = useRef<"read" | "save" | null>(null);
  const ticket = useRef<string | null>(null);
  const retire = (id: string | null) => {
    if (id) void vendorApi.cancelOpenCodeLocal(id).catch(() => {});
  };
  useEffect(() => {
    alive.current = true;
    // React Activity retains state while suspending effects. Never restore a
    // cancelled preview or retain a busy flag after its hidden request settled.
    setPreview(null);
    setSelected("");
    setBusy(operation.current);
    return () => {
      alive.current = false;
      generation.current += 1;
      retire(ticket.current);
      ticket.current = null;
    };
  }, []);
  const read = async () => {
    if (pending.current) return;
    pending.current = true;
    operation.current = "read";
    const request = ++generation.current;
    setBusy("read");
    setMessage("");
    setPreview(null);
    setSelected("");
    retire(ticket.current);
    ticket.current = null;
    try {
      const result = await vendorApi.previewOpenCodeLocal(true);
      if (!alive.current || generation.current !== request) {
        retire(result.previewId);
        return;
      }
      ticket.current = result.previewId;
      setPreview(result);
      setSelected(result.entries.length === 1 ? result.entries[0].id : "");
    } catch (error) {
      if (alive.current && generation.current === request)
        setMessage(
          errors[String(error)] ??
            "读取未完成，请检查本机 OpenCode 后重试。没有保存任何凭据。",
        );
    } finally {
      pending.current = false;
      operation.current = null;
      // No newer action can start while pending=true, so an older generation
      // may safely unlock a restored view without publishing its stale result.
      if (alive.current) setBusy(null);
    }
  };
  const cancel = () => {
    if (busy === "save") return;
    generation.current += 1;
    retire(ticket.current);
    ticket.current = null;
    setPreview(null);
    setSelected("");
    setMessage("已取消，没有保存凭据。");
    // A read already in flight must settle before another can start.
  };
  const save = async () => {
    if (pending.current || !preview || !selected) return;
    pending.current = true;
    operation.current = "save";
    const request = generation.current;
    setBusy("save");
    setMessage("");
    try {
      const result = await vendorApi.importOpenCodeLocal(
        preview.previewId,
        selected,
        true,
      );
      if (!alive.current || generation.current !== request) return;
      setMessage(
        result.configurationReady
          ? "已加密保存并添加接入配置，尚未验证真实调用。没有切换官方 OpenCode 当前账号，也不代表 OAuth 或原生积分池已接通。"
          : "凭据已保存，应用配置未完成。请在账号行刷新重试；再次导入也不会新建重复账号。尚未验证真实调用。",
      );
      onSaved(result.rowId);
    } catch (error) {
      if (alive.current && generation.current === request)
        setMessage(
          errors[String(error)] ??
            "加密保存未完成，请检查本地密钥库后重新预览。不会创建远端密钥。",
        );
    } finally {
      ticket.current = null;
      pending.current = false;
      operation.current = null;
      if (alive.current) setBusy(null);
      if (alive.current && generation.current === request) {
        setPreview(null);
        setSelected("");
      }
    }
  };
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
