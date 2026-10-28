import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import type { CodexAccountProviderLink } from "./hooks/useCodexAccountProviderLinks";

interface CodexAccountNativeEntryProps {
  accountId: string;
  needsReauth: boolean;
  providers: CodexAccountProviderLink[];
  state: "loading" | "ready" | "error";
  active: boolean;
  onOpenCodex?: () => void;
  onRetry: () => void;
}

export function CodexAccountNativeEntry({
  accountId,
  needsReauth,
  providers,
  state,
  active,
  onOpenCodex,
  onRetry,
}: CodexAccountNativeEntryProps) {
  const { t } = useTranslation();
  const boundaryId = useId();
  const copy = (key: string, fallback: string) =>
    t(`codexOauth.nativeSwitchEntry.${key}`, { defaultValue: fallback });
  const title = copy("title", "Codex official client");
  return (
    <section
      aria-label={title}
      className="space-y-3 rounded-lg border border-border bg-card p-4"
    >
      <h4 className="text-sm font-medium">{title}</h4>
      <p className="break-all text-xs text-muted-foreground">{accountId}</p>
      <p id={boundaryId} className="text-xs text-muted-foreground">
        {copy(
          "boundary",
          "Direct native account switching is not available here. Set as Default only changes LoongPort's default managed-auth account; it does not switch the official client.",
        )}
      </p>
      {needsReauth && (
        <p className="text-xs text-amber-700 dark:text-amber-300">
          {copy(
            "reauth",
            "Renew authorization before using this account. Opening the application does not renew it.",
          )}
        </p>
      )}
      {state === "loading" && (
        <p role="status" className="text-sm">
          {copy("loading", "Loading linked Codex configurations…")}
        </p>
      )}
      {state === "error" && (
        <div className="space-y-2">
          <p role="alert" className="text-sm">
            {copy(
              "readError",
              "Unable to read linked configurations. This read does not switch accounts.",
            )}
          </p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!active}
            onClick={() => onRetry()}
          >
            {copy("retry", "Retry reading configurations")}
          </Button>
        </div>
      )}
      {state === "ready" &&
        (providers.length ? (
          <div className="space-y-2">
            <p className="text-xs font-medium">
              {copy("linkedTitle", "Linked Codex official configurations")}
            </p>
            <ul className="space-y-2">
              {providers.map((provider) => (
                <li
                  key={provider.id}
                  className="min-w-0 space-y-1 rounded-md border border-border p-2"
                >
                  <p className="break-words text-sm">{provider.name}</p>
                  <p className="break-all text-xs text-muted-foreground">
                    {provider.id}
                  </p>
                  {provider.presentation?.isCurrent && (
                    <Badge variant="secondary">
                      {copy(
                        "selectedConfiguration",
                        "Selected application configuration",
                      )}
                    </Badge>
                  )}
                </li>
              ))}
            </ul>
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">
            {copy(
              "empty",
              "No Codex official configuration explicitly bound to this account was found.",
            )}
          </p>
        ))}
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled
          aria-describedby={boundaryId}
        >
          {copy("nativeSwitch", "Switch official client account")}
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!active || !onOpenCodex}
          onClick={() => onOpenCodex?.()}
        >
          {copy("openApp", "Go to Codex application page")}
        </Button>
      </div>
      {!onOpenCodex && (
        <p className="text-xs text-muted-foreground">
          {copy(
            "navigationUnavailable",
            "This view has no application navigation entry.",
          )}
        </p>
      )}
      <p className="text-xs text-muted-foreground">
        {copy(
          "hint",
          "Choose the corresponding configuration on LoongPort's Codex application page and follow its existing switch confirmation. Configuration state does not verify the client's online identity.",
        )}
      </p>
    </section>
  );
}
