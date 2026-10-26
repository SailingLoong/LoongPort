import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ArrowDown, ArrowUp, Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { GroupDraft, PreparedGroup } from "@/lib/api/sharedRouting";
import type { ServiceSource } from "@/lib/api/serviceSources";
import { routeSelectClass } from "./ApplicationRouteWorkspace";

function models(source: ServiceSource | undefined) {
  return [
    ...new Set(
      (source?.account.kind === "localProvider"
        ? (source.supportedTargets ?? [])
        : (source?.supportedTargets ?? source?.bindings ?? [])
      )
        .filter(
          (fact) =>
            fact.canSelect === true &&
            typeof fact.model === "string" &&
            fact.model.length > 0,
        )
        .map((fact) => fact.model as string),
    ),
  ];
}
export function SharedGroupEditor({
  draft,
  sources,
  onChange,
  onClose,
  onPreview,
  onConfirm,
  onBack,
  prepared,
  busy,
  blocked,
  canConfirm,
  status,
}: {
  draft: GroupDraft | null;
  sources: ServiceSource[];
  onChange: (draft: GroupDraft) => void;
  onClose: () => void;
  onPreview: () => void;
  onConfirm: () => void;
  onBack: () => void;
  prepared: PreparedGroup | null;
  busy: boolean;
  blocked: boolean;
  canConfirm: boolean;
  status?: ReactNode;
}) {
  const { t } = useTranslation();
  const move = (index: number, offset: number) => {
    if (!draft) return;
    const members = [...draft.members];
    [members[index], members[index + offset]] = [
      members[index + offset],
      members[index],
    ];
    onChange({ ...draft, members });
  };
  const update = (index: number, sourceId: string, model: string) => {
    if (draft)
      onChange({
        ...draft,
        members: draft.members.map((member, i) =>
          i === index ? { sourceId, model } : member,
        ),
      });
  };
  return (
    <Dialog
      open={draft !== null}
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>
            {t(
              prepared
                ? "sharedGroups.reviewTitle"
                : draft?.id
                  ? "sharedGroups.edit"
                  : "sharedGroups.create",
            )}
          </DialogTitle>
          <DialogDescription>
            {t(
              prepared ? "sharedGroups.reviewHelp" : "sharedGroups.editorHelp",
            )}
          </DialogDescription>
        </DialogHeader>
        {status}
        {draft && (
          <>
            <fieldset
              disabled={busy || blocked || Boolean(prepared)}
              className="space-y-4"
            >
              <div>
                <label
                  htmlFor="shared-group-name"
                  className="text-sm font-medium"
                >
                  {t("sharedGroups.name")}
                </label>
                <Input
                  id="shared-group-name"
                  className="mt-2"
                  value={draft.name}
                  onChange={(event) =>
                    onChange({ ...draft, name: event.target.value })
                  }
                />
              </div>
              <ol className="space-y-3">
                {draft.members.map((member, index) => {
                  const source = sources.find(
                      (source) => source.sourceId === member.sourceId,
                    ),
                    options = models(source);
                  return (
                    <li
                      key={index}
                      className="rounded-xl border border-border-default p-3"
                    >
                      <div className="mb-2 flex items-center justify-between gap-2">
                        <span className="text-xs text-muted-foreground">
                          {index + 1}
                        </span>
                        <div className="flex gap-1">
                          <Button
                            variant="ghost"
                            size="icon"
                            className="h-7 w-7"
                            disabled={
                              index === 0 ||
                              busy ||
                              blocked ||
                              Boolean(prepared)
                            }
                            aria-label={`${t("sharedGroups.moveUp")} ${index + 1}`}
                            onClick={() => move(index, -1)}
                          >
                            <ArrowUp className="h-3.5 w-3.5" />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="h-7 w-7"
                            disabled={
                              index === draft.members.length - 1 ||
                              busy ||
                              blocked ||
                              Boolean(prepared)
                            }
                            aria-label={`${t("sharedGroups.moveDown")} ${index + 1}`}
                            onClick={() => move(index, 1)}
                          >
                            <ArrowDown className="h-3.5 w-3.5" />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="h-7 w-7"
                            aria-label={`${t("sharedGroups.removeMember")} ${index + 1}`}
                            onClick={() =>
                              onChange({
                                ...draft,
                                members: draft.members.filter(
                                  (_, i) => i !== index,
                                ),
                              })
                            }
                          >
                            <X className="h-3.5 w-3.5" />
                          </Button>
                        </div>
                      </div>
                      <div className="grid gap-3 sm:grid-cols-2">
                        <label className="text-xs text-muted-foreground">
                          {t("routingWorkspace.source")}
                          <select
                            className={routeSelectClass}
                            aria-label={`${t("sharedGroups.memberSource")} ${index + 1}`}
                            value={member.sourceId}
                            onChange={(event) =>
                              update(index, event.target.value, "")
                            }
                          >
                            <option value="">
                              {t("routingWorkspace.chooseSource")}
                            </option>
                            {member.sourceId && !source && (
                              <option value={member.sourceId} disabled>
                                {member.sourceId}
                              </option>
                            )}
                            {sources.map((item) => (
                              <option key={item.sourceId} value={item.sourceId}>
                                {item.name}
                              </option>
                            ))}
                          </select>
                        </label>
                        <label className="text-xs text-muted-foreground">
                          {t("routingWorkspace.model")}
                          <select
                            className={routeSelectClass}
                            aria-label={`${t("sharedGroups.memberModel")} ${index + 1}`}
                            value={member.model}
                            onChange={(event) =>
                              update(index, member.sourceId, event.target.value)
                            }
                          >
                            <option value="">
                              {t("routingWorkspace.chooseModel")}
                            </option>
                            {member.model &&
                              !options.includes(member.model) && (
                                <option value={member.model} disabled>
                                  {member.model}
                                </option>
                              )}
                            {options.map((model) => (
                              <option key={model} value={model}>
                                {model}
                              </option>
                            ))}
                          </select>
                        </label>
                      </div>
                      {!source && member.sourceId && (
                        <p className="mt-2 text-xs text-muted-foreground">
                          {t("routingWorkspace.sourceUnknown")}
                        </p>
                      )}
                    </li>
                  );
                })}
              </ol>
              <Button
                variant="outline"
                size="sm"
                onClick={() =>
                  onChange({
                    ...draft,
                    members: [...draft.members, { sourceId: "", model: "" }],
                  })
                }
              >
                <Plus className="mr-2 h-4 w-4" />
                {t("sharedGroups.addMember")}
              </Button>
            </fieldset>
            {prepared && (
              <section className="rounded-xl border border-amber-500/25 bg-amber-500/5 p-4">
                <h3 className="text-sm font-medium">
                  {t("sharedGroups.affectedApps")}
                </h3>
                <p
                  data-testid="group-affected-apps"
                  className="mt-2 text-sm text-muted-foreground"
                >
                  {prepared.affectedAppIds.length
                    ? prepared.affectedAppIds
                        .map((app) => t(`apps.${app}`))
                        .join(" · ")
                    : t("sharedGroups.noAffectedApps")}
                </p>
                <p className="mt-2 text-xs text-muted-foreground">
                  {t("sharedGroups.notRequestProof")}
                </p>
              </section>
            )}
            <div className="flex flex-wrap justify-end gap-2">
              <Button
                variant="outline"
                disabled={busy}
                onClick={prepared ? onBack : onClose}
              >
                {t(prepared ? "common.back" : "common.cancel")}
              </Button>
              {prepared ? (
                <Button
                  disabled={!canConfirm || busy || blocked}
                  onClick={onConfirm}
                >
                  {t("sharedGroups.confirm")}
                </Button>
              ) : (
                <Button
                  disabled={
                    busy ||
                    blocked ||
                    !draft.name ||
                    draft.members.length === 0 ||
                    draft.members.some(
                      (member) => !member.sourceId || !member.model,
                    )
                  }
                  onClick={onPreview}
                >
                  {t("sharedGroups.preview")}
                </Button>
              )}
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
