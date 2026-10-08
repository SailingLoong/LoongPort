import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { zcodeClaimApi, type ClaimState } from "@/lib/api/zcodeClaim";

const emptyState: ClaimState = {
  enabled: false,
  participants: [],
  records: {},
  busy: false,
};
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
  const { t } = useTranslation();
  const copy = (
    key: string,
    fallback: string,
    values?: Record<string, string>,
  ) => t(`zcode.claim.${key}`, { defaultValue: fallback, ...values });
  const [state, setState] = useState<ClaimState>(emptyState);
  const [selected, setSelected] = useState<string[]>([]);
  const [auto, setAuto] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  const epoch = useRef(0);
  const operationInFlight = useRef(false);
  const operationVersion = useRef(0);
  const backendBusy = useRef(false);
  const refresh = useRef<(() => void) | null>(null);
  useEffect(() => {
    const generation = ++epoch.current;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let initialized = false;
    let reading = false;
    setState(emptyState);
    setSelected([]);
    setAuto(false);
    setLoaded(false);
    setPending(false);
    setError(false);
    backendBusy.current = false;
    operationInFlight.current = false;
    const poll = async () => {
      if (disposed || reading || document.hidden) return;
      reading = true;
      const readVersion = operationVersion.current;
      try {
        const next = await zcodeClaimApi.state(dataRoot);
        if (
          !disposed &&
          generation === epoch.current &&
          readVersion === operationVersion.current
        ) {
          setState(next);
          backendBusy.current = next.busy;
          setLoaded(true);
          setError(false);
          if (!initialized) {
            setSelected(next.participants);
            setAuto(next.enabled);
            initialized = true;
          }
        }
      } catch {
        if (!disposed) setError(true);
      } finally {
        reading = false;
        if (!disposed && !document.hidden)
          timer = setTimeout(poll, backendBusy.current ? 2000 : 10000);
      }
    };
    const visibility = () => {
      clearTimeout(timer);
      if (!document.hidden) void poll();
    };
    refresh.current = visibility;
    document.addEventListener("visibilitychange", visibility);
    void poll();
    return () => {
      disposed = true;
      ++epoch.current;
      clearTimeout(timer);
      refresh.current = null;
      document.removeEventListener("visibilitychange", visibility);
    };
  }, [dataRoot]);
  const ids = selected.filter((id) =>
    profiles.some((profile) => profile.id === id),
  );
  const claimable = ids.filter((id) => state.records[id]?.canClaim === true);
  const blocked = disabled || pending || state.busy || !loaded;
  async function perform(operation: () => Promise<ClaimState>) {
    if (operationInFlight.current) return;
    const generation = epoch.current;
    operationInFlight.current = true;
    ++operationVersion.current;
    setPending(true);
    setError(false);
    try {
      const next = await operation();
      if (generation === epoch.current) {
        setState(next);
        backendBusy.current = next.busy;
        refresh.current?.();
      }
    } catch {
      if (generation === epoch.current) setError(true);
    } finally {
      if (generation === epoch.current) {
        operationInFlight.current = false;
        setPending(false);
      }
    }
  }
  const date = (seconds: number | null) => {
    if (seconds === null) return copy("unknown", "Unknown");
    const value = new Date(seconds * 1000);
    return Number.isFinite(value.getTime())
      ? value.toLocaleString()
      : copy("unknown", "Unknown");
  };
  return (
    <section className="space-y-3" aria-label={copy("title", "Plan claims")}>
      <h4 className="text-sm font-medium">{copy("title", "Plan claims")}</h4>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled={blocked || ids.length === 0}
          onClick={() =>
            void perform(() => zcodeClaimApi.start(dataRoot, ids, true))
          }
        >
          {copy("checkSelected", "Check selected")}
        </Button>
        <Button
          size="sm"
          disabled={blocked || claimable.length === 0}
          onClick={() =>
            void perform(() => zcodeClaimApi.start(dataRoot, claimable, false))
          }
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
              onClick={() => void perform(() => zcodeClaimApi.cancel(dataRoot))}
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
          disabled={blocked || (auto && ids.length === 0)}
          onClick={() =>
            void perform(() => zcodeClaimApi.setAuto(dataRoot, auto, ids))
          }
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
                      setSelected((previous) =>
                        checked
                          ? [
                              ...previous.filter((id) => id !== profile.id),
                              profile.id,
                            ]
                          : previous.filter((id) => id !== profile.id),
                      )
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
                    onClick={() =>
                      void perform(() =>
                        zcodeClaimApi.start(dataRoot, [profile.id], true),
                      )
                    }
                  >
                    {copy("check", "Check")}
                  </Button>
                  <Button
                    size="sm"
                    disabled={blocked || record?.canClaim !== true}
                    onClick={() =>
                      void perform(() =>
                        zcodeClaimApi.start(dataRoot, [profile.id], false),
                      )
                    }
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
