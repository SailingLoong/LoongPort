import { useState } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { ClientSidebar } from "@/components/shell/ClientSidebar";
import { PAGE_HEADER_HEIGHT } from "@/components/shell/layout";
import { ServicesPage } from "@/components/relay/accounts/ServicesPage";
import { AddHubPage, type AddHubTab } from "@/components/relay/AddHubPage";
import type { AccountPlatform } from "@/components/relay/accounts/accountPlatforms";
import type { RelayRow } from "@/lib/api/relay";
import type { VendorAccountRow } from "@/lib/api/vendor";
import type { MiniMaxCodeAccount } from "@/lib/api/minimaxCode";
import { Button } from "@/components/ui/button";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import "@/index.css";

// Browser-only presentation fixture: no native configuration, login or writes.
const params = new URLSearchParams(location.search);
const scenario = params.get("scenario") ?? "populated";
const language = params.get("lang") === "zh" ? "zh" : "en";
const rejectedCommands: string[] = [];
Object.assign(window, { unifiedLayout: { rejectedCommands } });
const relay: RelayRow = {
  id: 1,
  siteOrigin: "https://relay.example",
  siteName: "Synthetic relay with a deliberately long display name",
  accountLabel: "Synthetic relay account",
  status: "notLoggedIn",
  isCurrent: false,
  canQueryBalance: false,
  canPurchase: false,
  canViewUsage: false,
  canRefresh: false,
  usageBlockers: [],
  removeConfirmation: "neverLoggedIn",
  tiers: [],
};
const vendor: VendorAccountRow = {
  id: 2,
  vendorId: "opencode",
  vendorName: "Synthetic OpenCode workspace with a long label",
  accountLabel: "Synthetic workspace account",
  status: "sessionExpiredUsable",
  canQueryBalance: false,
  canRefresh: false,
  canDelete: true,
  plans: [],
};
const nativeAccounts: MiniMaxCodeAccount[] = ["A", "B"].map((id) => ({
  id: `synthetic-${id}`,
  label: `Synthetic account ${id}`,
  region: "cn",
  buildEnv: "prod",
  realUserId: `synthetic-user-${id}`,
  authGeneration: 1,
  credentialVersion: 1,
  authorized: true,
  status: "unknown",
  canClaim: false,
  canRefresh: true,
  canRecheck: false,
  canRetire: true,
  operation: null,
  lastCheckedAt: null,
  lastObservedStatus: null,
  creditAmount: null,
}));
mockIPC(
  (command, payload) => {
    switch (command) {
      case "relay_list_relays":
        return scenario === "empty" ? [] : [relay];
      case "vendor_list_accounts":
        return {
          supported: true,
          accounts: scenario === "empty" ? [] : [vendor],
        };
      case "list_minimax_code_accounts":
        return nativeAccounts;
      case "auth_get_status":
        return {
          provider: (payload as { authProvider: string }).authProvider,
          authenticated: false,
          default_account_id: null,
          accounts: [],
        };
      case "get_application_routing":
        return {
          autoFailoverEnabled: false,
          routingActive: false,
          model: null,
          modelOptions: [],
          chainIds: [],
          tiers: [],
        };
      case "preset_referral_urls":
        return {};
      case "get_common_config_snippet":
        return null;
      case "get_providers":
        return {};
      case "get_service_sources":
        return [];
      case "relay_list_directory":
        return { items: [], syncedAt: 0 };
      case "get_settings":
        return {
          language,
          commonConfigConfirmed: true,
          plazaVisible: true,
          crowdMetricsEnabled: false,
        };
      case "plugin:path|resolve_directory":
      case "plugin:path|join":
      case "get_config_dir":
        return "/synthetic/config";
      case "get_app_config_dir_override":
        return null;
      case "is_portable_mode":
        return false;
      default:
        rejectedCommands.push(command);
        throw new Error(`Unexpected synthetic fixture command: ${command}`);
    }
  },
  { shouldMockEvents: true },
);
await i18n.use(initReactI18next).init({
  lng: language,
  resources: { en: { translation: en }, zh: { translation: zh } },
  fallbackLng: "en",
});
document.documentElement.lang = language;
if (params.get("dark") === "true")
  document.documentElement.classList.add("dark");
const client = new QueryClient({
  defaultOptions: {
    queries: { retry: false, refetchOnWindowFocus: false },
    mutations: { retry: false },
  },
});

Object.assign(window, {
  unifiedLayout: { rejectedCommands, settled: () => client.isFetching() === 0 },
});

function Fixture() {
  const [view, setView] = useState<"services" | "addHub">(
    scenario === "add" ? "addHub" : "services",
  );
  const [platform, setPlatform] = useState<AccountPlatform | undefined>(
    scenario === "native" ? "minimax_code" : undefined,
  );
  const [entryRequest, setEntryRequest] = useState<{
    id: number;
    tab: AddHubTab;
  }>({ id: 0, tab: "accounts" });
  const openAdd = (next: AddHubTab) => {
    setEntryRequest((entry) => ({ id: entry.id + 1, tab: next }));
    setView("addHub");
  };
  const back = () => {
    setPlatform(undefined);
    setView("services");
  };
  return (
    <div
      data-client-shell
      className="flex h-screen flex-col overflow-hidden bg-background pl-[var(--sidebar-width)] text-foreground"
      style={{ paddingTop: view === "addHub" ? 0 : PAGE_HEADER_HEIGHT }}
    >
      <ClientSidebar
        view={view}
        top={0}
        disabled={false}
        onNavigate={() => back()}
        onHelp={() => undefined}
      />
      <header
        hidden={view === "addHub"}
        data-layout-header
        className="fixed left-[var(--sidebar-width)] right-0 top-0 z-50 border-b border-border-default bg-background"
        style={{ height: PAGE_HEADER_HEIGHT }}
      >
        <div className="page-header flex h-full items-center gap-4">
          {platform && (
            <Button variant="ghost" onClick={back}>
              {i18n.t("common.back")}
            </Button>
          )}
          <h1 className="text-lg font-semibold">{i18n.t("client.services")}</h1>
        </div>
      </header>
      <main
        id="main-content"
        data-layout-view={view}
        className="flex min-h-0 flex-1 flex-col overflow-y-auto"
      >
        {view === "addHub" ? (
          <AddHubPage
            sourceAppId="codex"
            entryRequest={entryRequest}
            onBack={back}
            onAddProvider={async () => {
              throw new Error("Synthetic fixture does not save providers");
            }}
            onOpenPlatform={(next) => {
              setPlatform(next);
              setView("services");
            }}
          />
        ) : (
          <ServicesPage
            appId="codex"
            platform={platform}
            onOpenPlatform={setPlatform}
            onOpenAddHub={openAdd}
            onOpenApp={() => undefined}
            onBack={back}
          />
        )}
      </main>
    </div>
  );
}
createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={client}>
    <Fixture />
  </QueryClientProvider>,
);
