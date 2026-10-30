import { useEffect, useMemo, useRef, useState } from "react";
import { workbuddyApi } from "@/lib/api/workbuddy";
import { usePreservedViewActive } from "@/components/ui/PreservedView";
import {
  useMutation,
  useMutationState,
  useQueryClient,
} from "@tanstack/react-query";
import { accountPlatformPresenceKey } from "@/components/relay/accounts/useAccountPlatformPresence";
import { workbuddyNativeCapturesKey } from "@/components/relay/accounts/useWorkBuddyNativeCatalog";

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
export function useWorkBuddyNativePreviewViewModel() {
  const visible = usePreservedViewActive();
  const owner = useMemo(() => Symbol("native-preview-owner"), [visible]);
  const currentOwner = useRef(owner);
  currentOwner.current = owner;
  const [result, setResult] = useState<Observation | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const active = useRef(false);
  const alive = useRef(false);
  const generation = useRef(0);
  useEffect(() => {
    alive.current = true;
    setResult(null);
    setError(null);
    setBusy(active.current);
    return () => {
      alive.current = false;
      generation.current += 1;
    };
  }, []);
  const inspect = async () => {
    if (
      !alive.current ||
      !visible ||
      currentOwner.current !== owner ||
      active.current
    )
      return;
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
      if (alive.current) setBusy(false);
    }
  };
  return { result, error, busy, inspect };
}

// The mutation cache owns the outcome across Activity suspension and remounts.
// A rejected/lost response is not proof that the backend did not save.
const nativeCaptureMutationKey = ["workbuddy", "captureNative"] as const;

export function useWorkBuddyNativeCaptureViewModel(onCaptured?: () => void) {
  const client = useQueryClient();
  const visible = usePreservedViewActive();
  const token = useMemo(() => Symbol("native-capture-owner"), [visible]);
  const currentToken = useRef(token);
  currentToken.current = token;
  const history = useMutationState({
    filters: { mutationKey: nativeCaptureMutationKey, exact: true },
    select: (mutation) => ({
      status: mutation.state.status,
      conflict:
        mutation.state.error instanceof Error &&
        "code" in mutation.state.error &&
        mutation.state.error.code === "requestConflict",
    }),
  });
  const latest = history.at(-1);
  const status = latest?.status;
  const [label, setLabelState] = useState("");
  const labelRevision = useRef(0);
  const retired = useRef(false);
  const reviewedRevision = labelRevision.current;
  const setLabel = (value: string) => {
    if (
      !alive.current ||
      !visible ||
      currentToken.current !== token ||
      retired.current ||
      admitted.current
    )
      return;
    ++labelRevision.current;
    setLabelState(value);
  };
  const [submitted, setSubmitted] = useState(false);
  const alive = useRef(false);
  const admitted = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const mutation = useMutation({
    mutationKey: nativeCaptureMutationKey,
    retry: false,
    gcTime: Infinity,
    mutationFn: (input: { label: string; token: symbol }) =>
      workbuddyApi.captureNative({ label: input.label, expected: null }),
    onSuccess: (_result, input) => {
      // A cache refresh is not evidence about an unresolved request.
      void client
        .invalidateQueries({ queryKey: accountPlatformPresenceKey })
        .catch(() => {});
      void client
        .invalidateQueries({ queryKey: workbuddyNativeCapturesKey })
        .catch(() => {});
      if (
        alive.current &&
        !retired.current &&
        visible &&
        currentToken.current === input.token
      ) {
        try {
          onCaptured?.();
        } catch {
          /* The confirmed save is not undone by a UI refresh failure. */
        }
      }
    },
  });
  const locked = submitted || status === "pending" || status === "error";
  const confirm = () => {
    if (
      !alive.current ||
      retired.current ||
      !visible ||
      currentToken.current !== token ||
      reviewedRevision !== labelRevision.current ||
      admitted.current ||
      !label.trim()
    )
      return;
    // Recheck the shared owner synchronously; a stale render cannot race a click.
    const existing = client
      .getMutationCache()
      .findAll({ mutationKey: nativeCaptureMutationKey, exact: true });
    const last = existing.at(-1);
    if (
      last &&
      (last.state.status === "pending" || last.state.status === "error")
    )
      return;
    admitted.current = true;
    setSubmitted(true);
    mutation.mutate({ label: label.trim(), token });
  };
  const retire = () => {
    retired.current = true;
  };
  return {
    label,
    setLabel,
    retire,
    confirm,
    locked,
    status,
    conflict: latest?.conflict === true,
  };
}
