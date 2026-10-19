import { useRef, useState } from "react";
import { useProxyStatus } from "@/hooks/useProxyStatus";
import type { SettingsFormState } from "@/hooks/useSettings";

/** Owns the user command; persisted settings and runtime facts keep their owners. */
export function useLocalRoutingService(
  settings: SettingsFormState,
  onAutoSave: (updates: Partial<SettingsFormState>) => Promise<boolean | void>,
) {
  const { isRunning, startProxyServer, stopWithRestore, isPending } =
    useProxyStatus();
  const [showProxyConfirm, setShowProxyConfirm] = useState(false);
  const [commandPending, setCommandPending] = useState(false);
  const commandInFlight = useRef(false);

  const runCommand = async (command: () => Promise<void>) => {
    // The ref admits only one command even before React renders pending state.
    if (commandInFlight.current || isPending) return;
    commandInFlight.current = true;
    setCommandPending(true);
    try {
      await command();
    } catch (error) {
      // Save/runtime owners already present their errors; never retry implicitly.
      console.error("Proxy command failed:", error);
    } finally {
      commandInFlight.current = false;
      setCommandPending(false);
    }
  };

  const toggleProxy = async (checked: boolean) => {
    if (commandInFlight.current || isPending) return;
    if (checked && !settings?.proxyConfirmed) {
      setShowProxyConfirm(true);
      return;
    }
    return runCommand(async () => {
      if (checked) await startProxyServer();
      else await stopWithRestore();
    });
  };

  const confirmProxy = () =>
    runCommand(async () => {
      const saved = await onAutoSave({ proxyConfirmed: true });
      if (saved === false) return;
      try {
        await startProxyServer();
      } finally {
        // A start failure cannot undo a successfully persisted confirmation.
        setShowProxyConfirm(false);
      }
    });

  return {
    isRunning,
    isPending: commandPending || isPending,
    showProxyConfirm,
    toggleProxy,
    confirmProxy,
    cancelConfirmation: () => {
      if (!commandInFlight.current && !isPending) setShowProxyConfirm(false);
    },
  };
}
