import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { autoModeApi, type AutoModeStrategy } from "@/lib/api/autoMode";
import { proxyApi } from "@/lib/api/proxy";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { extractErrorMessage } from "@/utils/errorUtils";
import { getAppLabel, type ProxyAppId } from "@/config/appConfig";
import { useProxyStatus } from "@/hooks/useProxyStatus";

/**
 * 某应用的自动模式状态（enabled 是按 app 的，strategy 是全局的）
 */
export function useAutoModeStatus(appType: string, enabled = true) {
  return useQuery({
    queryKey: ["autoModeStatus", appType],
    queryFn: () => autoModeApi.getStatus(appType),
    enabled: enabled && !!appType,
  });
}

/**
 * 开/关某应用的自动模式。
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
              defaultValue: "{{app}} 自动模式已开启",
            })
          : t("autoMode.disabledToast", {
              app: getAppLabel(variables.appType),
              defaultValue: "{{app}} 自动模式已关闭",
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
 * 一键开启编排：自动模式要求「路由服务运行 + 该 app 接管」，此前用户得先去
 * 路由设置手动开两道前置 —— 现在这一步替他做。
 *
 * 前置状态**现查**（`getProxyStatus`/`getProxyTakeoverStatus`）而不是读 hook
 * 缓存：点击时缓存可能还没加载完（顶栏刚挂载），拿着 undefined 判断会多余调
 * start，而已运行的服务再 start 会报 AlreadyRunning 把编排打断。顺序执行
 * 既有命令：起路由服务（若未运行）→ 接管该 app（若未接管）→ 开自动模式。
 * 任一步失败即停（各命令幂等，重试无害），具体错误文案由各命令自己的
 * mutation toast 给出。
 */
export function useEnableAutoMode() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const { startProxyServer, setTakeoverForApp } = useProxyStatus();

  return useMutation({
    mutationFn: async ({ appType }: { appType: ProxyAppId }) => {
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
          defaultValue: "{{app}} 自动模式已开启",
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
