import { Suspense } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "@/App";
import { ThemeProvider } from "@/components/theme-provider";
import { UpdateProvider } from "@/contexts/UpdateContext";
import "@/i18n";

export function createApplicationsC2Client() {
  return new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
      mutations: { retry: false },
    },
  });
}
/** Only the actual application and its ordinary providers; no design recreation. */
export function ApplicationsC2App({ client }: { client: QueryClient }) {
  return (
    <QueryClientProvider client={client}>
      <ThemeProvider defaultTheme="light">
        <UpdateProvider>
          <Suspense fallback={<p>Loading synthetic C2 fixture…</p>}>
            <App />
          </Suspense>
        </UpdateProvider>
      </ThemeProvider>
    </QueryClientProvider>
  );
}
