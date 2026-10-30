import { useQoderAccountPanel } from "./useQoderAccountPanel";
import { useContext } from "react";
import {
  AccountWorkspace,
  AccountDetailSections,
  AccountApplicationNavigation,
} from "./AccountWorkspace";
import { QoderApplicationEntry } from "./QoderApplicationEntry";
import { Button } from "@/components/ui/button";
import { QoderInferenceIdentity } from "@/components/providers/qoder/QoderInferenceIdentity";
import { QoderCnDeviceLogin } from "@/components/providers/qoder/QoderCnDeviceLogin";
import { QoderDailyClaim } from "@/components/providers/qoder/QoderDailyClaim";
import { QoderCnCampaigns } from "@/components/providers/qoder/QoderCnCampaigns";
import type { AppId } from "@/lib/api/types";

/** Present original Provider records, never create a second account or credential owner. */
export function QoderAccountPanel({ defaultApp }: { defaultApp?: AppId }) {
  const { active, accounts, details, pending, error, refreshing, refresh } =
    useQoderAccountPanel();
  const openApplication = useContext(AccountApplicationNavigation);
  return (
    <section className="space-y-4" aria-label="Qoder 账号管理">
      <div className="flex items-center justify-between gap-3">
        <h2 className="font-semibold">Qoder 账号（国际版 / CN）</h2>
        <Button
          variant="outline"
          disabled={!active || refreshing}
          onClick={() => void refresh()}
        >
          刷新账号
        </Button>
      </div>
      <QoderCnDeviceLogin disabled={!active} />
      <p className="text-sm text-muted-foreground">
        这里管理原有 Qoder 配置，不会以自定义 API 或 SDK
        配置代替账号接入。保存配置不代表推理已接通。
      </p>
      <p className="text-sm text-muted-foreground">
        HTTP
        推理需先主动建立账号身份，再通过真实请求验证权益；积分领取独立验证国际版
        PAT、活动资格及设备组件，不代表已接通积分消费。
      </p>
      {pending ? (
        <p role="status">正在读取账号…</p>
      ) : error ? (
        <div role="alert">
          <p>Qoder 账号读取失败，请重试。</p>
          <Button variant="outline" onClick={() => void refresh()}>
            重试
          </Button>
        </div>
      ) : accounts.length === 0 ? (
        <p>
          尚无已保存的 Qoder 配置。原配置由 Codex 应用的 Qoder 国际版或 CN
          供应商入口管理。
        </p>
      ) : (
        <AccountWorkspace
          accounts={accounts}
          showApplicationNavigation={false}
          label={(provider) => provider.name}
          identityDetails={(provider) => (
            <p className="mt-1 text-xs text-muted-foreground">
              {details(provider).identityLabel}
            </p>
          )}
        >
          {(provider) => {
            const { generation, usesDeviceLogin, isInternational } =
              details(provider);
            return (
              <>
                <AccountDetailSections
                  authorization={
                    usesDeviceLogin ? (
                      <QoderCnDeviceLogin
                        providerId={provider.id}
                        sourceGeneration={generation}
                        disabled={!active}
                      />
                    ) : (
                      <p className="text-sm text-muted-foreground">
                        授权与连接操作沿用原配置，请在“Qoder
                        推理身份”中查看实际状态。
                      </p>
                    )
                  }
                  entitlements={
                    isInternational ? (
                      <QoderDailyClaim
                        providerId={provider.id}
                        sourceGeneration={generation}
                        disabled={!active || refreshing}
                      />
                    ) : (
                      <>
                        <QoderCnCampaigns
                          providerId={provider.id}
                          sourceGeneration={generation}
                          disabled={!active || refreshing}
                        />
                        <p className="text-sm text-muted-foreground">
                          免费福利可在“CN
                          活动状态”中单独领取，自动领取需单独开启；活动已领取不代表今日签到成功。
                        </p>
                      </>
                    )
                  }
                  inference={
                    <QoderInferenceIdentity
                      providerId={provider.id}
                      sourceGeneration={generation}
                      disabled={!active || refreshing}
                    />
                  }
                />
                <QoderApplicationEntry
                  provider={provider}
                  sourceGeneration={generation}
                  disabled={!active}
                  onOpenApp={openApplication}
                  defaultApp={defaultApp}
                  onOpenDefault={
                    openApplication ? () => openApplication() : undefined
                  }
                />
              </>
            );
          }}
        </AccountWorkspace>
      )}
    </section>
  );
}
