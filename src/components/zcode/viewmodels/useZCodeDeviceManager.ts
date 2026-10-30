import { useEffect, useRef, useState } from "react";
import { zcodeDeviceApi, type ZCodeDeviceState } from "@/lib/api/zcodeDevices";
const maxFileBytes = 64 * 1024;
function readFile(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(new Error("file"));
    reader.readAsText(file);
  });
}

export function useZCodeDeviceManager(dataRoot?: string, disabled = false) {
  const [state, setState] = useState<ZCodeDeviceState | null>(null);
  const [input, setInput] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  const epoch = useRef(0);
  const inFlight = useRef(false);
  useEffect(() => {
    const generation = ++epoch.current;
    setState(null);
    setInput("");
    setError(false);
    setPending(false);
    inFlight.current = false;
    void zcodeDeviceApi.state(dataRoot).then(
      (next) => {
        if (generation === epoch.current) {
          if (next) setState(next);
          else setError(true);
        }
      },
      () => {
        if (generation === epoch.current) setError(true);
      },
    );
    return () => {
      ++epoch.current;
    };
  }, [dataRoot]);
  const blocked = disabled || pending || !state;
  async function perform(operation: () => Promise<ZCodeDeviceState>) {
    if (inFlight.current) return;
    const generation = epoch.current;
    inFlight.current = true;
    setPending(true);
    setError(false);
    try {
      const next = await operation();
      if (generation === epoch.current) setState(next);
    } catch {
      if (generation === epoch.current) {
        setError(true);
        // Refresh the authoritative revision; preserve pasted text for retry.
        try {
          const next = await zcodeDeviceApi.state(dataRoot);
          if (generation === epoch.current) setState(next);
        } catch {
          // Keep the existing view and retry with a later explicit action.
        }
      }
    } finally {
      if (generation === epoch.current) {
        inFlight.current = false;
        setPending(false);
      }
    }
  }
  async function importDevices() {
    if (blocked || !input.trim() || !state) return;
    await perform(() => zcodeDeviceApi.import(dataRoot, input, state.revision));
  }
  async function selectDevice(value: string | null) {
    if (blocked || !state) return;
    await perform(() => zcodeDeviceApi.select(dataRoot, value, state.revision));
  }
  const chooseFile = (file: File | undefined) => {
    if (!file) return;
    const generation = epoch.current;
    if (file.size > maxFileBytes) {
      setError(true);
      return;
    }
    void readFile(file).then(
      (text) => {
        if (generation === epoch.current) {
          setInput(text);
          setError(false);
        }
      },
      () => {
        if (generation === epoch.current) setError(true);
      },
    );
  };
  return {
    state,
    input,
    setInput,
    pending,
    error,
    blocked,
    importDevices,
    selectDevice,
    chooseFile,
    retry: () => perform(() => zcodeDeviceApi.state(dataRoot)),
  };
}
