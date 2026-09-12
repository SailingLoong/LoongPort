import { useSyncExternalStore } from "react";
import type { AccountRoute } from "@/components/shell/navigation";

const storageKey = "loongport.accountDetailsHidden";
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((listener) => listener());
const snapshot = () => localStorage.getItem(storageKey);
function subscribe(listener: () => void) {
  listeners.add(listener);
  const onStorage = (event: StorageEvent) => {
    if (event.key === storageKey || event.key === null) listener();
  };
  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", onStorage);
  };
}
function readHidden(raw: string | null): string[] {
  try {
    const value: unknown = JSON.parse(raw ?? "[]");
    return Array.isArray(value)
      ? value.filter((key): key is string => typeof key === "string")
      : [];
  } catch {
    return [];
  }
}
const accountKey = (account: AccountRoute) => `${account.kind}:${account.id}`;

/** Display preference only. Never filters configured tiers or changes routing. */
export function useAccountVisibility() {
  const raw = useSyncExternalStore(subscribe, snapshot, () => null);
  const hidden = readHidden(raw);
  return {
    isAccountDetailsHidden: (account: AccountRoute) =>
      hidden.includes(accountKey(account)),
    setAccountDetailsHidden: (account: AccountRoute, hide: boolean) => {
      const next = new Set(readHidden(snapshot()));
      if (hide) next.add(accountKey(account));
      else next.delete(accountKey(account));
      localStorage.setItem(storageKey, JSON.stringify([...next]));
      notify();
    },
  };
}
