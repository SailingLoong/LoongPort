import { useQueryClient } from "@tanstack/react-query";

import { RELAY_DIRECTORY_UPDATED_EVENT } from "@/config/constants";
import type { LeaderboardKind } from "@/lib/api/relay";
import { relayDirectoryKeys } from "@/lib/query/relayDirectory";

import { useTauriEvent } from "./useTauriEvent";

/** Keep background VeriDrop refreshes visible even while the directory is closed. */
export function useRelayDirectoryCacheBridge() {
  const queryClient = useQueryClient();

  useTauriEvent<{ kind: LeaderboardKind }>(
    RELAY_DIRECTORY_UPDATED_EVENT,
    ({ kind }) =>
      queryClient.invalidateQueries({
        queryKey: relayDirectoryKeys.byKind(kind),
        exact: true,
      }),
  );
}
