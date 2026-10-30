import { useEffect, useRef, useState } from "react";
import { vendorApi } from "@/lib/api/vendor";
export function useOpenCodePersonalKeyViewModel(
  onSaved: (rowId: number) => void,
) {
  const [key, setKey] = useState("");
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const active = useRef(true);
  const pending = useRef(false);
  const generation = useRef(0);
  const secret = useRef("");
  useEffect(() => {
    active.current = true;
    setKey("");
    setBusy(pending.current);
    return () => {
      active.current = false;
      generation.current += 1;
      secret.current = "";
    };
  }, []);
  const ownerGeneration = generation.current;
  const save = async () => {
    if (
      !active.current ||
      ownerGeneration !== generation.current ||
      pending.current ||
      !secret.current.trim()
    )
      return;
    const request = ++generation.current;
    pending.current = true;
    setBusy(true);
    setMessage("");
    try {
      const result = await vendorApi.importOpenCodeKey(
        secret.current.trim(),
        label.trim(),
      );
      secret.current = "";
      if (!active.current || request !== generation.current) return;
      setKey("");
      setMessage(
        result.configurationReady === false
          ? "凭据已保存，应用配置未完成。请在账号行刷新重试；再次导入也不会新建重复账号。尚未验证真实调用。"
          : "已保存接入配置，尚未验证真实调用。请按已有权益选择 Console 或 Go。",
      );
      onSaved(result.rowId);
    } catch {
      if (active.current && request === generation.current)
        setMessage(
          "添加未完成，请检查完整密钥与本地密钥库后重试；不会自动创建密钥。",
        );
    } finally {
      pending.current = false;
      if (active.current) setBusy(false);
    }
  };
  const updateKey = (value: string) => {
    if (
      !active.current ||
      ownerGeneration !== generation.current ||
      pending.current ||
      value === secret.current
    )
      return;
    generation.current += 1;
    secret.current = value;
    setKey(value);
  };
  const updateLabel = (value: string) => {
    if (
      !active.current ||
      ownerGeneration !== generation.current ||
      pending.current ||
      value === label
    )
      return;
    generation.current += 1;
    setLabel(value);
  };
  return {
    key,
    setKey: updateKey,
    label,
    setLabel: updateLabel,
    busy,
    message,
    save,
  };
}
