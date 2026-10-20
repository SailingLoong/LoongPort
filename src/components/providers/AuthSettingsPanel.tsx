import { useEffect, useRef } from "react";
import type { ManagedAuthProvider } from "@/lib/api";
import { OPEN_ACCOUNT_MANAGER } from "@/components/relay/accounts/accountPlatforms";

/** Compatibility entry: open the one account owner view instead of mounting another manager. */
export function AuthSettingsPanel({
  target,
  onClose,
}: {
  target: ManagedAuthProvider | null;
  onClose: () => void;
}) {
  const sent = useRef<ManagedAuthProvider | null>(null);
  useEffect(() => {
    if (!target) {
      sent.current = null;
      return;
    }
    if (sent.current === target) return;
    sent.current = target;
    window.dispatchEvent(
      new CustomEvent(OPEN_ACCOUNT_MANAGER, { detail: target }),
    );
    onClose();
  }, [target, onClose]);
  return null;
}
