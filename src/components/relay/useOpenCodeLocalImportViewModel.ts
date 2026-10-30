import { useEffect, useRef, useState } from "react";
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
export function useOpenCodeLocalImportViewModel(
  onSaved: (rowId: number) => void,
) {
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
  const ownerGeneration = generation.current;
  const read = async () => {
    if (
      !alive.current ||
      ownerGeneration !== generation.current ||
      pending.current
    )
      return;
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
    if (
      !alive.current ||
      ownerGeneration !== generation.current ||
      operation.current === "save"
    )
      return;
    generation.current += 1;
    retire(ticket.current);
    ticket.current = null;
    setPreview(null);
    setSelected("");
    setMessage("已取消，没有保存凭据。");
    // A read already in flight must settle before another can start.
  };
  const save = async () => {
    if (
      !alive.current ||
      ownerGeneration !== generation.current ||
      pending.current ||
      !preview ||
      ticket.current !== preview.previewId ||
      !preview.entries.some((entry) => entry.id === selected)
    )
      return;
    pending.current = true;
    operation.current = "save";
    const request = ++generation.current;
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
  const select = (id: string) => {
    if (
      !alive.current ||
      ownerGeneration !== generation.current ||
      pending.current ||
      !preview ||
      ticket.current !== preview.previewId ||
      !preview.entries.some((entry) => entry.id === id) ||
      id === selected
    )
      return;
    generation.current += 1;
    setSelected(id);
  };
  return {
    preview,
    selected,
    setSelected: select,
    busy,
    message,
    read,
    cancel,
    save,
  };
}
