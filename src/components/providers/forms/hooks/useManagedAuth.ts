import { useState, useCallback, useRef, useEffect } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { authApi, settingsApi } from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import type {
  ManagedAuthProvider,
  ManagedAuthStatus,
  ManagedAuthDeviceCodeResponse,
} from "@/lib/api";

type AuthSession = {
  provider: ManagedAuthProvider;
  domain?: string;
  deviceCode?: string;
  pollInFlight: boolean;
  cancelIssued?: boolean;
  cancelGeneration?: number;
};

type PollingState = "idle" | "polling" | "success" | "error";

export function useManagedAuth(
  authProvider: ManagedAuthProvider,
  githubDomain?: string,
) {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const queryKey = ["managed-auth-status", authProvider];

  const [pollingState, setPollingState] = useState<PollingState>("idle");
  const [deviceCode, setDeviceCode] =
    useState<ManagedAuthDeviceCodeResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const mountedRef = useRef(false);
  const generationRef = useRef(0);
  const sessionRef = useRef<AuthSession | null>(null);
  const [isStarting, setIsStarting] = useState(false);

  const pollingTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pollingTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const {
    data: authStatus,
    isLoading: isLoadingStatus,
    isSuccess: isStatusSuccess,
    isError: isStatusError,
    refetch: refetchStatus,
  } = useQuery<ManagedAuthStatus>({
    queryKey,
    queryFn: () => authApi.authGetStatus(authProvider),
    staleTime: 30000,
    // A rejected xAI refresh token is persisted as `requires_reauth` by the
    // proxy hot path. Periodically refresh local status so an already-open Auth
    // Center stops showing the account as logged in without requiring a reload.
    refetchInterval: authProvider === "xai_oauth" ? 15_000 : false,
  });

  const stopPolling = useCallback(() => {
    if (pollingTimerRef.current) {
      clearTimeout(pollingTimerRef.current);
      pollingTimerRef.current = null;
    }
    if (pollingTimeoutRef.current) {
      clearTimeout(pollingTimeoutRef.current);
      pollingTimeoutRef.current = null;
    }
  }, []);

  const cancelDeviceFlow = useCallback(
    async (session: AuthSession) => {
      if (!session.deviceCode || session.cancelIssued) return;
      session.cancelIssued = true;
      try {
        await authApi.authCancelLogin(
          session.provider,
          session.deviceCode,
          session.domain,
        );
        // Cancellation may lose to an already committed account. Re-read the
        // original account owner; never delete an account to fake cancellation.
        await queryClient.invalidateQueries({
          queryKey: ["managed-auth-status", session.provider],
        });
      } catch (e) {
        if (
          mountedRef.current &&
          !sessionRef.current &&
          generationRef.current === session.cancelGeneration
        ) {
          setPollingState("error");
          setError(
            `Login cancellation could not be confirmed: ${e instanceof Error ? e.message : String(e)}`,
          );
        }
      }
    },
    [queryClient],
  );

  const invalidateSession = useCallback(() => {
    const session = sessionRef.current;
    sessionRef.current = null;
    generationRef.current += 1;
    stopPolling();
    if (session) {
      session.cancelGeneration = generationRef.current;
      void cancelDeviceFlow(session);
    }
  }, [stopPolling, cancelDeviceFlow]);

  useEffect(() => {
    mountedRef.current = true;
    generationRef.current += 1;
    // Provider/domain changes and unmount also invalidate async continuations.
    setIsStarting(false);
    setPollingState("idle");
    setDeviceCode(null);
    setError(null);
    return () => {
      mountedRef.current = false;
      invalidateSession();
    };
  }, [authProvider, githubDomain, invalidateSession]);

  const startLoginMutation = useMutation({
    mutationFn: (session: AuthSession) =>
      authApi.authStartLogin(session.provider, session.domain),
    onSuccess: async (response, session) => {
      const ownsSession = () => sessionRef.current === session;
      session.deviceCode = response.device_code;
      if (!ownsSession()) {
        // A cancelled start can return a newly registered backend flow later.
        await cancelDeviceFlow(session);
        return;
      }
      setIsStarting(false);
      setDeviceCode(response);
      setPollingState("polling");
      setError(null);

      try {
        await copyText(response.user_code);
      } catch (e) {
        console.debug("[ManagedAuth] Failed to copy user code:", e);
      }
      if (!ownsSession()) return;
      try {
        await settingsApi.openExternal(response.verification_uri);
      } catch (e) {
        console.debug("[ManagedAuth] Failed to open browser:", e);
      }
      if (!ownsSession()) return;

      const interval = Math.max((response.interval || 5) + 3, 8) * 1000;
      const expiresAt = Date.now() + response.expires_in * 1000;
      const expire = () => {
        if (!ownsSession()) return;
        invalidateSession();
        setPollingState("error");
        setError("Device code expired. Please try again.");
      };
      const pollOnce = async () => {
        if (!ownsSession() || session.pollInFlight) return;
        if (Date.now() > expiresAt) {
          expire();
          return;
        }
        session.pollInFlight = true;
        try {
          const newAccount = await authApi.authPollForAccount(
            session.provider,
            response.device_code,
            session.domain,
          );
          if (!ownsSession()) return;
          if (newAccount) {
            stopPolling();
            setPollingState("success");
            await refetchStatus();
            if (!ownsSession()) return;
            await queryClient.invalidateQueries({ queryKey });
            if (!ownsSession()) return;
            sessionRef.current = null;
            setPollingState("idle");
            setDeviceCode(null);
          }
        } catch (e) {
          if (!ownsSession()) return;
          const errorMessage = e instanceof Error ? e.message : String(e);
          if (
            !errorMessage.includes("pending") &&
            !errorMessage.includes("slow_down")
          ) {
            invalidateSession();
            setPollingState("error");
            setError(errorMessage);
          }
        } finally {
          session.pollInFlight = false;
          if (ownsSession()) {
            pollingTimerRef.current = setTimeout(pollOnce, interval);
          }
        }
      };
      pollingTimeoutRef.current = setTimeout(
        expire,
        response.expires_in * 1000,
      );
      void pollOnce();
    },
    onError: (e, session) => {
      if (sessionRef.current !== session) return;
      invalidateSession();
      setIsStarting(false);
      setPollingState("error");
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const logoutMutation = useMutation({
    mutationFn: () => authApi.authLogout(authProvider),
    onSuccess: async () => {
      setPollingState("idle");
      setDeviceCode(null);
      setError(null);
      queryClient.setQueryData(queryKey, {
        provider: authProvider,
        authenticated: false,
        default_account_id: null,
        accounts: [],
      });
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: async (e) => {
      console.error("[ManagedAuth] Failed to logout:", e);
      setError(e instanceof Error ? e.message : String(e));
      await refetchStatus();
    },
  });

  const removeAccountMutation = useMutation({
    mutationFn: (accountId: string) =>
      authApi.authRemoveAccount(authProvider, accountId),
    onSuccess: async () => {
      setPollingState("idle");
      setDeviceCode(null);
      setError(null);
      toast.success(
        t("managedAuth.accountRemoved", {
          defaultValue: "账号已移除",
        }),
      );
      await refetchStatus();
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e) => {
      console.error("[ManagedAuth] Failed to remove account:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const setDefaultAccountMutation = useMutation({
    mutationFn: (accountId: string) =>
      authApi.authSetDefaultAccount(authProvider, accountId),
    onSuccess: async () => {
      await refetchStatus();
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e) => {
      console.error("[ManagedAuth] Failed to set default account:", e);
      setError(e instanceof Error ? e.message : String(e));
    },
  });

  const startAuth = useCallback(() => {
    // Claim synchronously: repeated clicks before React renders still coalesce.
    if (sessionRef.current) return;
    const session: AuthSession = {
      provider: authProvider,
      domain: githubDomain,
      pollInFlight: false,
    };
    sessionRef.current = session;
    generationRef.current += 1;
    setIsStarting(true);
    setPollingState("idle");
    setDeviceCode(null);
    setError(null);
    startLoginMutation.mutate(session);
  }, [authProvider, githubDomain, startLoginMutation]);

  const cancelAuth = useCallback(() => {
    invalidateSession();
    setIsStarting(false);
    setPollingState("idle");
    setDeviceCode(null);
    setError(null);
  }, [invalidateSession]);

  const logout = useCallback(() => {
    cancelAuth();
    logoutMutation.mutate();
  }, [cancelAuth, logoutMutation]);

  const removeAccount = useCallback(
    (accountId: string) => {
      removeAccountMutation.mutate(accountId);
    },
    [removeAccountMutation],
  );

  const setDefaultAccount = useCallback(
    (accountId: string) => {
      setDefaultAccountMutation.mutate(accountId);
    },
    [setDefaultAccountMutation],
  );

  const accounts = authStatus?.accounts ?? [];

  return {
    authStatus,
    isLoadingStatus,
    // Distinguish "status loaded successfully" from "loading / failed" so
    // callers don't treat a failed query's empty `accounts` as authoritative.
    isStatusSuccess,
    isStatusError,
    accounts,
    hasAnyAccount: accounts.length > 0,
    isAuthenticated: authStatus?.authenticated ?? false,
    defaultAccountId: authStatus?.default_account_id ?? null,
    migrationError: authStatus?.migration_error ?? null,
    pollingState,
    deviceCode,
    error,
    isPolling: pollingState === "polling",
    isAddingAccount: isStarting || pollingState === "polling",
    isRemovingAccount: removeAccountMutation.isPending,
    isSettingDefaultAccount: setDefaultAccountMutation.isPending,
    startAuth,
    addAccount: startAuth,
    cancelAuth,
    logout,
    removeAccount,
    setDefaultAccount,
    refetchStatus,
  };
}
