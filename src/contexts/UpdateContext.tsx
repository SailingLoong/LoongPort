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
  getDismissedUpdateVersion,
  setDismissedUpdateVersion,
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

// localStorage 时代的旧键：只读一次性迁移到后端 settings，运行期不再读写。
const LEGACY_DISMISSED_KEYS = [
  "ccswitch:update:dismissedVersion",
  "dismissedUpdateVersion",
];

function legacyDismissedVersion(): string | null {
  for (const key of LEGACY_DISMISSED_KEYS) {
    const value = localStorage.getItem(key);
    if (value) {
      LEGACY_DISMISSED_KEYS.forEach((k) => localStorage.removeItem(k));
      return value;
    }
  }
  return null;
}

export function UpdateProvider({ children }: { children: React.ReactNode }) {
  const [hasUpdate, setHasUpdate] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [isChecking, setIsChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isDismissed, setIsDismissed] = useState(false);
  const isCheckingRef = useRef(false);
  // 「跳过本版本」的事实 owner 在后端 settings（启动闸门读同一份做自动
  // 安装决策）；这里只持镜像。refs 保证「检查事件先到、跳过版本后到」
  //（或反过来）都能重算出正确的 isDismissed。
  const updateInfoRef = useRef<UpdateInfo | null>(null);
  const dismissedVersionRef = useRef<string | null>(null);

  const syncDismissed = useCallback(() => {
    const info = updateInfoRef.current;
    setIsDismissed(
      info != null && dismissedVersionRef.current === info.availableVersion,
    );
  }, []);

  // 挂载时对齐后端的跳过版本，并把 localStorage 时代的旧值一次性迁过去。
  useEffect(() => {
    let mounted = true;
    void (async () => {
      const legacy = legacyDismissedVersion();
      let current: string | null = null;
      try {
        current = await getDismissedUpdateVersion();
      } catch (err) {
        console.error("读取跳过的更新版本失败:", err);
      }
      if (!mounted) return;
      if (current == null && legacy) {
        current = legacy;
        try {
          await setDismissedUpdateVersion(legacy);
        } catch (err) {
          console.error("迁移跳过的更新版本失败:", err);
        }
      }
      dismissedVersionRef.current = current;
      syncDismissed();
    })();
    return () => {
      mounted = false;
    };
  }, [syncDismissed]);

  const applyCheckResult = useCallback(
    (result: AppUpdateCheckResult) => {
      setError(null);

      if (result.status === "available") {
        setHasUpdate(true);
        updateInfoRef.current = result.info;
        setUpdateInfo(result.info);
        syncDismissed();
        return true;
      }

      setHasUpdate(false);
      updateInfoRef.current = null;
      setUpdateInfo(null);
      setIsDismissed(false);
      return false;
    },
    [syncDismissed],
  );

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
    const version = updateInfoRef.current?.availableVersion;
    if (!version) return;
    dismissedVersionRef.current = version;
    setIsDismissed(true);
    setDismissedUpdateVersion(version).catch((err) => {
      console.error("持久化跳过的更新版本失败:", err);
    });
  }, []);

  const resetDismiss = useCallback(() => {
    dismissedVersionRef.current = null;
    setIsDismissed(false);
    setDismissedUpdateVersion(null).catch((err) => {
      console.error("撤销跳过的更新版本失败:", err);
    });
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
