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

/** 启动后多久做第一次检查。留几秒让首屏渲染、数据库迁移、托盘创建先完成。 */
const STARTUP_CHECK_DELAY_MS = 5_000;

/**
 * 之后每隔多久再查一次。
 *
 * **6 小时**：发版频率是天/周级，查得再密也不会更早发现；而每次检查是一次网络请求，
 * 对挂着不关的用户（`minimize_to_tray_on_close` 默认为 true，所以这是常态）
 * 一天四次已经足够及时。
 */
const PERIODIC_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

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

  // 自动检查更新：**启动后一次 + 之后每 6 小时一次**。
  //
  // 2026-08-04 接上自己的发布渠道后启用（在此之前 updater 插件有意不注册、`check()`
  // 必抛，所以那时这段是删掉的）。2026-08-05 补上定时那半 —— 当初判断「桌面工具不是
  // 常驻服务、用户重开就再查」，那是**错的**：`minimize_to_tray_on_close` 默认为 true
  // ⇒ 用户点关闭是最小化到托盘、进程不退出 ⇒ 「挂着好几天」是常态而非例外，
  // 只在启动时查等于那些用户永远收不到提醒。
  //
  // 四个刻意的选择：
  //
  // **启动延迟 5 秒**，不跟启动争资源 —— 首屏要渲染、数据库要迁移、托盘要建，
  // 而「有没有新版本」不着急这几秒。
  //
  // **间隔 6 小时，且不做持久化。** 计时器只活在进程生命周期内 ⇒ 不需要存「上次查询
  // 时间」，重启就是重新开始（那时启动那次已经查过了，语义上不缺）。有意不做「按真实
  // 时间补偿」：睡眠唤醒后 `setInterval` 可能立刻触发一次，那恰好是我们想要的
  // （睡了一晚醒来查一次），不是要修的 bug。
  //
  // **失败完全静默**（`checkUpdate` 内部把错误收进 `error` 状态，这里不弹任何东西）。
  // 检查更新要发网络，而离线、GitHub 不可达、公司网络拦截都是**常态而非异常** ——
  // 为此弹一个「检查更新失败」是拿用户不关心的事打扰他。用户主动点「检查更新」时才
  // 提示，那时他在等一个答复（见 `AboutSection` 的 `handleCheckUpdate`）。
  //
  // **重复发现同一个版本不会重复打扰**：`checkUpdate` 会比对
  // `DISMISSED_VERSION_KEY` —— 用户关掉过某个版本的提醒后，之后每次轮询到同一个版本
  // 都仍是 dismissed 状态。所以 6 小时一次不会变成「6 小时一次弹窗」。
  useEffect(() => {
    // ⚠️ **必须 `.catch()`，不能只写 `void checkUpdate()`。** `checkUpdate` 失败时会
    // `throw err`（那是为主动点击那条路准备的 —— 调用方要据此弹提示），而自动检查这条
    // 路没人接 ⇒ 每次失败都是一条 unhandled rejection。启动查一次时那只是一条噪音，
    // 加上轮询后离线用户每 6 小时来一条，控制台会被刷满。
    //
    // 这里什么都不做是对的：错误已经被 `checkUpdate` 收进 `error` 状态、也已经
    // `console.error` 过，自动检查失败本就不该打扰用户（见上面那段说明）。
    const auto = () => {
      checkUpdate().catch(() => {});
    };
    const initial = setTimeout(auto, STARTUP_CHECK_DELAY_MS);
    const periodic = setInterval(auto, PERIODIC_CHECK_INTERVAL_MS);
    return () => {
      clearTimeout(initial);
      clearInterval(periodic);
    };
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
