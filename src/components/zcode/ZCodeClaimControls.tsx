import { useZCodeClaimControls } from "./viewmodels/useZCodeClaimControls";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";

const reasonText = {
  openOfficialClient: "Open official ZCode, then check again",
  notDue: "Plan is still valid; check after expiry",
} as const;
const statusText = {
  unknown: "Unknown",
  claimable: "Claimable",
  claimed: "Claimed",
  noClaim: "No eligible plan",
  verificationRequired: "User verification required",
  loginExpired: "Login expired",
  resultPending: "Result pending",
  cancelled: "Cancelled",
};

/** Reads cached results only; official eligibility and scheduling belong to Rust. */
export function ZCodeClaimControls({
  dataRoot,
  profiles,
  disabled = false,
}: {
  dataRoot?: string;
  profiles: { id: string; label: string | null }[];
  disabled?: boolean;
}) {
  const {
    checkSelected,
    claimSelected,
    checkAccount,
    claimAccount,
    cancelClaims,
    applyAutomatic,
    copy,
    state,
    draftRevision,
    selectAccount,
    auto,
    setAuto,
    loaded,
    pending,
    error,
    ids,
    claimable,
    blocked,
    date,
  } = useZCodeClaimControls({ dataRoot, profiles, disabled });

  return (
    <section className="space-y-3" aria-label={copy("title", "Plan claims")}>
      <h4 className="text-sm font-medium">{copy("title", "Plan claims")}</h4>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled={blocked || ids.length === 0}
          onClick={() => void checkSelected()}
        >
          {copy("checkSelected", "Check selected")}
        </Button>
        <Button
          size="sm"
          disabled={blocked || claimable.length === 0}
          onClick={() => void claimSelected()}
        >
          {copy("claimSelected", "Claim selected")}
        </Button>
        {state.busy && (
          <>
            <span role="status" className="text-sm">
              {copy("busy", "Checking claims…")}
            </span>
            <Button
              variant="outline"
              size="sm"
              disabled={pending || disabled}
              onClick={() => void cancelClaims()}
            >
              {copy("cancel", "Cancel")}
            </Button>
          </>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-3">
        <label className="flex items-center gap-2 text-sm">
          <Checkbox
            checked={auto}
            disabled={blocked}
            onCheckedChange={setAuto}
          />
          {copy("auto", "Automatic claims")}
        </label>
        <Button
          variant="outline"
          size="sm"
          disabled={blocked || !draftRevision || (auto && ids.length === 0)}
          onClick={() => void applyAutomatic()}
        >
          {copy("applyAuto", "Apply automatic participation")}
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        {copy(
          "autoHelp",
          "Apply to save automatic claims for the selected accounts. Automatic claims are off by default.",
        )}
      </p>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {copy("error", "Claim status could not be updated. Try again.")}
        </p>
      )}
      {!loaded && !error && (
        <p role="status" className="text-sm">
          {copy("loading", "Loading claim status…")}
        </p>
      )}
      {profiles.map((profile) => {
        const record = state.records[profile.id];
        const label = profile.label ?? profile.id;
        const reason =
          record?.reason === "openOfficialClient" || record?.reason === "notDue"
            ? record.reason
            : null;
        return (
          <Card key={profile.id} role="group" aria-label={label}>
            <CardContent className="space-y-2 p-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <label className="flex min-w-0 items-center gap-2 text-sm">
                  <Checkbox
                    aria-label={copy("select", "Select {{account}}", {
                      account: label,
                    })}
                    checked={ids.includes(profile.id)}
                    disabled={blocked}
                    onCheckedChange={(checked) =>
                      selectAccount(profile.id, checked === true)
                    }
                  />
                  <span className="break-all">
                    {copy("select", "Select {{account}}", { account: label })}
                  </span>
                </label>
                <span className="text-sm">
                  {copy(
                    `status.${record?.status ?? "unknown"}`,
                    statusText[record?.status ?? "unknown"],
                  )}
                </span>
                <div className="flex gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={blocked}
                    onClick={() => void checkAccount(profile.id)}
                  >
                    {copy("check", "Check")}
                  </Button>
                  <Button
                    size="sm"
                    disabled={blocked || record?.canClaim !== true}
                    onClick={() => void claimAccount(profile.id)}
                  >
                    {copy("claim", "Claim")}
                  </Button>
                </div>
              </div>
              {reason && (
                <p className="text-xs text-muted-foreground">
                  {copy(`reason.${reason}`, reasonText[reason])}
                </p>
              )}
              {record && (
                <div className="space-y-1 text-xs text-muted-foreground">
                  <p>
                    {copy("plan", "Plan: {{value}}", {
                      value:
                        record.planName ??
                        record.planId ??
                        copy("unknown", "Unknown"),
                    })}
                  </p>
                  <p>
                    {copy("starts", "Starts: {{value}}", {
                      value: date(record.startsAt),
                    })}{" "}
                    ·{" "}
                    {copy("expires", "Expires: {{value}}", {
                      value: date(record.endsAt),
                    })}
                  </p>
                  <p>
                    {copy("checked", "Checked: {{value}}", {
                      value: date(record.checkedAt),
                    })}
                  </p>
                  {record.plans.map((plan) => (
                    <div key={plan.id} className="space-y-1">
                      <p>
                        {plan.name ?? plan.id} ·{" "}
                        {copy("units", "Units: {{value}}", {
                          value:
                            plan.units === null
                              ? copy("unknown", "Unknown")
                              : String(plan.units),
                        })}
                      </p>
                      {plan.grants.map((grant, index) => (
                        <p key={index}>
                          {grant.name} · {grant.period} ·{" "}
                          {grant.units === null
                            ? copy("unknown", "Unknown")
                            : grant.units}
                        </p>
                      ))}
                    </div>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>
        );
      })}
    </section>
  );
}
