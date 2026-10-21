import { usePreservedViewActive } from "@/components/ui/PreservedView";
import { useQuery } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { providersApi } from "@/lib/api/providers";
import { QoderInferenceIdentity } from "@/components/providers/qoder/QoderInferenceIdentity";
import { QoderDailyClaim } from "@/components/providers/qoder/QoderDailyClaim";
import type { Provider } from "@/types";

function sourceGeneration(provider: Provider): string | undefined {
  const state = (provider.meta as { qoderCampaignState?: unknown } | undefined)
    ?.qoderCampaignState;
  if (!state || typeof state !== "object" || !("generation" in state))
    return undefined;
  return typeof state.generation === "string" ? state.generation : undefined;
}

/** Present original Provider records, never create a second account or credential owner. */
export function QoderAccountPanel() {
  const active = usePreservedViewActive();
  const query = useQuery({
    queryKey: ["providers", "codex"],
    enabled: active,
    queryFn: async () => ({ providers: await providersApi.getAll("codex") }),
  });
  const accounts = Object.values(query.data?.providers ?? {}).filter(
    (provider) => provider.meta?.providerType === "qoder_sdk",
  );
  return (
    <section className="space-y-4" aria-label="Qoder 账号管理">
      <div className="flex items-center justify-between gap-3">
        <h2 className="font-semibold">Qoder 国际版账号</h2>
        <Button
          variant="outline"
          disabled={!active || query.isFetching}
          onClick={() => void query.refetch()}
        >
          刷新账号
        </Button>
      </div>
      <p className="text-sm text-muted-foreground">
        这里管理原有 Qoder 配置，不会以自定义 API 或 SDK
        配置代替账号接入。保存配置不代表推理已接通。
      </p>
      <p className="text-sm text-muted-foreground">
        HTTP
        推理需先主动建立账号身份，再通过真实请求验证权益；积分领取独立验证国际版
        PAT、活动资格及设备组件，不代表已接通积分消费。
      </p>
      {query.isPending ? (
        <p role="status">正在读取账号…</p>
      ) : query.error ? (
        <div role="alert">
          <p>Qoder 账号读取失败，请重试。</p>
          <Button variant="outline" onClick={() => void query.refetch()}>
            重试
          </Button>
        </div>
      ) : accounts.length === 0 ? (
        <p>
          尚无已保存的 Qoder 配置。原配置由 Codex 应用的 Qoder
          国际版供应商入口管理。
        </p>
      ) : (
        <div className="space-y-3">
          {accounts.map((provider) => {
            const generation = sourceGeneration(provider);
            return (
              <article
                key={`${provider.id}:${generation ?? ""}`}
                className="rounded-xl border p-4 space-y-3"
              >
                <h3 className="font-medium">{provider.name}</h3>
                <div className="flex flex-wrap gap-2">
                  <QoderInferenceIdentity
                    providerId={provider.id}
                    sourceGeneration={generation}
                    disabled={!active || query.isFetching}
                  />
                  <QoderDailyClaim
                    providerId={provider.id}
                    disabled={!active || query.isFetching}
                  />
                </div>
              </article>
            );
          })}
        </div>
      )}
    </section>
  );
}
