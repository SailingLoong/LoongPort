import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { vendorApi } from "@/lib/api/vendor";

/** Fixed official account source, not the custom-API setup screen. */
export function OpenCodePersonalKeyForm({
  onSaved,
}: {
  onSaved: (rowId: number) => void;
}) {
  const [key, setKey] = useState("");
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const active = useRef(true);
  const pending = useRef(false);
  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);
  const save = async () => {
    if (pending.current || !key.trim()) return;
    pending.current = true;
    setBusy(true);
    setMessage("");
    try {
      const result = await vendorApi.importOpenCodeKey(
        key.trim(),
        label.trim(),
      );
      if (!active.current) return;
      setKey("");
      setMessage(
        result.configurationReady === false
          ? "凭据已保存，应用配置未完成。请在账号行刷新重试；再次导入也不会新建重复账号。尚未验证真实调用。"
          : "已保存接入配置，尚未验证真实调用。请按已有权益选择 Console 或 Go。",
      );
      onSaved(result.rowId);
    } catch {
      if (active.current)
        setMessage(
          "添加未完成，请检查完整密钥与本地密钥库后重试；不会自动创建密钥。",
        );
    } finally {
      pending.current = false;
      if (active.current) setBusy(false);
    }
  };
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
