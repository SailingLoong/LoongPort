import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  autoModeApi,
  type AutoModeStrategy,
  type EasyModeMode,
} from "@/lib/api/autoMode";
import { proxyApi } from "@/lib/api/proxy";
import { providersApi } from "@/lib/api/providers";
import { failoverApi } from "@/lib/api/failover";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  getAppLabel,
  PROXY_APP_IDS,
  type ProxyAppId,
} from "@/config/appConfig";
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
 * 当前开着省心模式的 app 集合（四个路由 app 的状态并集，同一批 query 缓存）。
 *
 * 消费方是「功能是否可用」的资格判断（如扣费对账：估算的原料是带档位归因的
 * 本地路由流量，没有省心模式就没有数据）。查不到状态时按「没开」处理 ——
 * 资格判断宁可保守。
 */
export function useEasyModeApps(): Set<string> {
  const claude = useAutoModeStatus("claude").data;
  const codex = useAutoModeStatus("codex").data;
  const gemini = useAutoModeStatus("gemini").data;
  const grokbuild = useAutoModeStatus("grokbuild").data;
  const apps = new Set<string>();
  if (claude?.enabled) apps.add("claude");
  if (codex?.enabled) apps.add("codex");
  if (gemini?.enabled) apps.add("gemini");
  if (grokbuild?.enabled) apps.add("grokbuild");
  return apps;
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
      // 看板的顺序/理由随策略变，全局策略 → 全部看板失效
      queryClient.invalidateQueries({ queryKey: ["easyModeTierBoard"] });
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
      // 偏好决定有效模型/单价与候选过滤
      queryClient.invalidateQueries({
        queryKey: ["easyModeTierBoard", variables.appType],
      });
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
 * 省心视图「切回官方/自建」：退省心（收接管）→ 切换到指定供应商。
 *
 * 顺序是硬的：官方/自建供应商的切换在接管态下会被拦（CLI 流量都走本地
 * 代理），必须先把该 app 的省心关掉、恢复接管配置，再走正常切换写 live。
 * confirmationRequired（ChatGPT 桌面版先退再切）原样透传给调用方，
 * 由 useCodexSwitchGuard 的弹窗处理后带 quitChatgpt 重试。
 */
export function useSwitchToSelfManaged() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const disableFlow = useDisableAutoMode();

  return useMutation({
    mutationFn: async ({
      appType,
      providerId,
      quitChatgpt,
    }: {
      appType: ProxyAppId;
      providerId: string;
      quitChatgpt?: boolean;
    }) => {
      await disableFlow.mutateAsync({ appType }).catch(() => undefined);
      return providersApi.switch(providerId, appType, quitChatgpt);
    },
    onSuccess: (result, variables) => {
      queryClient.invalidateQueries({
        queryKey: ["autoModeStatus", variables.appType],
      });
      queryClient.invalidateQueries({
        queryKey: ["providers", variables.appType],
      });
      queryClient.invalidateQueries({ queryKey: ["proxyStatus"] });
      if (result?.status !== "confirmationRequired") {
        toast.success(
          t("autoMode.board.switchBackDone", {
            defaultValue: "已切回，该应用转为自主模式",
          }),
        );
      }
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
 * 省心模式**总开关**（2026-08-17 定稿语义）：它就是「是否开启省心模式」，
 * 也就是「是否开启本地路由」—— 只有大开关开着，各 app 卡片的细节数据才
 * 可配置。
 *
 * - **开**：起路由服务（已在跑则幂等跳过）。不动任何 app —— 各 app 的
 *   省心模式由卡片逐个开（开卡时自动接管）。
 * - **关**：把所有开着省心模式的 app 关掉（raw API，静默容错），然后
 *   `stopWithRestore` —— 恢复全部接管配置并停服务，路由彻底回到关闭态。
 */
export function useSetEasyModeMaster() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const { startProxyServer, stopWithRestore } = useProxyStatus();

  return useMutation({
    mutationFn: async ({ enable }: { enable: boolean }) => {
      const status = await proxyApi.getProxyStatus();
      if (enable) {
        if (!status.running) {
          await startProxyServer();
        }
        return;
      }
      // 关：先落各 app 的省心模式开关（失败不拦停，路由该关还得关），
      // 再整体恢复 + 停服务（stopWithRestore 会还原所有接管配置）。
      for (const appType of PROXY_APP_IDS) {
        try {
          const appStatus = await autoModeApi.getStatus(appType);
          if (appStatus.enabled) {
            await autoModeApi.setEnabled(appType, false);
          }
        } catch (error) {
          console.warn(`省心模式总开关关闭 ${appType} 失败（继续）:`, error);
        }
      }
      await stopWithRestore();
    },
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: ["autoModeStatus"] });
      queryClient.invalidateQueries({ queryKey: ["proxyStatus"] });
      queryClient.invalidateQueries({ queryKey: ["providers"] });
      toast.success(
        variables.enable
          ? t("autoMode.masterOnToast", {
              defaultValue:
                "省心模式已开启（本地路由运行中），现在可以为各应用打开了",
            })
          : t("autoMode.masterOffToast", {
              defaultValue: "省心模式已关闭，路由已停并恢复全部配置",
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

/**
 * 档位看板（首页省心视图数据源）：顺序/倍率/单价/耗时/命中/余额一次拉全。
 * 余额链含站点查询，给个短 staleTime 避免频繁切 app 时反复打站点。
 */
export function useTierBoard(appType: string, enabled = true) {
  return useQuery({
    queryKey: ["easyModeTierBoard", appType],
    queryFn: () => autoModeApi.getTierBoard(appType),
    enabled: enabled && !!appType,
    staleTime: 30_000,
  });
}

/**
 * 切选路模式（自动/手动）。后端首切手动会快照当前序为初始清单。
 */
export function useSetEasyModeMode() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: ({ appType, mode }: { appType: string; mode: EasyModeMode }) =>
      autoModeApi.setEasyModeMode(appType, mode),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: ["easyModeTierBoard", variables.appType],
      });
      queryClient.invalidateQueries({
        queryKey: ["autoModeStatus", variables.appType],
      });
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
 * 写手动档位顺序（拖拽落定后整份提交，后端按清单序选路）。
 */
export function useSetEasyModeManualOrder() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  return useMutation({
    mutationFn: ({
      appType,
      orderedIds,
    }: {
      appType: string;
      orderedIds: string[];
    }) => autoModeApi.setEasyModeManualOrder(appType, orderedIds),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: ["easyModeTierBoard", variables.appType],
      });
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
