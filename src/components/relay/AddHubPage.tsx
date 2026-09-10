import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { AppId } from "@/lib/api";
import { PreservedView } from "@/components/ui/PreservedView";
import { ServiceConfiguration } from "./onboarding/ServiceConfiguration";
import type { ConnectedService } from "./onboarding/useServiceOnboarding";
import {
  AddProviderForm,
  type AddProviderFormProps,
} from "@/components/providers/AddProviderForm";

import { RelayDirectoryPage } from "./directory/RelayDirectoryPage";
import { OfficialApiPage } from "./OfficialApiPage";

export type AddHubTab = "directory" | "official" | "manual";

export function AddHubPage({
  sourceAppId,
  initialTab = "directory",
  onBack,
  onAddProvider,
  firstVisit = false,
  entryRequest,
}: {
  sourceAppId: AppId;
  /** 进来落在哪个标签；顶栏大「+」与首启引导不传（落默认「中转站」）。 */
  initialTab?: AddHubTab;
  onBack: () => void;
  onAddProvider: AddProviderFormProps["onSubmit"];
  /** 首次连接弹窗由后端引导状态决定。 */
  firstVisit?: boolean;
  /** Increment id for an explicit destination request; history navigation keeps it unchanged. */
  entryRequest?: { id: number; tab: AddHubTab };
}) {
  const { t } = useTranslation();
  const [domain, setDomain] = useState("");
  const [account, setAccount] = useState<ConnectedService | null>(null);
  const [configuring, setConfiguring] = useState(false);
  const [manualRevision, setManualRevision] = useState(0);
  const completeConnection = () => {
    setAccount(null);
    setConfiguring(false);
    setDomain("");
    onBack();
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
    setConfiguring(false);
  }

  return (
    <div className="page-content h-full overflow-auto">
      {account && (
        <PreservedView active={configuring}>
          <ServiceConfiguration
            key={`${account.kind}:${account.rowId}`}
            account={account}
            sourceAppId={sourceAppId}
            onBack={() => setConfiguring(false)}
            onDone={completeConnection}
          />
        </PreservedView>
      )}
      <PreservedView active={!configuring}>
        <Tabs
          value={tab}
          onValueChange={(v) => setTab(v as AddHubTab)}
          className="flex min-h-full w-full flex-col"
        >
          <div className="flex shrink-0 flex-wrap items-center gap-4 border-b border-border-default pb-5">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-8 shrink-0"
              onClick={onBack}
              aria-label={t("common.back")}
            >
              <ArrowLeft className="h-4 w-4" />
              {t("common.back")}
            </Button>
            <TabsList>
              <TabsTrigger value="directory">
                {t("loongport.sections.relay")}
              </TabsTrigger>
              <TabsTrigger value="official">
                {t("loongport.sections.official")}
              </TabsTrigger>
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

          <TabsContent
            value="directory"
            forceMount
            className="mt-0 min-h-0 flex-1 data-[state=inactive]:hidden"
          >
            <PreservedView active={tab === "directory"}>
              <RelayDirectoryPage
                sourceAppId={sourceAppId}
                onBack={onBack}
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
                onBack={onBack}
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
                onSubmit={async (values) => {
                  await onAddProvider(values);
                  setManualRevision((value) => value + 1);
                }}
                onDone={onBack}
              />
            </PreservedView>
          </TabsContent>
        </Tabs>
      </PreservedView>
    </div>
  );
}
