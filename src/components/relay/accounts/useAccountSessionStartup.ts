import { useEffect, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { relayApi } from "@/lib/api/relay";
import { serviceAccountsKey } from "./useServiceAccounts";

/** Mount once in the application shell, independently of pages and queries. */
export function useAccountSessionStartup(onExpired?: () => void) {
  const client = useQueryClient();
  const { t } = useTranslation();
  const latest = useRef({ t, onExpired });
  latest.current = { t, onExpired };
  const probe = useRef<Promise<number[]> | null>(null);

  useEffect(() => {
    let active = true;
    // Reuse the startup request during StrictMode's effect replay.
    probe.current ??= relayApi.checkSession();
    void probe.current.then(
      (expired) => {
        if (!active || expired.length === 0) return;
        toast.info(
          latest.current.t("loongport.session.expired", {
            count: expired.length,
          }),
        );
        void client.invalidateQueries({ queryKey: serviceAccountsKey });
        latest.current.onExpired?.();
      },
      () => {},
    );
    return () => {
      active = false;
    };
  }, [client]);
}
