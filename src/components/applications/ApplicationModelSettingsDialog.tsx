import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";
import type { AppId } from "@/lib/api";
import type {
  ApplicationModelSettingsPatch,
  ReasoningIntent,
} from "@/lib/api/applicationModelSettings";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { ApplicationModelDeclarationEditor } from "./ApplicationModelDeclarationEditor";
import { useApplicationModelSettings } from "./useApplicationModelSettings";

interface Props {
  appId: AppId;
  providerId: string;
  name: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void | Promise<void>;
  onAdvanced: () => void;
  onBusyChange?: (busy: boolean) => void;
}
const intentValue = (intent: ReasoningIntent) =>
  intent.kind === "effort" ? `effort:${intent.level}` : intent.kind;
const parseIntent = (value: string): ReasoningIntent =>
  value.startsWith("effort:")
    ? { kind: "effort", level: value.slice(7) }
    : { kind: value as "serviceDefault" | "on" | "off" };

/** Presentation of the native resolver and the original provider edit protocol. */
export function ApplicationModelSettingsDialog({
  appId,
  providerId,
  name,
  open,
  onOpenChange,
  onSaved,
  onAdvanced,
  onBusyChange,
}: Props) {
  const { t } = useTranslation();
  const key = (suffix: string) => t(`applications.modelSettings.${suffix}`);
  const settings = useApplicationModelSettings(
    appId,
    providerId,
    open,
    async () => {
      try {
        await onSaved();
      } finally {
        toast.success(key("saved"));
        onOpenChange(false);
      }
    },
  );
  useEffect(() => {
    onBusyChange?.(settings.busy);
    return () => onBusyChange?.(false);
  }, [settings.busy, onBusyChange]);
  const [patch, setPatch] = useState<ApplicationModelSettingsPatch>({});
  const [customModel, setCustomModel] = useState(false);
  useEffect(() => {
    setPatch({});
    setCustomModel(false);
  }, [settings.data]);
  const data = settings.data;
  const selectedModel =
    patch.model?.action === "set"
      ? patch.model.value
      : data?.sourceChanged
        ? null
        : data?.selectedModel;
  const selected = data?.models.find((entry) => entry.model === selectedModel);
  const capability = selected?.capabilities;
  const setModel = (value: string) =>
    setPatch((previous) => {
      const next = { ...previous, model: { action: "set" as const, value } };
      delete next.declaration; // A declaration always belongs to the final selected row.
      return next;
    });
  const intent =
    patch.reasoning?.action === "set"
      ? patch.reasoning.value
      : (data?.reasoningIntent ?? { kind: "serviceDefault" as const });
  const reasoningValue = intentValue(intent);
  // Render the controls reported by the resolver. No provider/model-name heuristics.
  const reasoningOptions = [
    { value: "serviceDefault", label: key("serviceDefault") },
    ...(capability?.controls.on ? [{ value: "on", label: key("on") }] : []),
    ...(capability?.controls.off ? [{ value: "off", label: key("off") }] : []),
    ...(capability?.controls.effort
      ? capability.levels.map((level) => ({
          value: `effort:${level}`,
          label: level,
        }))
      : []),
  ];
  const preservedIntent = !reasoningOptions.some(
    (option) => option.value === reasoningValue,
  );
  const conflicts = [
    ...(data?.conflicts ?? []),
    ...(capability?.conflicts ?? []),
    ...settings.previewConflicts,
  ];
  const locked =
    settings.busy || Boolean(settings.review) || Boolean(settings.result);
  const disabled = locked || !data?.editable || Boolean(data.manualConfig);
  const sourceChangeIncomplete = Boolean(
    data?.sourceChanged &&
    (!data.sourceChangeEffects ||
      !patch.confirmSourceChange ||
      patch.sourceChangeRevision !== data.revision ||
      patch.model?.action !== "set" ||
      patch.reasoning?.action !== "set"),
  );
  const close = () => {
    if (!settings.busy) onOpenChange(false);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) close();
      }}
    >
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle className="break-words pr-7">
            {key("title")} · {name}
          </DialogTitle>
          <DialogDescription>{key("description")}</DialogDescription>
        </DialogHeader>
        <div className="min-h-0 overflow-y-auto px-6 py-5 space-y-5">
          {settings.loading && (
            <p
              role="status"
              className="flex items-center gap-2 text-sm text-muted-foreground"
            >
              <Loader2 className="h-4 w-4 animate-spin" />
              {t("common.loading")}
            </p>
          )}
          {settings.error && (
            <div
              role="alert"
              className="space-y-2 rounded-lg border border-destructive/30 p-3 text-sm"
            >
              <p>{key(settings.error)}</p>
              {(!data ||
                settings.error === "sourceChanged" ||
                settings.error === "sourceConfirmationRequired") && (
                <Button
                  variant="outline"
                  onClick={() => void settings.reload()}
                >
                  {t("common.retry")}
                </Button>
              )}
            </div>
          )}
          {data && (
            <div className="space-y-5">
              {data.manualConfig && (
                <p
                  role="alert"
                  className="rounded-lg border border-amber-500/30 p-3 text-sm"
                >
                  {key("manualConfig")}
                </p>
              )}
              {data.sourceChanged && (
                <div
                  role="alert"
                  className="space-y-3 rounded-lg border border-amber-500/30 p-3 text-sm"
                >
                  <p className="font-medium">{key("sourceChangeTitle")}</p>
                  <p>{key("sourceChangeDescription")}</p>
                  {Boolean(
                    data.sourceChangeEffects?.declarationModels.length,
                  ) && (
                    <div>
                      <p>{key("sourceChangeDeclarationFields")}</p>
                      <ul className="list-inside list-disc break-words">
                        {data.sourceChangeEffects?.declarationModels.map(
                          (model) => (
                            <li key={model}>{model}</li>
                          ),
                        )}
                      </ul>
                    </div>
                  )}
                  {data.sourceChangeEffects?.transport && (
                    <p>{key("sourceChangeTransport")}</p>
                  )}
                  <p>{key("sourceChangeInherited")}</p>
                  <p>{key("sourceChangePreserves")}</p>
                  {!data.sourceChangeEffects && (
                    <p>{key("sourceChangeEffectsUnavailable")}</p>
                  )}
                  <Label
                    className="flex items-start gap-2"
                    htmlFor="confirm-source-change"
                  >
                    <Checkbox
                      id="confirm-source-change"
                      className="mt-0.5 shrink-0"
                      checked={patch.confirmSourceChange === true}
                      disabled={disabled || !data.sourceChangeEffects}
                      onCheckedChange={(checked) =>
                        setPatch((previous) => {
                          const next = { ...previous };
                          if (checked) {
                            next.confirmSourceChange = true;
                            next.sourceChangeRevision = data.revision;
                          } else {
                            delete next.confirmSourceChange;
                            delete next.sourceChangeRevision;
                          }
                          return next;
                        })
                      }
                    />
                    <span>{key("confirmSourceChange")}</span>
                  </Label>
                </div>
              )}
              <div className="space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <Label htmlFor="application-model">{key("model")}</Label>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={
                      disabled ||
                      data.sourceChanged ||
                      !data.overrides.model ||
                      !data.canRestoreModel
                    }
                    onClick={() =>
                      setPatch((value) => {
                        const next = {
                          ...value,
                          model: { action: "restore" as const },
                        };
                        delete next.declaration;
                        return next;
                      })
                    }
                  >
                    {key("restoreModel")}
                  </Button>
                </div>
                <Select
                  value={
                    data.sourceChanged && patch.model?.action !== "set"
                      ? ""
                      : (selectedModel ?? "__service_default__")
                  }
                  disabled={disabled}
                  onValueChange={setModel}
                >
                  <SelectTrigger
                    id="application-model"
                    aria-label={key("model")}
                  >
                    <SelectValue
                      placeholder={key(
                        data.sourceChanged ? "chooseModel" : "serviceDefault",
                      )}
                    />
                  </SelectTrigger>
                  <SelectContent>
                    {selectedModel == null && (
                      <SelectItem value="__service_default__" disabled>
                        {key("serviceDefault")}
                      </SelectItem>
                    )}
                    {selectedModel &&
                      !data.models.some(
                        (entry) => entry.model === selectedModel,
                      ) && (
                        <SelectItem value={selectedModel} disabled>
                          {selectedModel}
                        </SelectItem>
                      )}
                    {data.models.map((entry) => (
                      <SelectItem key={entry.model} value={entry.model}>
                        {entry.model}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={disabled}
                  onClick={() => setCustomModel(!customModel)}
                >
                  {key("customModel")}
                </Button>
                {customModel && (
                  <Input
                    aria-label={key("customModel")}
                    value={selectedModel ?? ""}
                    disabled={disabled}
                    onChange={(event) => setModel(event.target.value)}
                  />
                )}
                {data.overrides.model && !data.canRestoreModel && (
                  <p className="text-xs text-muted-foreground">
                    {data.restoreUnavailableReason ?? key("restoreUnavailable")}
                  </p>
                )}
                {patch.model?.action === "restore" && (
                  <p className="text-xs text-amber-700 dark:text-amber-400">
                    {key("restorePending")}
                  </p>
                )}
              </div>
              <div className="space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <Label htmlFor="application-reasoning">
                    {key("reasoning")}
                  </Label>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={
                      disabled ||
                      data.sourceChanged ||
                      !data.overrides.reasoning ||
                      !data.canRestoreReasoning
                    }
                    onClick={() =>
                      setPatch((value) => ({
                        ...value,
                        reasoning: { action: "restore" },
                      }))
                    }
                  >
                    {key("restoreReasoning")}
                  </Button>
                </div>
                <Select
                  value={
                    data.sourceChanged && patch.reasoning?.action !== "set"
                      ? ""
                      : reasoningValue
                  }
                  disabled={disabled}
                  onValueChange={(value) =>
                    setPatch((previous) => ({
                      ...previous,
                      reasoning: { action: "set", value: parseIntent(value) },
                    }))
                  }
                >
                  <SelectTrigger
                    id="application-reasoning"
                    aria-label={key("reasoning")}
                  >
                    <SelectValue placeholder={key("chooseReasoning")} />
                  </SelectTrigger>
                  <SelectContent>
                    {reasoningOptions.map((option) => (
                      <SelectItem key={option.value} value={option.value}>
                        {option.label}
                      </SelectItem>
                    ))}
                    {preservedIntent && (
                      <SelectItem value={reasoningValue} disabled>
                        {intent.kind === "effort"
                          ? intent.level
                          : key(intent.kind)}
                      </SelectItem>
                    )}
                  </SelectContent>
                </Select>
                {data.overrides.reasoning && !data.canRestoreReasoning && (
                  <p className="text-xs text-muted-foreground">
                    {data.restoreUnavailableReason ?? key("restoreUnavailable")}
                  </p>
                )}
                {patch.reasoning?.action === "restore" && (
                  <p className="text-xs text-amber-700 dark:text-amber-400">
                    {key("restorePending")}
                  </p>
                )}
                <p className="text-xs text-muted-foreground">
                  {key(capability?.mode ?? "unknown")}
                </p>
                {capability && (
                  <p className="text-xs text-muted-foreground">
                    {key(`source.${capability.source}`)}
                  </p>
                )}
                {capability && !capability.verified && (
                  <p className="text-xs text-amber-700 dark:text-amber-400">
                    {key("unverified")}
                  </p>
                )}
                {capability && (
                  <p className="text-xs text-muted-foreground">
                    {key(`transport.${capability.transport.kind}`)}
                  </p>
                )}
                <p className="text-xs text-muted-foreground">
                  {key("precedence")}
                </p>
              </div>
              <ApplicationModelDeclarationEditor
                data={data}
                selectedModel={selectedModel}
                disabled={disabled || data.sourceChanged}
                patch={patch}
                onPatchChange={setPatch}
              />
              {conflicts.length > 0 && (
                <div
                  role="alert"
                  className="rounded-lg border border-destructive/30 p-3 text-sm"
                >
                  {conflicts.map((conflict, index) => (
                    <p key={index}>
                      {conflict === "applicationModelSourceChanged" ||
                      (data.sourceChanged && conflict === "sourceChanged")
                        ? key("sourceConfirmationRequired")
                        : conflict}
                    </p>
                  ))}
                </div>
              )}
              {data.restartRequired && (
                <p className="text-sm text-muted-foreground">
                  {key("restart")}
                </p>
              )}
              {settings.review && !settings.result && (
                <div
                  role="status"
                  className="space-y-2 rounded-lg border border-border bg-muted/30 p-3 text-sm"
                >
                  <p>{key("reviewDescription")}</p>
                  {settings.review.patch.confirmSourceChange && (
                    <p>{key("confirmSourceChange")}</p>
                  )}
                  <ul className="list-inside list-disc">
                    {settings.review.patch.model && (
                      <li>
                        {key("model")}:{" "}
                        {settings.review.patch.model.action === "restore"
                          ? key("restorePending")
                          : settings.review.patch.model.value}
                      </li>
                    )}
                    {settings.review.patch.reasoning && (
                      <li>
                        {key("reasoning")}:{" "}
                        {settings.review.patch.reasoning.action === "restore"
                          ? key("restorePending")
                          : settings.review.patch.reasoning.value.kind ===
                              "effort"
                            ? settings.review.patch.reasoning.value.level
                            : key(settings.review.patch.reasoning.value.kind)}
                      </li>
                    )}
                    {settings.review.patch.declaration && (
                      <li>
                        {key("declarationTitle")}:{" "}
                        {settings.review.patch.declaration.model} ·{" "}
                        {settings.review.patch.declaration.change.action ===
                        "restore"
                          ? key("restorePending")
                          : key(
                              settings.review.patch.declaration.change.value
                                .reasoningMode,
                            )}
                      </li>
                    )}
                    {settings.review.patch.transport && (
                      <li>
                        {key("endpointTemplate")}:{" "}
                        {settings.review.patch.transport.action === "restore"
                          ? key("restorePending")
                          : (data.templateOptions?.find(
                              (entry) =>
                                entry.id ===
                                (settings.review?.patch.transport?.action ===
                                "set"
                                  ? settings.review.patch.transport.value
                                      .templateId
                                  : ""),
                            )?.label ??
                            settings.review.patch.transport.value.templateId)}
                      </li>
                    )}
                  </ul>
                  <p>{key(settings.review.preview.action)}</p>
                </div>
              )}
              {settings.result && settings.result.status !== "completed" && (
                <div
                  role="alert"
                  className="space-y-2 rounded-lg border border-amber-500/30 p-3 text-sm"
                >
                  <p>
                    {t(`provider.preview.result.${settings.result.status}`)}
                  </p>
                  {settings.unresolved && <p>{key("unresolved")}</p>}
                  <Button
                    variant="outline"
                    disabled={settings.busy}
                    onClick={() => void settings.recheck()}
                  >
                    {key("recheck")}
                  </Button>
                  {settings.canReturnToEdit && (
                    <Button
                      variant="outline"
                      disabled={settings.busy}
                      onClick={settings.backToEdit}
                    >
                      {t("provider.preview.backToEdit")}
                    </Button>
                  )}
                </div>
              )}
            </div>
          )}
        </div>
        <DialogFooter className="gap-2 sm:justify-between">
          <Button
            variant="ghost"
            disabled={locked}
            onClick={() => {
              onOpenChange(false);
              onAdvanced();
            }}
          >
            {key("advanced")}
          </Button>
          <div className="flex gap-2">
            <Button
              variant="outline"
              disabled={settings.busy}
              onClick={
                settings.review && !settings.result
                  ? settings.cancelReview
                  : close
              }
            >
              {settings.review && !settings.result
                ? key("back")
                : t("common.cancel")}
            </Button>
            {settings.review && !settings.result ? (
              <Button
                disabled={settings.busy}
                onClick={() => void settings.confirm()}
              >
                {key("confirm")}
              </Button>
            ) : (
              <Button
                disabled={
                  disabled ||
                  sourceChangeIncomplete ||
                  !Object.keys(patch).length ||
                  (patch.model?.action === "set" && !patch.model.value.trim())
                }
                onClick={() => void settings.prepare(patch)}
              >
                {key("review")}
              </Button>
            )}
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
