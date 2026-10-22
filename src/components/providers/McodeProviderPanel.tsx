import { useEffect, useRef, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { mcodeApi, type McodeBackup } from "@/lib/api/mcode";
import { extractErrorMessage } from "@/utils/errorUtils";

/** Native configuration affordances only; providers remain owned by ProviderList. */
export function McodeProviderPanel({
  children,
  onBusyChange,
}: {
  children: (busy: boolean) => ReactNode;
  onBusyChange?: (busy: boolean) => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  // Provider mutations invalidate this prefix too, refreshing backup availability.
  const backup = useQuery({
    queryKey: ["providers", "mcode", "backup"],
    queryFn: mcodeApi.getBackup,
  });
  const [candidate, setCandidate] = useState<McodeBackup | null>(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState("");
  useEffect(() => {
    onBusyChange?.(busy);
    return () => onBusyChange?.(false);
  }, [busy, onBusyChange]);

  const restore = async () => {
    if (!candidate || inFlight.current || error) return;
    inFlight.current = true;
    setBusy(true);
    try {
      const restored = await mcodeApi.restoreBackup(candidate);
      if (!restored) throw new Error(t("mcode.restoreFailed"));
      setCandidate(null);
      await client.invalidateQueries({ queryKey: ["providers", "mcode"] });
      toast.success(t("mcode.restored"));
    } catch (cause) {
      setError(extractErrorMessage(cause) || t("mcode.restoreFailed"));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-muted-foreground">
          {t("mcode.nativeDefaultsHint")}
        </p>
        {backup.data && (
          <Button
            variant="outline"
            disabled={busy || backup.isFetching}
            onClick={() => {
              setError("");
              setCandidate(backup.data);
            }}
          >
            {t("mcode.restore")}
          </Button>
        )}
      </div>
      {(error || backup.error) && (
        <div role="alert" className="space-y-2 text-sm text-destructive">
          <p>
            {error || extractErrorMessage(backup.error)} {t("mcode.rereadHint")}
          </p>
          <Button
            variant="outline"
            disabled={busy || backup.isFetching}
            onClick={() => {
              void backup.refetch();
            }}
          >
            {t("common.refresh")}
          </Button>
        </div>
      )}
      {children(busy)}
      <ConfirmDialog
        isOpen={candidate !== null}
        title={t("mcode.restore")}
        message={[
          t("mcode.restoreConfirm"),
          error && `${error}\n${t("mcode.rereadHint")}`,
        ]
          .filter(Boolean)
          .join("\n\n")}
        pending={busy}
        confirmDisabled={Boolean(error)}
        onConfirm={() => {
          void restore();
        }}
        onCancel={() => {
          if (!inFlight.current) setCandidate(null);
        }}
      />
    </div>
  );
}
