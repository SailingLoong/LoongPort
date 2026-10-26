import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { zcodeDeviceApi, type ZCodeDeviceState } from "@/lib/api/zcodeDevices";

const nativeDevice = "native";
const maxFileBytes = 64 * 1024;
function readFile(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(new Error("file"));
    reader.readAsText(file);
  });
}

/** Local configuration only. Applying a new device context requires a restart. */
export function ZCodeDeviceManager({
  dataRoot,
  disabled = false,
}: {
  dataRoot?: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const copy = (
    key: string,
    fallback: string,
    values?: Record<string, string | number>,
  ) => t(`zcode.devices.${key}`, { defaultValue: fallback, ...values });
  const [state, setState] = useState<ZCodeDeviceState | null>(null);
  const [input, setInput] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  const epoch = useRef(0);
  const inFlight = useRef(false);
  const fileInput = useRef<HTMLInputElement>(null);
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
  return (
    <section className="space-y-3" aria-label={copy("title", "Device IDs")}>
      <h4 className="text-sm font-medium">{copy("title", "Device IDs")}</h4>
      <p className="text-xs text-muted-foreground">
        {copy(
          "description",
          "Import IDs from your ZCode installations. The selected ID is shared by sign-in, activity checks, balance queries and claims in this ZCode environment.",
        )}
      </p>
      <Textarea
        aria-label={copy("input", "Device IDs to import")}
        placeholder={copy("placeholder", "One UUID per line, or paste JSON")}
        value={input}
        onChange={(event) => setInput(event.target.value)}
        disabled={disabled || pending}
        rows={3}
      />
      <div className="flex flex-wrap gap-2">
        <input
          ref={fileInput}
          type="file"
          accept=".json,.txt,application/json,text/plain"
          className="hidden"
          aria-label={copy("file", "Device ID file")}
          disabled={disabled || pending}
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = "";
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
          }}
        />
        <Button
          variant="outline"
          size="sm"
          disabled={disabled || pending}
          onClick={() => fileInput.current?.click()}
        >
          {copy("chooseFile", "Choose file")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={blocked || !input.trim()}
          onClick={() => {
            if (!state) return;
            void perform(() =>
              zcodeDeviceApi.import(dataRoot, input, state.revision),
            );
          }}
        >
          {copy("import", "Import device IDs")}
        </Button>
      </div>
      {state && (
        <p className="text-xs text-muted-foreground">
          {copy("saved", "{{count}} device IDs saved", {
            count: state.devices.length,
          })}
        </p>
      )}
      <label className="block text-sm">
        {copy("selected", "Device ID for the next session")}
      </label>
      <Select
        value={state?.selectedDeviceMid ?? nativeDevice}
        disabled={blocked}
        onValueChange={(value) => {
          if (!state) return;
          void perform(() =>
            zcodeDeviceApi.select(
              dataRoot,
              value === nativeDevice ? null : value,
              state.revision,
            ),
          );
        }}
      >
        <SelectTrigger
          aria-label={copy("selected", "Device ID for the next session")}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={nativeDevice}>
            {copy("native", "Use local ZCode")}
          </SelectItem>
          {state?.devices.map((id) => (
            <SelectItem key={id} value={id}>
              {id}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {state && (
        <p className="break-all text-xs text-muted-foreground">
          {copy("current", "Current session: {{device}}", {
            device:
              state.effectiveDeviceMid ?? copy("native", "Use local ZCode"),
          })}
        </p>
      )}
      {state?.restartRequired && (
        <p role="status" className="text-sm text-amber-600">
          {copy("restart", "Restart LoongPort to use the selected device ID.")}
        </p>
      )}
      {error && (
        <div className="space-y-2">
          <p role="alert" className="text-sm text-destructive">
            {copy(
              "error",
              "Unable to update device IDs. Check the UUID format and try again.",
            )}
          </p>
          {!state && (
            <Button
              variant="outline"
              size="sm"
              disabled={disabled || pending}
              onClick={() => void perform(() => zcodeDeviceApi.state(dataRoot))}
            >
              {copy("retry", "Retry")}
            </Button>
          )}
        </div>
      )}
    </section>
  );
}
