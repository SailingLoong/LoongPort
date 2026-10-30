import { useRef } from "react";
import { useZCodeDeviceManager } from "./viewmodels/useZCodeDeviceManager";
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

const nativeDevice = "native";
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
  const fileInput = useRef<HTMLInputElement>(null);
  const {
    state,
    input,
    setInput,
    pending,
    error,
    blocked,
    importDevices,
    selectDevice,
    chooseFile,
    retry,
  } = useZCodeDeviceManager(dataRoot, disabled);
  return (
    <section className="space-y-3" aria-label={copy("title", "Device IDs")}>
      <h4 className="text-sm font-medium">{copy("title", "Device IDs")}</h4>
      <p className="text-xs text-muted-foreground">
        {copy(
          "description",
          "Import device IDs and choose one when authorizing each account. Unbound accounts use the default below.",
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
            chooseFile(file);
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
          onClick={() => void importDevices()}
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
        onValueChange={(value) =>
          void selectDevice(value === nativeDevice ? null : value)
        }
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
              onClick={() => void retry()}
            >
              {copy("retry", "Retry")}
            </Button>
          )}
        </div>
      )}
    </section>
  );
}
