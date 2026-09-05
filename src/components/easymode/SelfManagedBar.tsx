/**
 * 省心视图置顶的「官方与自建」栏：不参与价格排序的**退出坡道**。
 *
 * 省心模式只在托管档位间选路；官方 OAuth / 自建供应商不进候选池。这条栏
 * 让它们在省心视图下保持可见可达——点击 = 退省心（收接管）→ 切换到所选
 * 供应商，该 app 随即转为自主模式（segmented 会跟着状态翻过去）。切换的
 * 「先退 ChatGPT」确认复用 useCodexSwitchGuard，与 provider 页同一形状。
 */

import { useTranslation } from "react-i18next";
import { isProxyAppId } from "@/config/appConfig";
import { useProvidersQuery } from "@/lib/query/queries";
import { isManagedProviderId } from "@/config/managedProviderId";
import { useSwitchToSelfManaged } from "@/lib/query/autoMode";
import { useCodexSwitchGuard } from "@/components/relay/useCodexSwitchGuard";

export function SelfManagedBar({ appId }: { appId: string }) {
  const { t } = useTranslation();
  // 运行时收窄而不是 as 断言：这个组件只会在路由 app 的省心视图里渲染
  if (!isProxyAppId(appId)) {
    return null;
  }
  const { data } = useProvidersQuery(appId);
  const switchBack = useSwitchToSelfManaged();
  const { guardedSwitch, switchDialog } = useCodexSwitchGuard(
    (provider, quitChatgpt) =>
      switchBack.mutateAsync({
        appType: appId,
        providerId: provider.id,
        quitChatgpt,
      }),
  );

  const selfManaged = Object.values(data?.providers ?? {}).filter(
    (provider) => !isManagedProviderId(provider.id),
  );

  return (
    <div className="rounded-lg border bg-card p-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="shrink-0 text-xs font-medium text-muted-foreground">
          {t("autoMode.board.selfManagedTitle", {
            defaultValue: "官方与自建",
          })}
        </span>
        {selfManaged.length === 0 ? (
          <span className="text-xs text-muted-foreground">
            {t("autoMode.board.selfManagedEmpty", {
              defaultValue: "还没有官方或自建供应商（在自主模式添加）",
            })}
          </span>
        ) : (
          selfManaged.map((provider) => (
            <button
              key={provider.id}
              type="button"
              disabled={switchBack.isPending}
              onClick={() => void guardedSwitch(provider)}
              title={t("autoMode.board.switchBack", { defaultValue: "切回" })}
              className="rounded-full border px-2.5 py-1 text-xs transition-colors hover:bg-accent disabled:opacity-50"
            >
              {provider.name}
            </button>
          ))
        )}
      </div>
      {switchDialog}
    </div>
  );
}
