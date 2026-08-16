import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { autoModeApi, type AutoModeStrategy } from "@/lib/api/autoMode";
import { proxyApi } from "@/lib/api/proxy";
import { failoverApi } from "@/lib/api/failover";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { extractErrorMessage } from "@/utils/errorUtils";
import { getAppLabel, type ProxyAppId } from "@/config/appConfig";
import { useProxyStatus } from "@/hooks/useProxyStatus";

/**
 * 某应用的省心模式状态（enabled 是按 app 的，strategy 是全局的）
 */
export function useAutoModeStatus(appType: string, enabled = true) {
  return useQuery({
    queryKey: ["autoModeStatus", appType],
    queryFn: () => autoModeApi.getStatus(appType),
    enabled: enabled && !!appType,
  });
}

/**
 * 开/关某应用的省心模式。
 *
 * 开启可能立即热切换到策略第一名（后端编排），成功后要刷新供应商列表与
 * 代理状态，让「当前供应商」的显示跟上实际服务目标。
 */
export function useSetAutoModeEnabled() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: ({ appType, enabled }: { appType: string; enabled: boolean }) =>
      autoModeApi.setEnabled(appType, enabled),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: ["autoModeStatus", variables.appType],
      });
      // 开启即切最优：当前供应商可能变了
      queryClient.invalidateQueries({
        queryKey: ["providers", variables.appType],
      });
      queryClient.invalidateQueries({ queryKey: ["proxyStatus"] });
      toast.success(
        variables.enabled
          ? t("autoMode.enabledToast", {
              app: getAppLabel(variables.appType),
              defaultValue: "{{app}} 省心模式已开启",
            })
          : t("autoMode.disabledToast", {
              app: getAppLabel(variables.appType),
              defaultValue: "{{app}} 省心模式已关闭",
            }),
      );
    },
    onError: (error) => {
      toast.error(
        t("autoMode.toggleFailed", {
          defaultValue: "操作失败",
        }) +
          ": " +
          extractErrorMessage(error),
      );
    },
  });
}

/**
 * 设置全局策略。策略从下一批请求生效（活跃会话被亲和保护，不会被立即打断），
 * 各 app 的状态缓存都要失效（strategy 字段是全局共享的）。
 */
export function useSetAutoModeStrategy() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: ({ strategy }: { strategy: AutoModeStrategy }) =>
      autoModeApi.setStrategy(strategy),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["autoModeStatus"] });
      toast.success(
        t("autoMode.strategySaved", { defaultValue: "策略已更新" }),
      );
    },
    onError: (error) => {
      toast.error(
        t("autoMode.toggleFailed", { defaultValue: "操作失败" }) +
          ": " +
          extractErrorMessage(error),
      );
    },
  });
}

/**
 * 设置模型偏好。后端会绕过会话亲和立即切到「目录含该模型、策略最优」的档位，
 * 当前供应商可能变化 —— 与开关同批失效。
 */
export function useSetAutoModeModel() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: ({
      appType,
      model,
    }: {
      appType: string;
      model: string | null;
    }) => autoModeApi.setModel(appType, model),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: ["autoModeStatus", variables.appType],
      });
      queryClient.invalidateQueries({
        queryKey: ["providers", variables.appType],
      });
      queryClient.invalidateQueries({ queryKey: ["proxyStatus"] });
    },
    onError: (error) => {
      toast.error(
        t("autoMode.toggleFailed", { defaultValue: "操作失败" }) +
          ": " +
          extractErrorMessage(error),
      );
    },
  });
}

/**
 * 单个 app 的开启编排（一键开启与总开关共用同一份顺序）：
 * 前置状态**现查**（`getProxyStatus`/`getProxyTakeoverStatus`）而不是读 hook
 * 缓存：点击时缓存可能还没加载完（顶栏刚挂载），拿着 undefined 判断会多余调
 * start，而已运行的服务再 start 会报 AlreadyRunning 把编排打断。
 * 起路由服务（若未运行）→ 接管该 app（若未接管）→ 开省心模式。
 * 任一步失败即抛出（各命令幂等，重试无害）。
 */
async function enableAutoModeApp(
  startProxyServer: () => Promise<unknown>,
  setTakeoverForApp: (input: {
    appType: string;
    enabled: boolean;
  }) => Promise<unknown>,
  appType: ProxyAppId,
): Promise<void> {
  const [status, takeover] = await Promise.all([
    proxyApi.getProxyStatus(),
    proxyApi.getProxyTakeoverStatus(),
  ]);
  if (!status.running) {
    await startProxyServer();
  }
  if (!(takeover?.[appType] ?? false)) {
    await setTakeoverForApp({ appType, enabled: true });
  }
  await autoModeApi.setEnabled(appType, true);
}

/**
 * 单个 app 的关闭编排：关省心模式时**默认把该 app 的路由接管一并关掉**
 * （恢复 CLI 的 live 配置；没有其它接管时代理服务会自动停）。接管是开启
 * 编排顺手开的，关的时候对称收回 —— 需要单独用路由的用户可以再开。
 */
async function disableAutoModeApp(
  setTakeoverForApp: (input: {
    appType: string;
    enabled: boolean;
  }) => Promise<unknown>,
  appType: ProxyAppId,
): Promise<void> {
  await autoModeApi.setEnabled(appType, false);
  const takeover = await proxyApi.getProxyTakeoverStatus();
  if (takeover?.[appType] ?? false) {
    await setTakeoverForApp({ appType, enabled: false });
  }
}

/**
 * 一键开启编排：省心模式要求「路由服务运行 + 该 app 接管」，此前用户得先去
 * 路由设置手动开两道前置 —— 现在这一步替他做。具体错误文案由各命令自己的
 * mutation toast 给出。
 */
export function useEnableAutoMode() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const { startProxyServer, setTakeoverForApp } = useProxyStatus();

  return useMutation({
    mutationFn: async ({ appType }: { appType: ProxyAppId }) => {
      await enableAutoModeApp(startProxyServer, setTakeoverForApp, appType);
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: ["autoModeStatus", variables.appType],
      });
      queryClient.invalidateQueries({
        queryKey: ["providers", variables.appType],
      });
      queryClient.invalidateQueries({ queryKey: ["proxyStatus"] });
      toast.success(
        t("autoMode.enabledToast", {
          app: getAppLabel(variables.appType),
          defaultValue: "{{app}} 省心模式已开启",
        }),
      );
    },
    onError: (error) => {
      toast.error(
        t("autoMode.toggleFailed", { defaultValue: "操作失败" }) +
          ": " +
          extractErrorMessage(error),
      );
    },
  });
}

/**
 * 关闭编排（关省心模式 + 默认收回该 app 的路由接管）。
 */
export function useDisableAutoMode() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const { setTakeoverForApp } = useProxyStatus();

  return useMutation({
    mutationFn: async ({ appType }: { appType: ProxyAppId }) => {
      await disableAutoModeApp(setTakeoverForApp, appType);
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: ["autoModeStatus", variables.appType],
      });
      queryClient.invalidateQueries({
        queryKey: ["providers", variables.appType],
      });
      queryClient.invalidateQueries({ queryKey: ["proxyStatus"] });
      toast.success(
        t("autoMode.disabledToast", {
          app: getAppLabel(variables.appType),
          defaultValue: "{{app}} 省心模式已关闭，路由接管已恢复",
        }),
      );
    },
    onError: (error) => {
      toast.error(
        t("autoMode.toggleFailed", { defaultValue: "操作失败" }) +
          ": " +
          extractErrorMessage(error),
      );
    },
  });
}

/**
 * 总开关：一次开/关多个 app 的省心模式（与单 app 同一份编排顺序，
 * 逐个执行、单个失败不影响其余 —— 剩下的下次再点即补齐）。
 */
export function useSetAutoModeAll() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const { startProxyServer, setTakeoverForApp } = useProxyStatus();

  return useMutation({
    mutationFn: async ({
      apps,
      enable,
    }: {
      apps: ProxyAppId[];
      enable: boolean;
    }) => {
      for (const appType of apps) {
        try {
          if (enable) {
            await enableAutoModeApp(
              startProxyServer,
              setTakeoverForApp,
              appType,
            );
          } else {
            await disableAutoModeApp(setTakeoverForApp, appType);
          }
        } catch (error) {
          // 单个 app 失败不停整批：各命令的 toast 已给出原因，
          // 其余 app 继续处理，总开关状态按实际结果刷新。
          console.error(`省心模式总开关处理 ${appType} 失败:`, error);
        }
      }
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ["autoModeStatus"] });
      queryClient.invalidateQueries({ queryKey: ["proxyStatus"] });
      queryClient.invalidateQueries({ queryKey: ["providers"] });
      toast.success(
        variables.enable
          ? t("autoMode.masterOnToast", {
              defaultValue: "省心模式已开启（有托管档位的应用）",
            })
          : t("autoMode.masterOffToast", {
              defaultValue: "省心模式已全部关闭，路由接管已恢复",
            }),
      );
    },
  });
}

/**
 * 统一的「自动故障转移」开关：一次设置全部应用（原先是每个 app 队列里各一个
 * 开关，重复且容易漏；队列本身仍按 app 各自管理）。
 */
export function useSetFailoverAll() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: async ({
      apps,
      enabled,
    }: {
      apps: string[];
      enabled: boolean;
    }) => {
      for (const appType of apps) {
        await failoverApi.setAutoFailoverEnabled(appType, enabled);
      }
    },
    onSuccess: (_, variables) => {
      for (const appType of variables.apps) {
        queryClient.invalidateQueries({
          queryKey: ["autoFailoverEnabled", appType],
        });
      }
      toast.success(
        variables.enabled
          ? t("proxy.failover.allOnToast", {
              defaultValue: "自动故障转移已全部开启",
            })
          : t("proxy.failover.allOffToast", {
              defaultValue: "自动故障转移已全部关闭",
            }),
      );
    },
    onError: (error) => {
      toast.error(
        t("autoMode.toggleFailed", { defaultValue: "操作失败" }) +
          ": " +
          extractErrorMessage(error),
      );
    },
  });
}
