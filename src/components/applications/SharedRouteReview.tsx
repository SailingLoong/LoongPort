import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { RouteTarget, SharedGroup } from "@/lib/api/sharedRouting";
import type { ServiceSource } from "@/lib/api/serviceSources";

export function RouteTargetSummary({
  target,
  groups,
  sources,
  inlineModel = false,
  className,
}: {
  target: RouteTarget | null | undefined;
  groups: SharedGroup[];
  sources: ServiceSource[] | undefined;
  inlineModel?: boolean;
  className?: string;
}) {
  const { t } = useTranslation();
  if (target === undefined)
    return (
      <span className="text-muted-foreground">
        {t("routingWorkspace.savedStateUnknown")}
      </span>
    );
  if (target === null)
    return (
      <span className="text-muted-foreground">
        {t("routingWorkspace.noSavedTarget")}
      </span>
    );
  const source =
    target.kind === "manual"
      ? sources?.find((source) => source.sourceId === target.sourceId)
      : undefined;
  return (
    <div className={cn("min-w-0 text-sm", className)}>
      <p className="break-words font-semibold">
        <span>
          {t(
            target.kind === "manual"
              ? "routingWorkspace.manual"
              : "routingWorkspace.ordered",
          )}
        </span>
        {" · "}
        <span>
          {target.kind === "manual"
            ? (source?.name ?? target.sourceId)
            : (groups.find((group) => group.id === target.groupId)?.name ??
              target.groupId)}
        </span>
        {target.kind === "manual" && inlineModel && (
          <span className="font-normal text-muted-foreground">
            {" "}
            / <span className="break-all">{target.model}</span>
          </span>
        )}
      </p>
      {target.kind === "manual" && !inlineModel && (
        <p className="mt-1 break-all text-xs text-muted-foreground">
          {target.model}
        </p>
      )}
      {target.kind === "manual" && sources && !source && (
        <p className="mt-1 text-xs text-muted-foreground">
          {t("routingWorkspace.sourceUnknown")}
        </p>
      )}
    </div>
  );
}
export function SharedRouteReview({
  open,
  onClose,
  onConfirm,
  canConfirm,
  busy,
  saved,
  draft,
  groups,
  sources,
}: {
  open: boolean;
  onClose: () => void;
  onConfirm: () => void;
  canConfirm: boolean;
  busy: boolean;
  saved: RouteTarget | null | undefined;
  draft: RouteTarget | null;
  groups: SharedGroup[];
  sources: ServiceSource[] | undefined;
}) {
  const { t } = useTranslation();
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && !busy) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("routingWorkspace.reviewTitle")}</DialogTitle>
          <DialogDescription>
            {t("routingWorkspace.reviewHelp")}
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-4 sm:grid-cols-2">
          <section className="rounded-xl border p-4">
            <h3 className="mb-2 text-xs text-muted-foreground">
              {t("routingWorkspace.saved")}
            </h3>
            <RouteTargetSummary
              target={saved}
              groups={groups}
              sources={sources}
            />
          </section>
          <section className="rounded-xl border border-primary/30 bg-primary/5 p-4">
            <h3 className="mb-2 text-xs text-primary">
              {t("routingWorkspace.draft")}
            </h3>
            <RouteTargetSummary
              target={draft}
              groups={groups}
              sources={sources}
            />
          </section>
        </div>
        <p className="text-sm text-muted-foreground">
          {t("routingWorkspace.requestDisclaimer")}
        </p>
        {!canConfirm && !busy && (
          <p
            role="status"
            className="text-sm text-amber-700 dark:text-amber-400"
          >
            {t("routingWorkspace.reviewChanged")}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <Button variant="outline" disabled={busy} onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button disabled={!canConfirm || busy} onClick={onConfirm}>
            {t("routingWorkspace.confirm")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
