import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { workbuddyApi } from "@/lib/api/workbuddy";

// This is an observation, never a native snapshot or an activation grant.
type Observation = {
  sourceVerification: "knownLocationOnly";
  activationAvailable: false;
  logoutMarkerPresent: boolean;
} & (
  | { authFile: "missing"; realm: null }
  | {
      authFile: "recognizedPersonalFormat";
      realm: "www.workbuddy.cn" | "www.codebuddy.cn";
    }
);
function observation(value: unknown): Observation | null {
  if (!value || typeof value !== "object") return null;
  const v = value as Record<string, unknown>;
  if (
    v.sourceVerification !== "knownLocationOnly" ||
    v.activationAvailable !== false ||
    typeof v.logoutMarkerPresent !== "boolean"
  )
    return null;
  if (!(
    (v.authFile === "missing" && v.realm === null) ||
    (v.authFile === "recognizedPersonalFormat" &&
      (v.realm === "www.workbuddy.cn" || v.realm === "www.codebuddy.cn"))
  ))
    return null;
  // Copy only the reviewed fields; do not retain unexpected backend material.
  return {
    sourceVerification: "knownLocationOnly",
    activationAvailable: false,
    logoutMarkerPresent: v.logoutMarkerPresent,
    authFile: v.authFile,
    realm: v.realm,
  } as Observation;
}
const errors = [
  "unsafeSource",
  "sourceChanged",
  "unreadableSource",
  "invalidDocument",
  "unsupportedContext",
  "unsupportedPlatform",
] as const;
type Failure = (typeof errors)[number] | "unknown";
function failure(value: unknown): Failure {
  return errors.find((error) => error === value) ?? "unknown";
}
export function WorkBuddyNativePreview() {
  const { t } = useTranslation();
  const [result, setResult] = useState<Observation | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const active = useRef(false);
  const generation = useRef(0);
  useEffect(
    () => () => {
      generation.current += 1;
    },
    [],
  );
  const inspect = async () => {
    if (active.current) return;
    active.current = true;
    const expected = generation.current;
    setBusy(true);
    setResult(null);
    setError(null);
    try {
      const value = observation(await workbuddyApi.previewNative());
      if (generation.current !== expected) return;
      if (value) setResult(value);
      else setError("unknown");
    } catch (cause) {
      if (generation.current === expected) setError(failure(cause));
    } finally {
      active.current = false;
      if (generation.current === expected) setBusy(false);
    }
  };
  return (
    <section
      aria-label={t("workbuddy.nativePreview.title")}
      className="space-y-2 rounded-xl border border-border bg-card p-3 text-xs"
    >
      <h3 className="text-sm font-medium">
        {t("workbuddy.nativePreview.title")}
      </h3>
      <p className="text-muted-foreground">
        {t("workbuddy.nativePreview.description")}
      </p>
      <Button
        size="sm"
        variant="outline"
        disabled={busy}
        onClick={() => void inspect()}
      >
        {t(
          busy
            ? "workbuddy.nativePreview.inspecting"
            : "workbuddy.nativePreview.inspect",
        )}
      </Button>
      {error && (
        <p role="alert">{t(`workbuddy.nativePreview.error.${error}`)}</p>
      )}
      {result && (
        <div role="status" className="space-y-1">
          <p>
            {t(
              result.authFile === "missing"
                ? "workbuddy.nativePreview.missing"
                : "workbuddy.nativePreview.recognized",
            )}
          </p>
          {result.realm && (
            <p>
              {t(
                result.realm === "www.workbuddy.cn"
                  ? "workbuddy.nativePreview.realmWorkbuddy"
                  : "workbuddy.nativePreview.realmCodebuddy",
              )}
            </p>
          )}
          <p>
            {t(
              result.logoutMarkerPresent
                ? "workbuddy.nativePreview.markerPresent"
                : "workbuddy.nativePreview.markerAbsent",
            )}
          </p>
        </div>
      )}
      <p className="text-muted-foreground">
        {t("workbuddy.nativePreview.notActivation")}
      </p>
    </section>
  );
}
