import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useOpenCodePersonalKeyViewModel } from "./useOpenCodePersonalKeyViewModel";
/** Fixed official account source, not the custom-API setup screen. */
export function OpenCodePersonalKeyForm({
  onSaved,
}: {
  onSaved: (rowId: number) => void;
}) {
  const { key, setKey, label, setLabel, busy, message, save } =
    useOpenCodePersonalKeyViewModel(onSaved);
  return (
    <section
      className="space-y-3 rounded-lg border border-border-default p-4"
      aria-label="OpenCode 个人密钥接入"
    >
      <p className="text-sm font-medium">OpenCode · Personal API key</p>
      <p className="text-xs text-muted-foreground">
        使用本人 workspace 已有 credits 或 Go 权益，只连接 OpenCode
        官方接口。无需 All 或 Billing 权限，Models Request 即可。余额、workspace
        身份和调用权限尚未验证。
      </p>
      <Input
        aria-label="显示名（可选）"
        placeholder="显示名（可选，不代表官网身份）"
        maxLength={100}
        value={label}
        disabled={busy}
        onChange={(e) => setLabel(e.target.value)}
      />
      <Input
        aria-label="OpenCode Personal API key"
        type="password"
        autoComplete="off"
        spellCheck={false}
        placeholder="oc_sk_… 完整密钥，不是列表前缀"
        value={key}
        disabled={busy}
        onChange={(e) => setKey(e.target.value)}
      />
      <p className="text-xs text-muted-foreground">
        保存仅写入本机加密密钥库，不会发起测试消费、订阅或创建密钥。需要新密钥时，请在官网
        Account → Personal API keys 自行确认。
      </p>
      <Button
        type="button"
        disabled={busy || !key.trim()}
        onClick={() => void save()}
      >
        同意加密保存并添加 OpenCode
      </Button>
      {message && (
        <p role="status" className="text-xs">
          {message}
        </p>
      )}
    </section>
  );
}
