import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { AppId } from "@/lib/api";
import type { ApplicationModeState } from "@/lib/api/applicationRouting";
import { startupUpgradeApi } from "@/lib/api/startupUpgrade";
import { StartupUpgrade } from "@/components/StartupUpgrade";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/** Fault-only access to the existing upgrade owner, not another recovery protocol. */
export function ApplicationStartupCheckpoint({
  appId,
  modeState,
  onRefresh,
}: {
  appId: AppId;
  modeState: ApplicationModeState | null | undefined;
  onRefresh: () => unknown;
}) {
  const { t } = useTranslation();
  const [review, setReview] = useState(false);
  const blockedReason = modeState?.blockedReason;
  const checkpointBlocked = blockedReason === "upgrade.checkpoint_pending";
  const upgrade = useQuery({
    queryKey: ["startupUpgrade", "applicationRouting", appId],
    queryFn: startupUpgradeApi.query,
    enabled: checkpointBlocked,
    retry: false,
  });
  const canReview =
    checkpointBlocked &&
    upgrade.isSuccess &&
    !upgrade.isFetching &&
    !upgrade.error &&
    upgrade.data.status === "database_verified" &&
    typeof upgrade.data.reviewToken === "string" &&
    upgrade.data.reviewToken.length > 0;
  useEffect(() => {
    setReview(false);
  }, [appId, blockedReason]);
  useEffect(() => {
    if (!canReview) setReview(false);
  }, [canReview]);
  const recheck = () => {
    // Inspection only: a lost/unknown outcome never replays a mutation.
    void Promise.allSettled([
      Promise.resolve().then(onRefresh),
      ...(checkpointBlocked ? [upgrade.refetch()] : []),
    ]);
  };
  if (!blockedReason?.startsWith("upgrade.")) return null;
  return (
    <div className="mt-3 space-y-3" data-testid="startup-checkpoint-fault">
      <p role="status" className="text-xs text-muted-foreground">
        {t(
          upgrade.error && checkpointBlocked
            ? "startupUpgrade.queryFailed"
            : blockedReason === "upgrade.source_changed"
              ? "codexSharedSettings.error.sourceChanged"
              : "startupUpgrade.verificationRequired",
        )}
      </p>
      <div className="flex flex-wrap gap-2">
        {canReview && (
          <Button size="sm" variant="outline" onClick={() => setReview(true)}>
            {t("startupUpgrade.reviewApps")}
          </Button>
        )}
        <Button
          size="sm"
          variant="outline"
          disabled={upgrade.isFetching}
          onClick={recheck}
        >
          {t("applications.recheckOperation")}
        </Button>
      </div>
      <Dialog
        open={review && canReview}
        onOpenChange={(open) => {
          setReview(open);
          if (!open) recheck();
        }}
      >
        <DialogContent className="max-h-[90vh] max-w-4xl overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{t("startupUpgrade.title")}</DialogTitle>
            <DialogDescription>
              {t("startupUpgrade.notComplete")}
            </DialogDescription>
          </DialogHeader>
          {review && canReview && <StartupUpgrade embedded />}
        </DialogContent>
      </Dialog>
    </div>
  );
}
