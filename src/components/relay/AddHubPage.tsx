import { QoderAccountPanel } from "./accounts/QoderAccountPanel";
import { AccountApplicationNavigation } from "./accounts/AccountWorkspace";
import {
  accountPlatforms,
  accountPlatformLabel,
  type AccountPlatform,
} from "./accounts/accountPlatforms";
import {
  serviceEntryIntent,
  type ServiceEntryIntent,
} from "./accounts/serviceEntryIntent";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { AppId } from "@/lib/api";
import {
  PreservedView,
  usePreservedViewActive,
} from "@/components/ui/PreservedView";
import { ServiceConfiguration } from "./onboarding/ServiceConfiguration";
import type { ConnectedService } from "./onboarding/useServiceOnboarding";
import {
  AddProviderForm,
  type AddProviderFormProps,
} from "@/components/providers/AddProviderForm";

import { RelayDirectoryPage } from "./directory/RelayDirectoryPage";
import { OfficialApiPage } from "./OfficialApiPage";

export type AddHubTab = "accounts" | "directory" | "official" | "manual";

export function AddHubPage({
  sourceAppId,
  initialTab = "accounts",
  onOpenPlatform,
  onOpenApp,
  onBack,
  onAddProvider,
  firstVisit = false,
  entryRequest,
}: {
  sourceAppId: AppId;
  onOpenPlatform?: (platform: AccountPlatform) => void;
  onOpenApp?: (app: AppId) => void;
  /** 统一添加默认先选平台；首启与旧直达入口显式指定原标签。 */
  initialTab?: AddHubTab;
  onBack: () => void;
  onAddProvider: AddProviderFormProps["onSubmit"];
  /** 首次连接弹窗由后端引导状态决定。 */
  firstVisit?: boolean;
  /** Increment id for an explicit destination request; history navigation keeps it unchanged. */
  entryRequest?: { id: number; tab: AddHubTab; intent?: ServiceEntryIntent };
}) {
  const { t } = useTranslation();
  const viewActive = usePreservedViewActive();
  const entryFocus = useRef<HTMLDivElement>(null);
  const [sourceIntent, setSourceIntent] = useState<
    ServiceEntryIntent | undefined
  >(entryRequest?.intent);
  const [sourceRevision, setSourceRevision] = useState(0);
  const sourceGeneration = useRef(0);
  // Saving belongs to a visibility lifetime, while sourceRevision preserves navigation.
  const saveGeneration = useRef(0);
  const advanceDestination = () => {
    ++saveGeneration.current;
    return ++sourceGeneration.current;
  };
  const active = useRef(true);
  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
      ++saveGeneration.current;
    };
  }, []);
  const leaveHub = () => {
    advanceDestination();
    onBack();
  };
  const closeSource = (revision: number) => {
    if (!active.current || revision !== sourceGeneration.current) return false;
    advanceDestination();
    setSourceIntent(undefined);
    return true;
  };
  const openSource = (intent: ServiceEntryIntent) => {
    setSourceIntent(intent);
    setSourceRevision(advanceDestination());
  };
  const [domain, setDomain] = useState("");
  const [account, setAccount] = useState<ConnectedService | null>(null);
  const [configuring, setConfiguring] = useState(false);
  const [manualRevision, setManualRevision] = useState(0);
  const completeConnection = () => {
    setAccount(null);
    setConfiguring(false);
    setDomain("");
    leaveHub();
  };
  const connected = (value: ConnectedService) => {
    setAccount(value);
    setConfiguring(true);
  };

  const [tab, setTab] = useState<AddHubTab>(entryRequest?.tab ?? initialTab);
  const [appliedEntryId, setAppliedEntryId] = useState(entryRequest?.id);
  if (entryRequest && entryRequest.id !== appliedEntryId) {
    setAppliedEntryId(entryRequest.id);
    setTab(entryRequest.tab);
    setSourceIntent(entryRequest.intent);
    setSourceRevision(advanceDestination());
    setConfiguring(false);
  }

  useEffect(() => {
    if (viewActive && !configuring && !sourceIntent)
      entryFocus.current?.focus();
  }, [viewActive, configuring, sourceIntent, entryRequest?.id]);

  return (
    <div
      ref={entryFocus}
      tabIndex={-1}
      role="region"
      aria-label={t("loongport.addEntry.title", {
        defaultValue: "添加服务与账号",
      })}
      className="page-content h-full overflow-auto focus:outline-none"
    >
      {sourceIntent && (
        <section className="space-y-4" aria-label="服务接入">
          <Button variant="ghost" onClick={() => closeSource(sourceRevision)}>
            返回平台列表
          </Button>
          {sourceIntent.kind === "account-boundary" ? (
            <AccountApplicationNavigation.Provider
              value={
                onOpenApp ? (app = sourceAppId) => onOpenApp(app) : undefined
              }
            >
              <QoderAccountPanel defaultApp={sourceAppId} />
            </AccountApplicationNavigation.Provider>
          ) : sourceIntent.kind === "vendor" ? (
            <OfficialApiPage
              key={`source-${sourceRevision}`}
              sourceAppId={sourceAppId}
              vendorId={sourceIntent.vendorId}
              onBack={() => closeSource(sourceRevision)}
              onConnected={(value) => {
                if (closeSource(sourceRevision)) connected(value);
              }}
              embedded
            />
          ) : (
            <AddProviderForm
              key={`source-${sourceRevision}`}
              appId={sourceAppId}
              initialPresetName={sourceIntent.preset}
              onSubmit={onAddProvider}
              onSaveStart={() => saveGeneration.current}
              onDone={(reason, saveEpoch) => {
                if (reason !== "saved" || saveEpoch === saveGeneration.current)
                  closeSource(sourceRevision);
              }}
            />
          )}
        </section>
      )}
      {account && (
        <PreservedView active={configuring && !sourceIntent}>
          <ServiceConfiguration
            key={`${account.kind}:${account.rowId}`}
            account={account}
            sourceAppId={sourceAppId}
            onBack={() => setConfiguring(false)}
            onDone={completeConnection}
          />
        </PreservedView>
      )}
      <PreservedView active={!configuring && !sourceIntent}>
        <Tabs
          value={tab}
          onValueChange={(v) => {
            advanceDestination();
            setTab(v as AddHubTab);
          }}
          className="flex min-h-full w-full flex-col"
        >
          <div className="flex shrink-0 flex-wrap items-center gap-4 border-b border-border-default pb-5">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-8 shrink-0"
              onClick={leaveHub}
              aria-label={t("common.back")}
            >
              <ArrowLeft className="h-4 w-4" />
              {t("common.back")}
            </Button>
            <TabsList className="flex h-auto min-w-0 max-w-full flex-wrap justify-start">
              {sourceAppId !== "mcode" && (
                <>
                  <TabsTrigger value="accounts">
                    {t("zcode.accounts.login.account", {
                      defaultValue: "账号",
                    })}
                  </TabsTrigger>
                  <TabsTrigger value="directory">
                    {t("loongport.sections.relay")}
                  </TabsTrigger>
                  <TabsTrigger value="official">
                    {t("loongport.sections.official")}
                  </TabsTrigger>
                </>
              )}
              <TabsTrigger value="manual">
                <Plus className="mr-1.5 h-3.5 w-3.5" />
                {t("loongport.addEntry.manual")}
              </TabsTrigger>
            </TabsList>
            {account && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => setConfiguring(true)}
              >
                {t("loongport.onboarding.resume")}
              </Button>
            )}
          </div>

          <TabsContent value="accounts" className="mt-4">
            <div className="grid gap-2 sm:grid-cols-2">
              {accountPlatforms.map((platform) => (
                <Button
                  key={platform.id}
                  variant="outline"
                  onClick={() => {
                    const intent = serviceEntryIntent(platform.id);
                    if (intent) openSource(intent);
                    else onOpenPlatform?.(platform.id);
                  }}
                >
                  {platform.id === "openrouter"
                    ? "OpenRouter · API Key"
                    : accountPlatformLabel(platform)}
                </Button>
              ))}
            </div>
          </TabsContent>
          <TabsContent
            value="directory"
            forceMount
            className="mt-0 min-h-0 flex-1 data-[state=inactive]:hidden"
          >
            <PreservedView active={tab === "directory"}>
              <RelayDirectoryPage
                sourceAppId={sourceAppId}
                onBack={leaveHub}
                embedded
                firstVisit={firstVisit}
                domain={domain}
                onDomainChange={setDomain}
                onOfficial={() => setTab("official")}
                onConnected={connected}
              />
            </PreservedView>
          </TabsContent>

          <TabsContent
            value="official"
            forceMount
            className="mt-0 min-h-0 flex-1 data-[state=inactive]:hidden"
          >
            <PreservedView active={tab === "official"}>
              <OfficialApiPage
                sourceAppId={sourceAppId}
                onBack={leaveHub}
                onConnected={connected}
                embedded
              />
            </PreservedView>
          </TabsContent>

          <TabsContent
            value="manual"
            forceMount
            className="mt-0 min-h-0 flex-1 pt-6 data-[state=inactive]:hidden"
          >
            <PreservedView active={tab === "manual"}>
              <AddProviderForm
                key={manualRevision}
                appId={sourceAppId}
                onSaveStart={() => saveGeneration.current}
                onSubmit={onAddProvider}
                onDone={(reason, saveEpoch) => {
                  if (
                    reason !== "saved" ||
                    (active.current && saveEpoch === saveGeneration.current)
                  ) {
                    if (reason === "saved")
                      setManualRevision((value) => value + 1);
                    leaveHub();
                  }
                }}
              />
            </PreservedView>
          </TabsContent>
        </Tabs>
      </PreservedView>
    </div>
  );
}
