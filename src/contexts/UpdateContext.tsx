import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { APP_UPDATE_CHECKED_EVENT } from "@/config/constants";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  checkForUpdate,
  type AppUpdateCheckResult,
  type UpdateInfo,
} from "../lib/updater";

interface UpdateContextValue {
  hasUpdate: boolean;
  updateInfo: UpdateInfo | null;
  isChecking: boolean;
  error: string | null;
  isDismissed: boolean;
  dismissUpdate: () => void;
  checkUpdate: () => Promise<boolean>;
  resetDismiss: () => void;
}

const UpdateContext = createContext<UpdateContextValue | undefined>(undefined);

const DISMISSED_VERSION_KEY = "ccswitch:update:dismissedVersion";
const LEGACY_DISMISSED_KEY = "dismissedUpdateVersion";

function dismissedVersion(): string | null {
  const current = localStorage.getItem(DISMISSED_VERSION_KEY);
  if (current) return current;

  const legacy = localStorage.getItem(LEGACY_DISMISSED_KEY);
  if (legacy) {
    localStorage.setItem(DISMISSED_VERSION_KEY, legacy);
    localStorage.removeItem(LEGACY_DISMISSED_KEY);
  }
  return legacy;
}

export function UpdateProvider({ children }: { children: React.ReactNode }) {
  const [hasUpdate, setHasUpdate] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [isChecking, setIsChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isDismissed, setIsDismissed] = useState(false);
  const isCheckingRef = useRef(false);

  const applyCheckResult = useCallback((result: AppUpdateCheckResult) => {
    setError(null);

    if (result.status === "available") {
      setHasUpdate(true);
      setUpdateInfo(result.info);
      setIsDismissed(dismissedVersion() === result.info.availableVersion);
      return true;
    }

    setHasUpdate(false);
    setUpdateInfo(null);
    setIsDismissed(false);
    return false;
  }, []);

  const checkUpdate = useCallback(async () => {
    if (isCheckingRef.current) return false;
    isCheckingRef.current = true;
    setIsChecking(true);
    setError(null);

    try {
      return applyCheckResult(await checkForUpdate());
    } catch (err) {
      console.error("检查更新失败:", err);
      setError(extractErrorMessage(err) || "检查更新失败");
      setHasUpdate(false);
      throw err;
    } finally {
      setIsChecking(false);
      isCheckingRef.current = false;
    }
  }, [applyCheckResult]);

  const dismissUpdate = useCallback(() => {
    setIsDismissed(true);
    if (updateInfo?.availableVersion) {
      localStorage.setItem(DISMISSED_VERSION_KEY, updateInfo.availableVersion);
      localStorage.removeItem(LEGACY_DISMISSED_KEY);
    }
  }, [updateInfo?.availableVersion]);

  const resetDismiss = useCallback(() => {
    setIsDismissed(false);
    localStorage.removeItem(DISMISSED_VERSION_KEY);
    localStorage.removeItem(LEGACY_DISMISSED_KEY);
  }, []);

  useEffect(() => {
    let mounted = true;
    let unlisten: UnlistenFn | undefined;

    void listen<AppUpdateCheckResult>(APP_UPDATE_CHECKED_EVENT, (event) => {
      if (mounted) applyCheckResult(event.payload);
    })
      .then((cleanup) => {
        if (mounted) {
          unlisten = cleanup;
        } else {
          cleanup();
        }
      })
      .catch((listenerError) => {
        console.error("Failed to listen for app update checks", listenerError);
      });

    return () => {
      mounted = false;
      unlisten?.();
    };
  }, [applyCheckResult]);

  return (
    <UpdateContext.Provider
      value={{
        hasUpdate,
        updateInfo,
        isChecking,
        error,
        isDismissed,
        dismissUpdate,
        checkUpdate,
        resetDismiss,
      }}
    >
      {children}
    </UpdateContext.Provider>
  );
}

export function useUpdate() {
  const context = useContext(UpdateContext);
  if (!context) {
    throw new Error("useUpdate must be used within UpdateProvider");
  }
  return context;
}
