import { useState } from "react";
import { createRoot } from "react-dom/client";
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { Download, Import } from "lucide-react";
import { GithubIcon } from "@/components/icons/GithubIcon";
import { ClientSidebar } from "@/components/shell/ClientSidebar";
import type { ClientView } from "@/components/shell/navigation";
import { PAGE_HEADER_HEIGHT } from "@/components/shell/layout";
import { Button } from "@/components/ui/button";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import "@/index.css";

// Presentation-only fixture. No Tauri bridge, database, credentials or requests.
const language = new URLSearchParams(location.search).get("lang") ?? "en";
await i18n.use(initReactI18next).init({
  lng: language,
  resources: { en: { translation: en }, zh: { translation: zh } },
  fallbackLng: "en",
});

function ShellFixture() {
  const [view, setView] = useState<ClientView>("providers");
  const [navigations, setNavigations] = useState(0);
  const top = 28;
  return (
    <div
      data-client-shell
      className="flex h-screen flex-col overflow-hidden bg-background pl-[var(--sidebar-width)] text-foreground"
      style={{ paddingTop: top + PAGE_HEADER_HEIGHT }}
    >
      <ClientSidebar
        view={view}
        top={top}
        disabled={false}
        onNavigate={(next) => {
          setView(next);
          setNavigations((count) => count + 1);
        }}
        onHelp={() => undefined}
        footer={
          <>
            <Button size="icon" variant="ghost" aria-label="Import fixture">
              <Import className="h-4 w-4" />
            </Button>
            <Button size="icon" variant="ghost" aria-label="Update fixture">
              <Download className="h-4 w-4" />
            </Button>
            <Button size="icon" variant="ghost" aria-label="GitHub fixture">
              <GithubIcon className="h-4 w-4" />
            </Button>
          </>
        }
      />
      <header
        className="fixed left-[var(--sidebar-width)] right-0 z-50 border-b border-border-default bg-background"
        style={{ top, height: PAGE_HEADER_HEIGHT }}
      >
        <div className="page-header flex h-full items-center justify-between gap-4">
          <h1 className="text-lg font-semibold">
            {i18n.t("client.applications")}
          </h1>
          <span className="text-xs text-muted-foreground">
            Shell regression fixture
          </span>
        </div>
      </header>
      <main
        id="main-content"
        tabIndex={-1}
        className="flex min-h-0 flex-1 flex-col overflow-y-auto focus:outline-none"
      >
        <div className="page-content space-y-6">
          <p className="text-muted-foreground">
            Synthetic presentation data. This fixture validates the shell only.
          </p>
          <div className="rounded-lg border border-border-default p-5">
            <h2 className="font-semibold">Example workspace</h2>
            <p className="mt-2 text-muted-foreground">
              No configuration writes or live accounts.
            </p>
          </div>
          <div
            data-table-scroll
            className="overflow-x-auto rounded-lg border border-border-default"
          >
            <table className="w-full text-left" style={{ minWidth: 1120 }}>
              <thead className="border-b border-border-default text-muted-foreground">
                <tr>
                  {[
                    "Order",
                    "Profile / account",
                    "Model",
                    "Latency",
                    "Errors",
                    "Balance",
                    "Action",
                  ].map((label) => (
                    <th key={label} className="p-4 font-medium">
                      {label}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {[1, 2, 3].map((number) => (
                  <tr
                    key={number}
                    className="border-b border-border-default last:border-b-0"
                  >
                    <td className="p-4">{number}</td>
                    <td className="p-4">Example profile {number}</td>
                    <td className="p-4">Model A</td>
                    <td className="p-4">—</td>
                    <td className="p-4">—</td>
                    <td className="p-4">—</td>
                    <td className="p-4">—</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p data-navigation-count={navigations}>
            Navigation actions: {navigations}
          </p>
        </div>
      </main>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<ShellFixture />);
