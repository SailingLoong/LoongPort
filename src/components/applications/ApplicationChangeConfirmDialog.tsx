import { useTranslation } from "react-i18next";
import type { ApplicationConfiguration } from "@/lib/api/applicationOverview";
import type { ApplicationRoutingChange } from "@/lib/api/applicationRouting";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

export interface ApplicationChangeReview {
  change: ApplicationRoutingChange;
  appliedIds: string[];
  configurations: ApplicationConfiguration[];
  currentProviderId?: string;
  currentModel?: string | null;
  isCurrent: () => boolean;
}

/** Displays this request's intent; the routing owner still decides runtime facts. */
export function ApplicationChangeConfirmDialog({
  review,
  stale,
  disabled,
  onCancel,
  onConfirm,
}: {
  review: ApplicationChangeReview | null;
  stale: boolean;
  disabled: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  if (!review) return null;
  const label = (id?: string) => {
    const item = review.configurations.find((entry) => entry.providerId === id);
    return item
      ? [item.name, item.serviceName, item.accountLabel]
          .filter(Boolean)
          .join(" · ")
      : (id ?? "—");
  };
  const list = (key: string, ids: string[]) => (
    <section className="space-y-2">
      <h3 className="font-medium">{t(key)}</h3>
      <ol
        aria-label={t(key)}
        className="list-inside list-decimal space-y-1 break-words text-sm"
      >
        {ids.map((id) => (
          <li key={id}>{label(id)}</li>
        ))}
      </ol>
    </section>
  );
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("applications.reviewChanges")}</DialogTitle>
          <DialogDescription>
            {t("applications.reviewChangesDescription")}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4 overflow-y-auto px-6 py-5">
          {stale && (
            <p role="alert" className="text-sm text-destructive">
              {t("applications.reviewChanged")}
            </p>
          )}
          <p className="text-sm">
            {t("applications.orderProfiles")}:{" "}
            {review.change.order?.profileName}
          </p>
          {list("applications.appliedOrder", review.appliedIds)}
          {list(
            "applications.proposedOrder",
            review.change.order?.providerIds ?? [],
          )}
          <section className="space-y-2 text-sm">
            <h3 className="font-medium">
              {t("applications.currentConfiguration")}
            </h3>
            <p>{label(review.currentProviderId)}</p>
            <p>{review.currentModel ?? "—"}</p>
          </section>
          {review.change.selection ? (
            <section className="space-y-2 text-sm">
              <h3 className="font-medium">
                {t("applications.requestedSelection")}
              </h3>
              <p>{label(review.change.selection.providerId)}</p>
              <p>{review.change.selection.model ?? "—"}</p>
            </section>
          ) : (
            <p className="text-sm text-muted-foreground">
              {t("applications.noExplicitSelection")}
            </p>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button disabled={disabled || stale} onClick={onConfirm}>
            {t("common.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
