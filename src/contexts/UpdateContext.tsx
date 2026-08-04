import React, {
  createContext,
  useContext,
  useState,
  useEffect,
  useCallback,
  useRef,
} from "react";
import type { UpdateInfo } from "../lib/updater";
import { checkForUpdate } from "../lib/updater";

interface UpdateContextValue {
  // 更新状态
  hasUpdate: boolean;
  updateInfo: UpdateInfo | null;
  isChecking: boolean;
  error: string | null;

  // 提示状态
  isDismissed: boolean;
  dismissUpdate: () => void;

  // 操作方法
  checkUpdate: () => Promise<boolean>;
  resetDismiss: () => void;
}

const UpdateContext = createContext<UpdateContextValue | undefined>(undefined);

export function UpdateProvider({ children }: { children: React.ReactNode }) {
  const DISMISSED_VERSION_KEY = "ccswitch:update:dismissedVersion";
  const LEGACY_DISMISSED_KEY = "dismissedUpdateVersion"; // 兼容旧键

  const [hasUpdate, setHasUpdate] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [isChecking, setIsChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isDismissed, setIsDismissed] = useState(false);

  // 从 localStorage 读取已关闭的版本
  useEffect(() => {
    const current = updateInfo?.availableVersion;
    if (!current) return;

    // 读取新键；若不存在，尝试迁移旧键
    let dismissedVersion = localStorage.getItem(DISMISSED_VERSION_KEY);
    if (!dismissedVersion) {
      const legacy = localStorage.getItem(LEGACY_DISMISSED_KEY);
      if (legacy) {
        localStorage.setItem(DISMISSED_VERSION_KEY, legacy);
        localStorage.removeItem(LEGACY_DISMISSED_KEY);
        dismissedVersion = legacy;
      }
    }

    setIsDismissed(dismissedVersion === current);
  }, [updateInfo?.availableVersion]);

  const isCheckingRef = useRef(false);

  const checkUpdate = useCallback(async () => {
    if (isCheckingRef.current) return false;
    isCheckingRef.current = true;
    setIsChecking(true);
    setError(null);

    try {
      const result = await checkForUpdate({ timeout: 30000 });

      if (result.status === "available") {
        setHasUpdate(true);
        setUpdateInfo(result.info);

        // 检查是否已经关闭过这个版本的提醒
        let dismissedVersion = localStorage.getItem(DISMISSED_VERSION_KEY);
        if (!dismissedVersion) {
          const legacy = localStorage.getItem(LEGACY_DISMISSED_KEY);
          if (legacy) {
            localStorage.setItem(DISMISSED_VERSION_KEY, legacy);
            localStorage.removeItem(LEGACY_DISMISSED_KEY);
            dismissedVersion = legacy;
          }
        }
        setIsDismissed(dismissedVersion === result.info.availableVersion);
        return true; // 有更新
      } else {
        setHasUpdate(false);
        setUpdateInfo(null);
        setIsDismissed(false);
        return false; // 已是最新
      }
    } catch (err) {
      console.error("检查更新失败:", err);
      setError(err instanceof Error ? err.message : "检查更新失败");
      setHasUpdate(false);
      throw err; // 抛出错误让调用方处理
    } finally {
      setIsChecking(false);
      isCheckingRef.current = false;
    }
  }, []);

  const dismissUpdate = useCallback(() => {
    setIsDismissed(true);
    if (updateInfo?.availableVersion) {
      localStorage.setItem(DISMISSED_VERSION_KEY, updateInfo.availableVersion);
      // 清理旧键
      localStorage.removeItem(LEGACY_DISMISSED_KEY);
    }
  }, [updateInfo?.availableVersion]);

  const resetDismiss = useCallback(() => {
    setIsDismissed(false);
    localStorage.removeItem(DISMISSED_VERSION_KEY);
    localStorage.removeItem(LEGACY_DISMISSED_KEY);
  }, []);

  // 启动时自动检查一次。2026-08-04 接上自己的发布渠道后启用（在此之前 updater 插件
  // 有意不注册，`check()` 必抛 ⇒ 那时这段每次启动只会吐一条错误，所以当时是删掉的）。
  //
  // 三个刻意的选择：
  //
  // **延迟 5 秒**，不跟启动争资源 —— 首屏要渲染、数据库要迁移、托盘要建，
  // 而"有没有新版本"不着急这几秒。
  //
  // **失败完全静默**（`checkUpdate` 内部把错误收进 `error` 状态，这里不弹任何东西）。
  // 检查更新要发网络，而离线、GitHub 不可达、公司网络拦截都是**常态而非异常** ——
  // 为此弹一个"检查更新失败"是拿用户不关心的事打扰他。用户主动点"检查更新"时才提示，
  // 那时他在等一个答复（见 `AboutSection` 的 `handleCheckUpdate`）。
  //
  // **只查一次，不设轮询**。这是个桌面工具、不是常驻服务，用户重开就再查一次；
  // 定时轮询要多一套"上次查询时间"的持久化状态，收益却只是"挂着不关也能收到提醒"。
  // 真需要时再加（那时判据是：有多少用户会连着挂几天不重启）。
  useEffect(() => {
    const timer = setTimeout(() => {
      void checkUpdate();
    }, 5000);
    return () => clearTimeout(timer);
  }, [checkUpdate]);

  const value: UpdateContextValue = {
    hasUpdate,
    updateInfo,
    isChecking,
    error,
    isDismissed,
    dismissUpdate,
    checkUpdate,
    resetDismiss,
  };

  return (
    <UpdateContext.Provider value={value}>{children}</UpdateContext.Provider>
  );
}

export function useUpdate() {
  const context = useContext(UpdateContext);
  if (!context) {
    throw new Error("useUpdate must be used within UpdateProvider");
  }
  return context;
}
