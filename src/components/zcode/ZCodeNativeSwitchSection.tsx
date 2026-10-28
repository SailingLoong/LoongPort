import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import type {
  CurrentNativeIdentity,
  SwitchOperationStatus,
} from "@/lib/api/zcodeAccounts";
export interface ZCodeNativeSwitchSectionProps {
  currentIdentity: CurrentNativeIdentity | null;
  identityReading: boolean;
  operation: SwitchOperationStatus | null;
  queryState: "idle" | "pending" | "ready" | "error" | "changed";
  operationPending: boolean;
  controlsDisabled: boolean;
  hasInspectedSource: boolean;
  onReadIdentity: () => void;
  onQueryOriginal: () => void;
  onViewRecovery: () => void;
}

/** Typed local evidence only; the parent owns every read and native action. */
export function ZCodeNativeSwitchSection({
  currentIdentity,
  identityReading,
  operation,
  queryState,
  operationPending,
  controlsDisabled,
  hasInspectedSource,
  onReadIdentity,
  onQueryOriginal,
  onViewRecovery,
}: ZCodeNativeSwitchSectionProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const unavailableId = useId();
  const copy = (
    key: string,
    defaultValue: string,
    values?: Record<string, string>,
  ) => t(`zcode.accounts.nativeSwitch.${key}`, { defaultValue, ...values });
  const reading = identityReading || queryState === "pending";
  const disabled = controlsDisabled || operationPending || reading;
  const phaseText = () => {
    if (!operation) return null;
    switch (operation.phase) {
      case "accepted":
        return copy(
          "accepted",
          "Original request accepted; completion is not confirmed.",
        );
      case "exited":
        return copy(
          "exited",
          "Client exit recorded; local write completion is not confirmed.",
        );
      case "transactionUncertain":
        return copy(
          "transactionUncertain",
          "The original write result is unknown. Query the original operation; do not switch again.",
        );
      case "committed":
        return operation.restartRequested
          ? copy(
              "committedRestartUnknown",
              "Local write committed; the requested restart result is still unknown.",
            )
          : copy(
              "committed",
              "Local write committed; no restart was requested.",
            );
      case "restartVerified":
        return copy(
          "restartVerified",
          "Local write committed and client process reopened. Account sign-in remains unverified.",
        );
      case "restartFailed":
        return copy(
          "restartFailed",
          "Local write committed; client reopening was not verified.",
        );
      case "failed":
        return copy(
          "failed",
          "The original operation ended without a confirmed commit. The previous identity is not verified.",
        );
    }
  };
  return (
    <section
      aria-labelledby={titleId}
      className="space-y-3 rounded-lg border p-4"
    >
      <h3 id={titleId} className="text-sm font-medium">
        {copy("title", "Official client")}
      </h3>
      <div className="space-y-1 text-sm">
        <p className="break-all">
          {currentIdentity?.id
            ? copy(
                "identity",
                "Local identity (last read): {{account}} · {{family}}",
                {
                  account: currentIdentity.label ?? currentIdentity.id,
                  family: currentIdentity.family ?? "",
                },
              )
            : copy("unknown", "Local identity: unknown")}
        </p>
        {currentIdentity && (
          <p className="text-xs text-muted-foreground">
            {copy("readAt", "Read at {{time}}", {
              time: new Date(currentIdentity.readAt).toISOString(),
            })}
          </p>
        )}
        <p className="text-xs text-muted-foreground">
          {copy(
            "localOnly",
            "A local read does not verify official-client sign-in or online availability.",
          )}
        </p>
      </div>
      <div role="status" aria-live="polite" className="space-y-1 text-sm">
        {operationPending ? (
          <p>{copy("pending", "Operation in progress. Do not repeat it.")}</p>
        ) : (
          <>
            {queryState === "pending" && (
              <p>{copy("querying", "Reading the original operation…")}</p>
            )}
            {queryState === "changed" && (
              <p>
                {copy(
                  "requestChanged",
                  "The original request changed. Earlier unresolved evidence is retained; verify the original operation before switching again.",
                )}
              </p>
            )}
            {queryState === "error" && (
              <p>
                {copy(
                  "queryError",
                  "Unable to refresh the original operation. Previous evidence is retained; the latest status is unconfirmed.",
                )}
              </p>
            )}
            {operation ? (
              <>
                <p className="break-all">
                  {copy(
                    "original",
                    "Original request: {{request}} · Target: {{target}}",
                    { request: operation.requestId, target: operation.target },
                  )}
                </p>
                <p>{phaseText()}</p>
                {operation.phase === "restartFailed" && (
                  <p className="text-xs text-muted-foreground">
                    {copy(
                      "restartHelp",
                      "Open the official client normally using the inspected source and verify it there. Opening the client manually does not clear the original operation or re-enable switching.",
                    )}
                  </p>
                )}
              </>
            ) : (
              queryState === "ready" && (
                <p>
                  {copy(
                    "noOperation",
                    "No confirmed original-operation record is available. This does not mean the switch succeeded.",
                  )}
                </p>
              )
            )}
          </>
        )}
      </div>
      {!hasInspectedSource && (
        <p className="text-xs text-muted-foreground">
          {copy(
            "inspectFirst",
            "Inspect the selected native source before reading identity or querying an operation.",
          )}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled={disabled || !hasInspectedSource}
          onClick={onReadIdentity}
        >
          {copy("read", "Read local identity")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={disabled || !hasInspectedSource}
          onClick={onQueryOriginal}
        >
          {copy("query", "Query original operation")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={disabled}
          onClick={onViewRecovery}
        >
          {copy("viewRecovery", "View recovery records")}
        </Button>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled
          aria-describedby={unavailableId}
        >
          {copy("restore", "Restore pre-switch data")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled
          aria-describedby={unavailableId}
        >
          {copy("retryStartup", "Retry startup")}
        </Button>
      </div>
      <p id={unavailableId} className="text-xs text-muted-foreground">
        {copy(
          "unavailable",
          "This version cannot automatically restore pre-switch data or retry startup separately. Review recovery records and verify the account in the official client.",
        )}
      </p>
    </section>
  );
}
