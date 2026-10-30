import { accountPlatforms, type AccountPlatform } from "./accountPlatforms";

export const ACCOUNT_OVERVIEW_PREFERENCES_KEY =
  "loongport:account-overview-preferences:v1";
export type AccountOverviewSort = "expiryAsc" | "balanceDesc" | "nameAsc";
export interface AccountOverviewPreferences {
  version: 1;
  groupOrder: AccountPlatform[];
  collapsedGroups: AccountPlatform[];
  sortByGroup: Partial<Record<AccountPlatform, AccountOverviewSort>>;
  hideExhausted: boolean;
}
type PreferenceStorage = Pick<Storage, "getItem" | "setItem">;
const groupIds = accountPlatforms.map((group) => group.id);
const knownGroup = (value: unknown): value is AccountPlatform =>
  typeof value === "string" && groupIds.some((id) => id === value);
const validSort = (value: unknown): value is AccountOverviewSort =>
  value === "expiryAsc" || value === "balanceDesc" || value === "nameAsc";
const ids = (value: unknown): AccountPlatform[] =>
  Array.isArray(value) ? [...new Set(value.filter(knownGroup))] : [];

/** UI preferences only. Never restores identity, routing priority or admission. */
export function normalizeAccountOverviewPreferences(
  value: unknown,
): AccountOverviewPreferences {
  const v =
    value &&
    typeof value === "object" &&
    (value as Record<string, unknown>).version === 1
      ? (value as Record<string, unknown>)
      : {};
  const order = ids(v.groupOrder);
  const sorts =
    v.sortByGroup && typeof v.sortByGroup === "object"
      ? Object.entries(v.sortByGroup).filter(
          ([id, sort]) => knownGroup(id) && validSort(sort),
        )
      : [];
  return {
    version: 1,
    groupOrder: [...order, ...groupIds.filter((id) => !order.includes(id))],
    collapsedGroups: ids(v.collapsedGroups),
    sortByGroup: Object.fromEntries(sorts),
    hideExhausted:
      typeof v.hideExhausted === "boolean" ? v.hideExhausted : true,
  };
}

function browserStorage(): PreferenceStorage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Save once per explicit VM operation; read/normalization never writes. */
export function createAccountOverviewPreferencesRepository(
  storage: PreferenceStorage | null = browserStorage(),
) {
  return {
    read(): AccountOverviewPreferences {
      try {
        return normalizeAccountOverviewPreferences(
          JSON.parse(
            storage?.getItem(ACCOUNT_OVERVIEW_PREFERENCES_KEY) ?? "null",
          ),
        );
      } catch {
        return normalizeAccountOverviewPreferences(null);
      }
    },
    save(value: AccountOverviewPreferences): {
      preferences: AccountOverviewPreferences;
      persisted: boolean;
    } {
      const preferences = normalizeAccountOverviewPreferences(value);
      try {
        if (!storage) return { preferences, persisted: false };
        storage.setItem(
          ACCOUNT_OVERVIEW_PREFERENCES_KEY,
          JSON.stringify(preferences),
        );
        return { preferences, persisted: true };
      } catch {
        return { preferences, persisted: false };
      }
    },
  };
}
