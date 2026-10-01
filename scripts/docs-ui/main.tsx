import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { fixture } from "./fixtures";
mockWindows("main");
mockIPC(fixture);
window.fetch = async () => {
  throw Error("Network disabled in documentation fixture");
};
import React from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "@/App";
import { UpdateProvider } from "@/contexts/UpdateContext";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { FrontendErrorBoundary } from "@/components/FrontendErrorBoundary";
import i18n from "@/i18n";
import "@/index.css";
localStorage.setItem("loongport-last-app", "codex");
localStorage.setItem("loongport-last-view", "providers");
localStorage.setItem("loongport-theme", "light");
i18n.changeLanguage("zh");
const client = new QueryClient({
  defaultOptions: {
    queries: { retry: false, refetchOnWindowFocus: false, staleTime: 600000 },
  },
});
createRoot(document.getElementById("root")!).render(
  <FrontendErrorBoundary>
    <QueryClientProvider client={client}>
      <ThemeProvider defaultTheme="light" storageKey="loongport-theme">
        <UpdateProvider>
          <App />
          <Toaster />
        </UpdateProvider>
      </ThemeProvider>
    </QueryClientProvider>
  </FrontendErrorBoundary>,
);
