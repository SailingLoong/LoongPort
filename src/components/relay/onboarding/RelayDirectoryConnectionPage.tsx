import { useState } from "react";
import { ArrowLeft } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AppId } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { RelayDirectoryPage } from "../directory/RelayDirectoryPage";
import { PreservedView } from "@/components/ui/PreservedView";
import { ServiceConfiguration } from "./ServiceConfiguration";
import type { ConnectedService } from "./useServiceOnboarding";

/** Standalone directory route, including its connection and configuration steps. */
export function RelayDirectoryConnectionPage({
  sourceAppId,
  onBack,
}: {
  sourceAppId: AppId;
  onBack: () => void;
}) {
  const { t } = useTranslation();
  const [account, setAccount] = useState<ConnectedService | null>(null);
  const [configuring, setConfiguring] = useState(false);
  const [domain, setDomain] = useState("");
  return (
    <div className="h-full overflow-auto px-6">
      {account && (
        <PreservedView active={configuring}>
          <ServiceConfiguration
            key={`${account.kind}:${account.rowId}`}
            account={account}
            sourceAppId={sourceAppId}
            onBack={() => setConfiguring(false)}
            onDone={() => {
              setAccount(null);
              setConfiguring(false);
              setDomain("");
              onBack();
            }}
          />
        </PreservedView>
      )}
      <PreservedView active={!configuring}>
        <Button variant="ghost" className="mt-4" onClick={onBack}>
          <ArrowLeft className="mr-2 h-4 w-4" />
          {t("common.back")}
        </Button>
        {account && (
          <Button
            variant="outline"
            className="mt-4"
            onClick={() => setConfiguring(true)}
          >
            {t("loongport.onboarding.resume")}
          </Button>
        )}
        <RelayDirectoryPage
          sourceAppId={sourceAppId}
          domain={domain}
          onDomainChange={setDomain}
          onBack={onBack}
          embedded
          onConnected={(value) => {
            setAccount(value);
            setConfiguring(true);
          }}
        />
      </PreservedView>
    </div>
  );
}
