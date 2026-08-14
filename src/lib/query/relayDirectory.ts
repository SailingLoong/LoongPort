import type { LeaderboardKind } from "@/lib/api/relay";

export const relayDirectoryKeys = {
  all: ["relay-directory"] as const,
  byKind: (kind: LeaderboardKind) => [...relayDirectoryKeys.all, kind] as const,
};
