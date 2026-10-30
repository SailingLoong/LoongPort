import { WorkBuddyNativeAttachment } from "./WorkBuddyNativeAttachment";
import { WorkBuddyNativePreview } from "./WorkBuddyNativePreview";
import { WorkBuddyNativeRecovery } from "./WorkBuddyNativeRecovery";
import { AutomaticClaimStatus } from "@/components/relay/accounts/AutomaticClaimStatus";
import {
  AccountWorkspace,
  AccountDetailSections,
} from "@/components/relay/accounts/AccountWorkspace";
import { useWorkBuddyAccountsViewModel } from "./useWorkBuddyAccountsViewModel";
import { useTranslation } from "react-i18next";
import { ChevronDown, Loader2, Plus, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
const amount = (value: number | null) =>
  value == null
    ? "—"
    : value.toLocaleString(undefined, { maximumFractionDigits: 2 });
const date = (value: number | null) =>
  value == null ? "—" : new Date(value).toLocaleString();
export function WorkBuddyAccounts({
  unified = false,
}: { unified?: boolean } = {}) {
  const { t } = useTranslation();
  const {
    accounts,
    loading,
    failed,
    busy,
    automaticFailed,
    automaticUncertain,
    login,
    waiting,
    inferenceLogin,
    inferenceTarget,
    inferenceBusy,
    inferenceWaiting,
    inferenceFailed,
    models,
    claimResults,
    automaticStatus,
    loadAccounts,
    beginLogin,
    finishLogin,
    closeLogin,
    beginInference,
    closeInference,
    disableInference,
    pollInference,
    discoverModels,
    setAutomatic,
    claimAll,
    canClaimAll,
    refreshAll,
    refreshAccount,
    claimAccount,
    openLogin,
    openInference,
    automaticDisabled,
    inferenceState,
    canDisableInference,
    canDiscoverModels,
  } = useWorkBuddyAccountsViewModel();
  return (
    <section aria-label={t("workbuddy.title")} className="space-y-3">
      <WorkBuddyNativePreview onCaptured={() => void loadAccounts(true)} />
      <WorkBuddyNativeRecovery />
      <header className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-medium">{t("workbuddy.title")}</h2>
        <div className="flex gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={busy !== null || !canClaimAll}
            onClick={() => void claimAll()}
          >
            {busy === "claimAll" && (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            )}
            {t(
              busy === "claimAll"
                ? "workbuddy.claimingAll"
                : "workbuddy.claimAll",
            )}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={busy !== null || loading || accounts.length === 0}
            onClick={() => void refreshAll()}
          >
            <RefreshCw className="h-3.5 w-3.5" />
            {t("workbuddy.refreshAll")}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={busy !== null}
            onClick={() => void beginLogin()}
          >
            <Plus className="h-3.5 w-3.5" />
            {t("workbuddy.add")}
          </Button>
        </div>
      </header>
      {loading && (
        <p role="status" className="text-sm text-muted-foreground">
          {t("common.loading")}
        </p>
      )}
      {failed && (
        <p role="alert" className="text-sm text-destructive">
          {t("workbuddy.failed")}
        </p>
      )}
      <p className="text-xs text-muted-foreground">
        {t("workbuddy.auto.notice")}
      </p>
      {automaticFailed && (
        <p role="alert" className="text-sm text-destructive">
          {t("workbuddy.auto.failed")}
        </p>
      )}
      {inferenceFailed && (
        <p role="alert" className="text-sm text-destructive">
          {t("workbuddy.inference.failed", {
            defaultValue:
              "Inference authorization or model discovery failed. Select the matching account and try again.",
          })}
        </p>
      )}
      {inferenceTarget && (inferenceLogin || inferenceBusy === "begin") && (
        <div
          role="region"
          aria-label={t("workbuddy.inference.authorization", {
            defaultValue: "Inference authorization",
          })}
          className="space-y-2 rounded-xl border p-3 text-sm"
        >
          <p>{accounts.find((row) => row.id === inferenceTarget)?.label}</p>
          <p>
            {t("workbuddy.inference.identityNotice", {
              defaultValue:
                "Authorize this same WorkBuddy account in the same region. The existing check-in login is retained.",
            })}
          </p>
          {inferenceLogin && (
            <>
              <a
                href={inferenceLogin.verificationUri}
                className="text-primary underline"
                onClick={(event) => {
                  event.preventDefault();
                  void openInference();
                }}
              >
                {t("workbuddy.inference.open", {
                  defaultValue: "Open inference authorization",
                })}
              </a>
              <Button
                size="sm"
                variant="outline"
                disabled={inferenceBusy !== null}
                onClick={() => void pollInference()}
              >
                {t("workbuddy.inference.finish", {
                  defaultValue: "I have authorized this account",
                })}
              </Button>
            </>
          )}
          <Button size="sm" variant="ghost" onClick={closeInference}>
            {t("workbuddy.inference.cancel", {
              defaultValue: "Cancel authorization",
            })}
          </Button>
          {inferenceWaiting && (
            <p role="status">{t("workbuddy.authorizationWaiting")}</p>
          )}
        </div>
      )}
      {(login || busy === "login") && (
        <Button size="sm" variant="ghost" onClick={closeLogin}>
          {t("common.cancel")}
        </Button>
      )}
      {login && (
        <div className="flex flex-wrap items-center gap-3 rounded-xl border border-border p-3 text-sm">
          <a
            className="text-primary underline"
            href={login.verificationUri}
            onClick={(event) => {
              event.preventDefault();
              void openLogin();
            }}
          >
            {t("workbuddy.authorize")}
          </a>
          <Button
            size="sm"
            variant="outline"
            disabled={busy !== null}
            onClick={() => void finishLogin()}
          >
            {t("workbuddy.finishAuthorization")}
          </Button>
          {waiting && (
            <span role="status">{t("workbuddy.authorizationWaiting")}</span>
          )}
        </div>
      )}
      {!loading && !failed && accounts.length === 0 && (
        <p className="rounded-xl border border-dashed p-4 text-sm text-muted-foreground">
          {t("workbuddy.empty")}
        </p>
      )}
      {claimResults !== null && (
        <div
          role="region"
          aria-label={t("workbuddy.claimResults")}
          className="rounded-xl border border-border p-3 text-sm"
        >
          <h3 className="font-medium">{t("workbuddy.claimResults")}</h3>
          {claimResults.length === 0 ? (
            <p role="status" className="mt-2 text-muted-foreground">
              {t("workbuddy.claimNone")}
            </p>
          ) : (
            <ul className="mt-2 space-y-2">
              {claimResults.map((result) => {
                const account = accounts.find((row) => row.id === result.id);
                return account ? (
                  <li
                    key={result.id}
                    className="flex flex-wrap justify-between gap-2"
                  >
                    <span>{account.label}</span>
                    <span
                      role="status"
                      className={
                        result.error
                          ? "text-destructive"
                          : "text-muted-foreground"
                      }
                    >
                      {result.error
                        ? t("workbuddy.claimFailed")
                        : t(`workbuddy.state.${account.claimState}`)}
                    </span>
                  </li>
                ) : null;
              })}
            </ul>
          )}
        </div>
      )}
      <div className="grid gap-2">
        {
          <AccountWorkspace
            accounts={accounts}
            label={(account) => account.label}
            enabled={unified}
          >
            {(account) => {
              const actions = (
                <div className="flex flex-wrap items-center justify-between gap-3">
                  {!unified && (
                    <h3 className="text-sm font-medium">{account.label}</h3>
                  )}
                  <div className="flex flex-wrap gap-2">
                    <Button
                      role={
                        automaticUncertain[account.id] ? "button" : "switch"
                      }
                      aria-label={t(
                        automaticUncertain[account.id]
                          ? "workbuddy.auto.unknown"
                          : "workbuddy.auto.label",
                      )}
                      aria-checked={
                        automaticUncertain[account.id]
                          ? undefined
                          : account.autoClaimEnabled === true
                      }
                      size="sm"
                      variant="outline"
                      disabled={automaticDisabled(account)}
                      onClick={() => void setAutomatic(account)}
                    >
                      {t(
                        automaticUncertain[account.id]
                          ? "workbuddy.auto.unknown"
                          : account.autoClaimEnabled
                            ? "workbuddy.auto.on"
                            : "workbuddy.auto.off",
                      )}
                    </Button>
                    {account.autoClaimBlocked && (
                      <span
                        role="status"
                        className="text-xs text-muted-foreground"
                      >
                        {t("workbuddy.auto.blocked")}
                      </span>
                    )}
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={busy !== null || !account.canRefresh}
                      onClick={() => void refreshAccount(account)}
                    >
                      {t("common.refresh")}
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={busy !== null || !account.canClaim}
                      onClick={() => void claimAccount(account)}
                    >
                      {t("workbuddy.claim")}
                    </Button>
                  </div>
                </div>
              );
              const inferencePaused = inferenceState(account.id) === "paused";
              const authorization = (
                <>
                  <WorkBuddyNativeAttachment
                    accountId={account.id}
                    onAttached={() => void loadAccounts(true)}
                  />
                  <p role="status">
                    {t(
                      `workbuddy.inference.state.${inferenceState(account.id)}`,
                      {
                        defaultValue: (
                          {
                            needsAuthorization:
                              "New inference authorization required",
                            ready: "Inference authorization available",
                            refreshRequired:
                              "Inference authorization needs refresh",
                            expired: "Inference authorization expired",
                            paused:
                              "WorkBuddy model requests are paused; account management and domestic check-in remain available",
                          } as const
                        )[inferenceState(account.id)],
                      },
                    )}
                  </p>
                  {!inferencePaused && (
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={
                        busy !== null ||
                        (inferenceTarget === account.id &&
                          inferenceBusy === "begin")
                      }
                      onClick={() => void beginInference(account.id)}
                    >
                      {t("workbuddy.inference.authorize", {
                        defaultValue: "Authorize inference",
                      })}
                    </Button>
                  )}
                  {canDisableInference(account.id) && (
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={busy !== null || inferenceBusy !== null}
                      onClick={() => void disableInference(account.id)}
                    >
                      {t("workbuddy.inference.disable", {
                        defaultValue: "Stop inference",
                      })}
                    </Button>
                  )}
                </>
              );
              const inference = inferencePaused ? null : (
                <>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={!canDiscoverModels(account.id)}
                    onClick={() => void discoverModels(account.id)}
                  >
                    {t("workbuddy.inference.models", {
                      defaultValue: "Discover inference models",
                    })}
                  </Button>
                  {models[account.id] && (
                    <ul
                      aria-label={t("workbuddy.inference.availableModels", {
                        defaultValue: "Available inference models",
                      })}
                    >
                      {models[account.id].map((model) => (
                        <li key={model.id}>
                          {model.name} · {model.id}
                        </li>
                      ))}
                    </ul>
                  )}
                </>
              );
              const credits = (
                <>
                  <AutomaticClaimStatus {...automaticStatus} />
                  <dl className="mt-3 flex flex-wrap gap-x-6 gap-y-2 text-xs text-muted-foreground">
                    <div>
                      <dt>{t("workbuddy.total")}</dt>
                      <dd
                        data-testid="workbuddy-total"
                        className="mt-1 tabular-nums text-foreground"
                      >
                        {amount(account.credits.totalRemaining)}
                      </dd>
                    </div>
                    <div>
                      <dt>{t("workbuddy.expiry")}</dt>
                      <dd className="mt-1">
                        {date(account.credits.nearestExpiry)}
                      </dd>
                    </div>
                    <div>
                      <dt>{t("workbuddy.updated")}</dt>
                      <dd className="mt-1">
                        {date(account.credits.updatedAt)}
                      </dd>
                    </div>
                  </dl>
                  <p
                    role="status"
                    className="mt-3 flex items-center gap-2 text-xs text-muted-foreground"
                  >
                    {busy === account.id && (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    )}
                    {t(`workbuddy.state.${account.claimState}`)}
                    {account.credited != null && (
                      <span>
                        {t("workbuddy.credited", {
                          amount: amount(account.credited),
                        })}
                      </span>
                    )}
                  </p>
                  {account.credits.packages.length > 0 && (
                    <Collapsible className="mt-2">
                      <CollapsibleTrigger asChild>
                        <Button
                          size="sm"
                          variant="ghost"
                          className="h-7 px-0 text-xs"
                        >
                          {t("workbuddy.packages")}
                          <ChevronDown className="h-3 w-3" />
                        </Button>
                      </CollapsibleTrigger>
                      <CollapsibleContent>
                        <ul className="divide-y divide-border/40">
                          {account.credits.packages.map((pack, index) => (
                            <li
                              key={`${pack.id}:${index}`}
                              className="flex flex-wrap justify-between gap-2 py-2 text-xs"
                            >
                              <span>{pack.name || t("workbuddy.package")}</span>
                              <span className="tabular-nums">
                                {t("workbuddy.remaining")}{" "}
                                {amount(pack.remaining)} ·{" "}
                                {t("workbuddy.expires")} {date(pack.expireAt)}
                              </span>
                            </li>
                          ))}
                        </ul>
                      </CollapsibleContent>
                    </Collapsible>
                  )}
                </>
              );
              if (unified)
                return (
                  <AccountDetailSections
                    authorization={authorization}
                    entitlements={
                      <>
                        {actions}
                        {credits}
                      </>
                    }
                    inference={
                      <>
                        {inference}
                        {!inferencePaused && (
                          <p className="text-xs text-muted-foreground">
                            {t("automaticStatus.inferenceUnverified")}
                          </p>
                        )}
                      </>
                    }
                  />
                );
              return (
                <article className="rounded-xl border border-border bg-card p-3">
                  {actions}
                  <div className="mt-3 space-y-2 border-t pt-3 text-xs">
                    {authorization}
                    {inference}
                  </div>
                  {credits}
                </article>
              );
            }}
          </AccountWorkspace>
        }
      </div>
    </section>
  );
}
