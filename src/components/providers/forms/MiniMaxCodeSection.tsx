import { AutomaticClaimStatus } from "@/components/relay/accounts/AutomaticClaimStatus";
import {
  AccountWorkspace,
  AccountDetailSections,
} from "@/components/relay/accounts/AccountWorkspace";
import { useMiniMaxCodeViewModel } from "./useMiniMaxCodeViewModel";
import { useTranslation } from "react-i18next";
import { ExternalLink, Loader2, Plus, RefreshCw } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { MiniMaxCodeRegion } from "@/lib/api/minimaxCode";
import { MiniMaxNativeImport } from "./MiniMaxNativeImport";

const date = (value: number) => new Date(value).toLocaleString();
export function MiniMaxCodeSection({
  edition,
  unified = false,
}: { edition?: MiniMaxCodeRegion; unified?: boolean } = {}) {
  const { t } = useTranslation();
  const {
    visibleAccounts,
    loaded,
    loading,
    error,
    busy,
    region,
    login,
    loginBusy,
    waiting,
    automaticStatus,
    runAccount,
    beginLogin,
    cancelLogin,
    pollLogin,
    chooseRegion,
    reload,
    openLogin,
    toggleAutomatic,
  } = useMiniMaxCodeViewModel(edition);
  return (
    <div className="space-y-4">
      <p className="text-sm text-muted-foreground">
        {t("minimaxCode.description")}
      </p>
      <div className="flex flex-wrap items-end gap-2">
        {!edition && (
          <div className="min-w-40 space-y-2">
            <Label htmlFor="minimax-code-region">
              {t("minimaxCode.region")}
            </Label>
            <Select
              value={region}
              onValueChange={(value) =>
                chooseRegion(value as MiniMaxCodeRegion)
              }
              disabled={loginBusy || login !== null}
            >
              <SelectTrigger id="minimax-code-region">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="cn">
                  {t("minimaxCode.regions.cn")}
                </SelectItem>
                <SelectItem value="global">
                  {t("minimaxCode.regions.global")}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
        )}
        <Button
          type="button"
          variant="outline"
          disabled={!loaded || loading || loginBusy || login !== null}
          onClick={() => void beginLogin()}
        >
          {loginBusy && !login ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Plus className="mr-2 h-4 w-4" />
          )}
          {t("minimaxCode.add")}
        </Button>
        <MiniMaxNativeImport
          region={region}
          disabled={!loaded || loading || loginBusy || login !== null}
          onSaved={() => void reload()}
        />
        <Button
          type="button"
          variant="ghost"
          disabled={loading}
          onClick={() => void reload()}
        >
          <RefreshCw className="mr-2 h-4 w-4" />
          {t("minimaxCode.reload")}
        </Button>
      </div>
      {loading && (
        <p role="status" className="text-sm text-muted-foreground">
          {t("common.loading")}
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {t(`minimaxCode.errors.${error}`)}
        </p>
      )}
      {(login || loginBusy) && (
        <div className="space-y-3 rounded-lg border bg-muted/50 p-4">
          {login && (
            <>
              <p className="text-sm">{t("minimaxCode.enterCode")}</p>
              <code className="block font-mono text-2xl font-bold tracking-wider">
                {login.userCode}
              </code>
              <a
                href={login.verificationUri}
                className="inline-flex items-center gap-1 text-sm text-primary underline"
                onClick={(event) => {
                  event.preventDefault();
                  void openLogin();
                }}
              >
                {t("minimaxCode.authorize")}
                <ExternalLink className="h-3 w-3" />
              </a>
              <p className="text-xs text-muted-foreground">
                {t("minimaxCode.expiresAt", { time: date(login.expiresAt) })}
              </p>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={loginBusy || !loaded || loading}
                onClick={() => void pollLogin()}
              >
                {t("minimaxCode.finishAuthorization")}
              </Button>
            </>
          )}
          <Button
            type="button"
            size="sm"
            variant="ghost"
            onClick={() => void cancelLogin()}
          >
            {t("common.cancel", "取消")}
          </Button>
          {waiting && (
            <p role="status" className="text-sm text-muted-foreground">
              {t("minimaxCode.authorizationWaiting")}
            </p>
          )}
        </div>
      )}
      {loaded && !loading && visibleAccounts.length === 0 && (
        <p className="rounded-md border border-dashed p-4 text-sm text-muted-foreground">
          {t("minimaxCode.empty")}
        </p>
      )}
      <div className="space-y-2">
        {
          <AccountWorkspace
            accounts={visibleAccounts}
            label={(account) => account.label}
            identityDetails={(account) => (
              <p className="mt-1 break-all text-xs text-muted-foreground">
                {t(`minimaxCode.regions.${account.region}`)} ·{" "}
                {account.realUserId}
              </p>
            )}
            enabled={unified}
          >
            {(account) => {
              const identity = (
                <>
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <h5 className="text-sm font-medium">{account.label}</h5>
                    <Badge variant="secondary">
                      {t(`minimaxCode.status.${account.status}`)}
                    </Badge>
                  </div>
                  <p className="break-all text-xs text-muted-foreground">
                    {t(`minimaxCode.regions.${account.region}`)} ·{" "}
                    {account.realUserId}
                  </p>
                </>
              );
              const credentials = (
                <>
                  {account.credentialKind === "nativeAccessOnly" && (
                    <p className="text-xs text-muted-foreground">
                      {t("minimaxCode.credentials.accessOnly")}
                    </p>
                  )}
                  {account.credentialExpiresAt != null && (
                    <p className="text-xs text-muted-foreground">
                      {t("minimaxCode.credentials.expires", {
                        time: date(account.credentialExpiresAt),
                      })}
                    </p>
                  )}
                  {account.requiresReimport && (
                    <p role="status" className="text-xs text-amber-600">
                      {t("minimaxCode.credentials.reimport")}
                    </p>
                  )}
                </>
              );
              const credits = (
                <>
                  <AutomaticClaimStatus {...automaticStatus} />
                  <div className="text-xs text-muted-foreground">
                    <p>
                      {t("minimaxCode.entitlement.remaining", {
                        amount:
                          account.entitlement?.remainingCredits ??
                          t("minimaxCode.entitlement.unknown"),
                      })}
                    </p>
                    <p>
                      {t("minimaxCode.entitlement.plan", {
                        status: t(
                          account.entitlement?.hasTokenPlan === true
                            ? "minimaxCode.entitlement.active"
                            : account.entitlement?.hasTokenPlan === false
                              ? "minimaxCode.entitlement.inactive"
                              : "minimaxCode.entitlement.unknown",
                        ),
                      })}
                    </p>
                    {account.entitlement?.quotaState === "not-subscribed" ? (
                      <p>{t("minimaxCode.entitlement.quotaNotSubscribed")}</p>
                    ) : (
                      (["fiveHour", "weekly"] as const).map((kind) => {
                        const window =
                          account.entitlement?.quotaState === "available"
                            ? account.entitlement.quota?.[kind]
                            : undefined;
                        const remaining = window?.unlimited
                          ? t("minimaxCode.entitlement.quotaUnlimited")
                          : window?.remainingPercent != null
                            ? t("minimaxCode.entitlement.quotaPercent", {
                                percent: window.remainingPercent,
                              })
                            : t("minimaxCode.entitlement.unknown");
                        return (
                          <div key={kind}>
                            <p>
                              {t(
                                kind === "fiveHour"
                                  ? "minimaxCode.entitlement.quotaFiveHour"
                                  : "minimaxCode.entitlement.quotaWeekly",
                                { remaining },
                              )}
                            </p>
                            {window?.resetAtMs != null && (
                              <p>
                                {t("minimaxCode.entitlement.quotaReset", {
                                  time: date(window.resetAtMs),
                                })}
                              </p>
                            )}
                          </div>
                        );
                      })
                    )}
                    {account.entitlement?.tokenPlanExpiresAt != null && (
                      <p>
                        {t("minimaxCode.entitlement.expires", {
                          time: date(account.entitlement.tokenPlanExpiresAt),
                        })}
                      </p>
                    )}
                  </div>
                  {account.creditAmount !== null && (
                    <p className="text-sm">
                      {t("minimaxCode.creditAmount", {
                        amount: account.creditAmount,
                      })}
                    </p>
                  )}
                  {account.operation && (
                    <div className="space-y-1 rounded-md border p-2 text-sm">
                      <p>
                        {t(`minimaxCode.operation.${account.operation.status}`)}
                      </p>
                      <p>
                        {t("minimaxCode.startedAt", {
                          time: date(account.operation.startedAt),
                        })}
                      </p>
                      <p className="break-all text-xs text-muted-foreground">
                        {t("minimaxCode.attempt", {
                          id: account.operation.attemptId,
                        })}
                      </p>
                    </div>
                  )}
                  {account.status === "unknown" && (
                    <p className="text-sm text-amber-600">
                      {t("minimaxCode.unknownHint")}
                    </p>
                  )}
                  {account.lastCheckedAt !== null && (
                    <p className="text-xs text-muted-foreground">
                      {t("minimaxCode.lastCheckedAt", {
                        time: date(account.lastCheckedAt),
                      })}
                    </p>
                  )}
                  {account.lastObservedStatus !== null && (
                    <p className="text-xs text-muted-foreground">
                      {t(`minimaxCode.observed.${account.lastObservedStatus}`)}
                    </p>
                  )}
                </>
              );
              const creditActions = (
                <div className="flex flex-wrap gap-2">
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={
                      loading ||
                      busy.has(account.id) ||
                      (!account.authorized && !account.autoClaimEnabled)
                    }
                    onClick={() => void toggleAutomatic(account)}
                  >
                    {t(
                      account.autoClaimEnabled
                        ? "automaticStatus.disable"
                        : "automaticStatus.enable",
                    )}
                  </Button>
                  <p className="w-full text-xs text-muted-foreground">
                    {t(
                      account.autoClaimEnabled
                        ? "automaticStatus.enabled"
                        : "automaticStatus.disabled",
                    )}
                    {" · "}
                    {t("automaticStatus.notice")}
                  </p>
                  {account.autoClaimFailure && (
                    <p role="status" className="w-full text-xs text-amber-600">
                      {t("automaticStatus.actionRequired")}
                      {t(`minimaxCode.errors.${account.autoClaimFailure}`)}
                    </p>
                  )}
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={
                      loading || busy.has(account.id) || !account.canRefresh
                    }
                    onClick={() => void runAccount(account, "refresh")}
                  >
                    {t("minimaxCode.refresh")}
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={
                      loading || busy.has(account.id) || !account.canClaim
                    }
                    onClick={() => void runAccount(account, "claim")}
                  >
                    {t("minimaxCode.claim")}
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={
                      loading || busy.has(account.id) || !account.canRecheck
                    }
                    onClick={() => void runAccount(account, "recheck")}
                  >
                    {t("minimaxCode.recheck")}
                  </Button>
                </div>
              );
              const retireAction = (
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  disabled={
                    loading || busy.has(account.id) || !account.canRetire
                  }
                  onClick={() => void runAccount(account, "retire")}
                >
                  {t("minimaxCode.retire")}
                </Button>
              );
              if (unified)
                return (
                  <AccountDetailSections
                    authorization={
                      <>
                        <p role="status" className="text-sm">
                          {account.authorized
                            ? t("automaticStatus.authorizationSaved")
                            : t("automaticStatus.reauthorize")}
                        </p>
                        {credentials}
                        {retireAction}
                      </>
                    }
                    entitlements={
                      <>
                        <Badge variant="secondary">
                          {t(`minimaxCode.status.${account.status}`)}
                        </Badge>
                        {credits}
                        {creditActions}
                      </>
                    }
                    inference={
                      <p className="text-xs text-muted-foreground">
                        {t("automaticStatus.inferenceUnverified")}
                      </p>
                    }
                  />
                );
              return (
                <article
                  aria-label={account.label}
                  className="space-y-3 rounded-md border bg-muted/30 p-3"
                >
                  {identity}
                  {credentials}
                  {credits}
                  {creditActions}
                  {retireAction}
                </article>
              );
            }}
          </AccountWorkspace>
        }
      </div>
    </div>
  );
}
