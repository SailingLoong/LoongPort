import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { autoModeApi, type AutoModeStrategy } from "@/lib/api/autoMode";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { extractErrorMessage } from "@/utils/errorUtils";
import { getAppLabel } from "@/config/appConfig";

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
