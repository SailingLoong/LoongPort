export const relayDirectoryKeys = {
  all: ["relay-directory"] as const,
  listing: () => [...relayDirectoryKeys.all, "listing"] as const,
};
