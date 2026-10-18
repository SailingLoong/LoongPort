import { useState, type Dispatch, type SetStateAction } from "react";
import { useTranslation } from "react-i18next";
import type {
  ApplicationModelSettings,
  ApplicationModelSettingsPatch,
  ReasoningDeclaration,
  ReasoningMode,
} from "@/lib/api/applicationModelSettings";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { ReasoningLevelsEditor } from "@/components/providers/forms/CodexFormFields";

interface Props {
  data: ApplicationModelSettings;
  selectedModel: string | null | undefined;
  disabled: boolean;
  patch: ApplicationModelSettingsPatch;
  onPatchChange: Dispatch<SetStateAction<ApplicationModelSettingsPatch>>;
}

/** Edits original catalog fields and native-listed templates; never resolves capabilities. */
export function ApplicationModelDeclarationEditor({
  data,
  selectedModel,
  disabled,
  patch,
  onPatchChange,
}: Props) {
  const { t } = useTranslation();
  const key = (suffix: string) => t(`applications.modelSettings.${suffix}`);
  const [declarationOpen, setDeclarationOpen] = useState(false);
  const selected = data.models.find((entry) => entry.model === selectedModel);
  const declaration =
    patch.declaration?.change.action === "set"
      ? patch.declaration.change.value
      : selected?.declaration;
  const selectedTemplateId =
    patch.transport?.action === "set"
      ? patch.transport.value.templateId
      : data?.selectedTemplateId;
  const changeDeclaration = (update: Partial<ReasoningDeclaration>) => {
    if (!selectedModel?.trim()) return;
    onPatchChange((previous) => ({
      ...previous,
      declaration: {
        model: selectedModel.trim(),
        change: {
          action: "set",
          value: {
            ...(previous.declaration?.change.action === "set"
              ? previous.declaration.change.value
              : (selected?.declaration ?? {
                  reasoningMode: "unknown",
                  reasoningLevels: [],
                })),
            ...update,
          },
        },
      },
    }));
  };
  return (
    <Collapsible
      open={declarationOpen}
      onOpenChange={setDeclarationOpen}
      className="rounded-lg border border-border"
    >
      <CollapsibleTrigger asChild>
        <Button
          variant="ghost"
          className="h-auto w-full justify-start whitespace-normal py-2 text-left"
        >
          {key("declarationTitle")}
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent className="space-y-4 px-3 pb-3">
        <p className="text-xs text-muted-foreground">
          {key("declarationHint")}
        </p>
        <fieldset
          disabled={
            disabled ||
            !selectedModel?.trim() ||
            patch.model?.action === "restore"
          }
          className="space-y-3 disabled:opacity-60"
        >
          <div className="space-y-2">
            <Label htmlFor="model-declaration-mode">
              {key("declarationMode")}
            </Label>
            <Select
              value={declaration?.reasoningMode ?? "unknown"}
              disabled={
                disabled ||
                !selectedModel?.trim() ||
                patch.model?.action === "restore"
              }
              onValueChange={(value) =>
                changeDeclaration({
                  reasoningMode: value as ReasoningMode,
                })
              }
            >
              <SelectTrigger
                id="model-declaration-mode"
                aria-label={key("declarationMode")}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {(
                  [
                    "unknown",
                    "unsupported",
                    "toggle",
                    "required",
                    "supported",
                  ] as const
                ).map((mode) => (
                  <SelectItem key={mode} value={mode}>
                    {key(mode)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-2">
            <Label>{key("declarationLevels")}</Label>
            <ReasoningLevelsEditor
              allowAutomatic={false}
              ariaLabel={key("declarationLevels")}
              levels={declaration?.reasoningLevels ?? []}
              defaultLevel={declaration?.defaultReasoningLevel}
              onLevelsChange={(levels) =>
                changeDeclaration({ reasoningLevels: levels ?? [] })
              }
              onDefaultLevelChange={(level) =>
                changeDeclaration({ defaultReasoningLevel: level })
              }
            />
          </div>
          <Button
            size="sm"
            variant="outline"
            disabled={
              disabled ||
              !selected?.canRestoreDeclaration ||
              patch.model?.action === "restore"
            }
            onClick={() =>
              selectedModel &&
              onPatchChange((previous) => ({
                ...previous,
                declaration: {
                  model: selectedModel,
                  change: { action: "restore" },
                },
              }))
            }
          >
            {key("restoreDeclaration")}
          </Button>
          {selected?.restoreUnavailableReason && (
            <p className="text-xs text-muted-foreground">
              {selected.restoreUnavailableReason}
            </p>
          )}
          {patch.declaration?.change.action === "restore" && (
            <p className="text-xs text-amber-700 dark:text-amber-400">
              {key("restorePending")}
            </p>
          )}
        </fieldset>
        <div className="space-y-2">
          <Label htmlFor="endpoint-template">{key("endpointTemplate")}</Label>
          <Select
            value={selectedTemplateId ?? "__automatic__"}
            disabled={disabled || !data.templateOptions?.length}
            onValueChange={(templateId) =>
              onPatchChange((previous) => ({
                ...previous,
                transport: { action: "set", value: { templateId } },
              }))
            }
          >
            <SelectTrigger
              id="endpoint-template"
              aria-label={key("endpointTemplate")}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {!selectedTemplateId && (
                <SelectItem value="__automatic__" disabled>
                  {key("serviceDefault")}
                </SelectItem>
              )}
              {selectedTemplateId &&
                !data.templateOptions?.some(
                  (entry) => entry.id === selectedTemplateId,
                ) && (
                  <SelectItem value={selectedTemplateId} disabled>
                    {selectedTemplateId}
                  </SelectItem>
                )}
              {data.templateOptions?.map((entry) => (
                <SelectItem key={entry.id} value={entry.id}>
                  {entry.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <p className="text-xs text-muted-foreground">{key("templateHint")}</p>
          <Button
            size="sm"
            variant="outline"
            disabled={disabled || !data.canRestoreTransport}
            onClick={() =>
              onPatchChange((previous) => ({
                ...previous,
                transport: { action: "restore" },
              }))
            }
          >
            {key("restoreTransport")}
          </Button>
          {data.restoreUnavailableReason && (
            <p className="text-xs text-muted-foreground">
              {data.restoreUnavailableReason}
            </p>
          )}
          {patch.transport?.action === "restore" && (
            <p className="text-xs text-amber-700 dark:text-amber-400">
              {key("restorePending")}
            </p>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
