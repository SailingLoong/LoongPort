import { useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import { workbuddyApi } from "@/lib/api/workbuddy";
import { usePreservedViewActive } from "@/components/ui/PreservedView";
import type { PlatformAccountSummary } from "./useAccountPlatformPresence";
import {
  workbuddyInventorySignature,
  type WorkBuddyNativeCatalogObservation,
} from "./workbuddyNativeRowIdentity";
export const workbuddyNativeCapturesKey = ["workbuddyNativeCaptures"] as const;
/** Same QueryClient as the inventory. No native writes or second account store. */
export function useWorkBuddyNativeCatalog({
  accounts,
  revision,
  fresh,
}: {
  accounts: readonly PlatformAccountSummary[];
  revision: number;
  fresh: boolean;
}): WorkBuddyNativeCatalogObservation {
  const active = usePreservedViewActive();
  const signature = workbuddyInventorySignature(accounts);
  const knownRevision =
    Number.isFinite(revision) && revision > 0 ? revision : null;
  const context = JSON.stringify([signature, knownRevision, fresh, active]);
  // A read observation epoch prevents delete/recreate and refresh transitions from
  // selecting an old cached incarnation, even if a clock timestamp is repeated.
  // It is ephemeral UI request identity, never backend account state.
  const observation = useRef({ context, epoch: 0 });
  if (observation.current.context !== context)
    observation.current = { context, epoch: observation.current.epoch + 1 };
  const enabled =
    active && fresh && signature !== "[]" && knownRevision !== null;
  const query = useQuery({
    queryKey: [
      ...workbuddyNativeCapturesKey,
      signature,
      knownRevision,
      observation.current.epoch,
    ],
    queryFn: ({ signal }) => workbuddyApi.listNativeCaptures(signal),
    enabled,
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: 0,
  });
  const state =
    !fresh || !active
      ? "stale"
      : !enabled
        ? "disabled"
        : query.isFetching || query.isPending
          ? "loading"
          : query.isError
            ? "error"
            : "success";
  return {
    state,
    data: state === "success" ? query.data : undefined,
    isFetching: query.isFetching,
  };
}
