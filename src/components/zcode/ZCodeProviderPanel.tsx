import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Plus, RefreshCw } from "lucide-react";
import {
  zcodeApi,
  type ZCodeConfig,
  type ZCodeProvider,
  type ZCodeProviderInput,
} from "@/lib/api/zcode";
import { extractErrorMessage } from "@/utils/errorUtils";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
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
import { ConfirmDialog } from "@/components/ConfirmDialog";

const queryKey = ["zcodeConfig"];
type Draft = Omit<ZCodeProviderInput, "models"> & { modelText: string };

/** Native file is authoritative. Only redacted read results enter the query cache. */
export function ZCodeProviderPanel({
  onNavigationBlockedChange,
}: {
  onNavigationBlockedChange?: (blocked: boolean) => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const query = useQuery({ queryKey, queryFn: zcodeApi.read, retry: false });
  const [draft, setDraft] = useState<Draft | null>(null);
  const [removing, setRemoving] = useState<{
    provider: ZCodeProvider;
    revision: string;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [conflict, setConflict] = useState(false);
  const [latest, setLatest] = useState<ZCodeConfig | null>(null);
  useEffect(() => {
    onNavigationBlockedChange?.(busy);
  }, [busy, onNavigationBlockedChange]);
  useEffect(
    () => () => onNavigationBlockedChange?.(false),
    [onNavigationBlockedChange],
  );
  const startEdit = (provider?: ZCodeProvider) => {
    if (!query.data || query.isError || inFlight.current) return;
    setError(null);
    setConflict(false);
    setLatest(null);
    setDraft({
      id: provider?.id ?? null,
      revision: query.data.revision,
      name: provider?.name ?? "",
      apiType: (provider?.apiType ?? "openai-responses") as Draft["apiType"],
      baseUrl: provider?.baseUrl ?? "",
      apiKey: "",
      modelText: provider?.models.join("\n") ?? "",
    });
  };
  const write = async (operation: () => Promise<ZCodeConfig>) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      await client.cancelQueries({ queryKey });
      const result = await operation();
      client.setQueryData(queryKey, result);
      setDraft(null);
      setRemoving(null);
    } catch (cause) {
      if (
        typeof cause === "object" &&
        cause !== null &&
        "code" in cause &&
        cause.code === "zcode.configuration_changed"
      ) {
        setConflict(true);
        setLatest(null);
        setError(
          t("zcode.conflict", {
            defaultValue:
              "ZCode configuration was changed elsewhere. Read the latest configuration before saving again.",
          }),
        );
      } else setError(extractErrorMessage(cause) || t("common.error"));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };
  const save = () => {
    if (!draft || conflict) return;
    const { modelText, ...input } = draft;
    void write(() =>
      zcodeApi.save({
        ...input,
        apiKey: input.apiKey?.trim() || null,
        models: modelText
          .split("\n")
          .map((model) => model.trim())
          .filter(Boolean),
      }),
    );
  };
  const close = () => {
    if (!inFlight.current) {
      setDraft(null);
      setError(null);
      setConflict(false);
      setLatest(null);
    }
  };
  const readLatest = async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      await client.cancelQueries({ queryKey });
      const result = await zcodeApi.read();
      client.setQueryData(queryKey, result);
      setLatest(result);
    } catch (cause) {
      setError(extractErrorMessage(cause) || t("common.error"));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };
  const external = latest?.providers.find(
    (provider) => provider.id === draft?.id,
  );
  const unavailable = Boolean(latest && draft?.id && !external?.managed);
  const resolveConflict = (keep: boolean) => {
    if (!draft || !latest || unavailable || inFlight.current) return;
    setDraft(
      keep || !external
        ? { ...draft, revision: latest.revision }
        : {
            id: external.id,
            revision: latest.revision,
            name: external.name,
            apiType: external.apiType as Draft["apiType"],
            baseUrl: external.baseUrl,
            apiKey: "",
            modelText: external.models.join("\n"),
          },
    );
    setConflict(false);
    setLatest(null);
    setError(null);
  };
  return (
    <section className="space-y-4">
      <div className="flex items-start justify-between gap-4">
        <div className="space-y-1">
          <h2 className="text-lg font-semibold">ZCode</h2>
          <p className="text-sm text-muted-foreground">
            {t("zcode.description", {
              defaultValue:
                "Configure personal providers and models in ZCode. Manage accounts and the default model in ZCode.",
            })}
          </p>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={busy || query.isFetching || !!draft || !!removing}
            onClick={() => {
              setError(null);
              void query.refetch();
            }}
          >
            <RefreshCw className="mr-2 h-4 w-4" />
            {t("zcode.refresh", { defaultValue: "Refresh" })}
          </Button>
          <Button
            size="sm"
            disabled={busy || !query.data || query.isError}
            onClick={() => startEdit()}
          >
            <Plus className="mr-2 h-4 w-4" />
            {t("zcode.add", { defaultValue: "Add provider" })}
          </Button>
        </div>
      </div>
      {query.isError && (
        <p role="alert" className="text-sm text-destructive">
          {extractErrorMessage(query.error)}
        </p>
      )}
      {error && !draft && !removing && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {query.isPending && (
        <p className="text-sm text-muted-foreground">{t("common.loading")}</p>
      )}
      {!query.isError && query.data?.providers.length === 0 && (
        <p className="text-sm text-muted-foreground">
          {t("zcode.empty", { defaultValue: "No personal providers yet" })}
        </p>
      )}
      {query.data?.providers.map((provider) => (
        <Card key={provider.id}>
          <CardContent className="flex items-start justify-between gap-4 p-4">
            <div className="min-w-0 space-y-1">
              <h3 className="font-medium">{provider.name}</h3>
              <p className="break-all text-sm text-muted-foreground">
                {provider.apiType} · {provider.baseUrl}
              </p>
              <p className="break-all text-sm text-muted-foreground">
                {provider.models.join(", ")}
              </p>
              {!provider.managed && (
                <p className="text-xs text-muted-foreground">
                  {t("zcode.native", { defaultValue: "Managed in ZCode" })}
                </p>
              )}
            </div>
            {provider.managed && (
              <div className="flex shrink-0 gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy || query.isError}
                  onClick={() => startEdit(provider)}
                >
                  {t("zcode.edit", { defaultValue: "Edit" })}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy || query.isError}
                  onClick={() => {
                    setError(null);
                    setRemoving({ provider, revision: query.data!.revision });
                  }}
                >
                  {t("zcode.remove", { defaultValue: "Remove" })}
                </Button>
              </div>
            )}
          </CardContent>
        </Card>
      ))}
      <Dialog
        open={draft !== null}
        onOpenChange={(open) => {
          if (!open) close();
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {t(draft?.id ? "zcode.edit" : "zcode.add", {
                defaultValue: draft?.id ? "Edit" : "Add provider",
              })}
            </DialogTitle>
            <DialogDescription>
              {t("zcode.keyHelp", {
                defaultValue:
                  "Keys are saved locally in ZCode. Leave the key blank to keep an existing key.",
              })}
            </DialogDescription>
          </DialogHeader>
          {draft && (
            <form
              className="min-h-0 space-y-4 overflow-y-auto"
              onSubmit={(event) => {
                event.preventDefault();
                save();
              }}
            >
              {(
                [
                  ["name", "Name", "text"],
                  ["baseUrl", "Base URL", "url"],
                  ["apiKey", "API Key", "password"],
                ] as const
              ).map(([field, label, type]) => (
                <div className="space-y-2" key={field}>
                  <Label htmlFor={`zcode-${field}`}>
                    {t(`zcode.${field}`, { defaultValue: label })}
                  </Label>
                  <Input
                    id={`zcode-${field}`}
                    type={type}
                    value={draft[field] ?? ""}
                    autoComplete="off"
                    disabled={busy}
                    required={field !== "apiKey" || !draft.id}
                    onChange={(event) =>
                      setDraft({ ...draft, [field]: event.target.value })
                    }
                  />
                </div>
              ))}
              <div className="space-y-2">
                <Label htmlFor="zcode-protocol">
                  {t("zcode.protocol", { defaultValue: "Protocol" })}
                </Label>
                <Select
                  value={draft.apiType}
                  disabled={busy}
                  onValueChange={(apiType) =>
                    setDraft({ ...draft, apiType: apiType as Draft["apiType"] })
                  }
                >
                  <SelectTrigger id="zcode-protocol">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="anthropic-messages">
                      Anthropic Messages
                    </SelectItem>
                    <SelectItem value="openai-chat-completions">
                      OpenAI Chat
                    </SelectItem>
                    <SelectItem value="openai-responses">
                      OpenAI Responses
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-2">
                <Label htmlFor="zcode-models">
                  {t("zcode.models", { defaultValue: "Models (one per line)" })}
                </Label>
                <Textarea
                  id="zcode-models"
                  value={draft.modelText}
                  required
                  disabled={busy}
                  onChange={(event) =>
                    setDraft({ ...draft, modelText: event.target.value })
                  }
                />
              </div>
              {error && (
                <p role="alert" className="text-sm text-destructive">
                  {error}
                </p>
              )}
              {conflict && (
                <div className="space-y-3 rounded-md border p-3 text-sm">
                  <Button
                    type="button"
                    variant="outline"
                    disabled={busy}
                    onClick={() => void readLatest()}
                  >
                    {t("zcode.readLatest", {
                      defaultValue: "Read latest configuration",
                    })}
                  </Button>
                  {unavailable ? (
                    <p role="alert">
                      {t("zcode.targetUnavailable", {
                        defaultValue:
                          "This provider was removed or is now managed in ZCode. Your input has not been saved.",
                      })}
                    </p>
                  ) : (
                    latest && (
                      <>
                        <table className="w-full table-fixed text-left text-xs">
                          <thead>
                            <tr>
                              <th>
                                {t("zcode.field", { defaultValue: "Field" })}
                              </th>
                              <th>
                                {t("zcode.myInput", {
                                  defaultValue: "My input",
                                })}
                              </th>
                              <th>
                                {t("zcode.externalValue", {
                                  defaultValue: "External value",
                                })}
                              </th>
                            </tr>
                          </thead>
                          <tbody>
                            {(
                              [
                                ["name", draft.name, external?.name],
                                ["baseUrl", draft.baseUrl, external?.baseUrl],
                                ["protocol", draft.apiType, external?.apiType],
                                [
                                  "models",
                                  draft.modelText,
                                  external?.models.join("\n"),
                                ],
                              ] as const
                            ).map(([field, mine, theirs]) => (
                              <tr key={field} className="align-top">
                                <th className="py-2">{t(`zcode.${field}`)}</th>
                                <td className="break-all whitespace-pre-wrap py-2">
                                  {mine || "—"}
                                </td>
                                <td className="break-all whitespace-pre-wrap py-2">
                                  {theirs || "—"}
                                </td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                        <p className="text-muted-foreground">
                          {t("zcode.keyNotCompared", {
                            defaultValue:
                              "Keys are not shown or compared. Keeping your input retains a key you entered; using external values clears it. Save again only after reviewing.",
                          })}
                        </p>
                        <div className="flex flex-wrap gap-2">
                          <Button
                            type="button"
                            disabled={busy}
                            onClick={() => resolveConflict(true)}
                          >
                            {t("zcode.keepInput", {
                              defaultValue: "Keep my input",
                            })}
                          </Button>
                          {external && (
                            <Button
                              type="button"
                              variant="outline"
                              disabled={busy}
                              onClick={() => resolveConflict(false)}
                            >
                              {t("zcode.useExternal", {
                                defaultValue: "Use external values",
                              })}
                            </Button>
                          )}
                        </div>
                      </>
                    )
                  )}
                </div>
              )}
              <DialogFooter>
                <Button
                  type="button"
                  variant="outline"
                  disabled={busy}
                  onClick={close}
                >
                  {t("zcode.cancel", { defaultValue: "Cancel" })}
                </Button>
                <Button type="submit" disabled={busy || conflict}>
                  {t("zcode.save", { defaultValue: "Save" })}
                </Button>
              </DialogFooter>
            </form>
          )}
        </DialogContent>
      </Dialog>
      <ConfirmDialog
        isOpen={removing !== null}
        pending={busy}
        title={t("zcode.remove", { defaultValue: "Remove" })}
        message={
          error ??
          t("zcode.removeConfirm", {
            name: removing?.provider.name,
            defaultValue: "Remove {{name}} from ZCode?",
          })
        }
        confirmText={t("zcode.remove", { defaultValue: "Remove" })}
        cancelText={t("zcode.cancel", { defaultValue: "Cancel" })}
        onCancel={() => {
          if (!inFlight.current) {
            setRemoving(null);
            setError(null);
          }
        }}
        onConfirm={() => {
          if (removing)
            void write(() =>
              zcodeApi.remove(removing.provider.id, removing.revision),
            );
        }}
      />
    </section>
  );
}
