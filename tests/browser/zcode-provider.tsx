import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { ZCodeProviderPanel } from "@/components/zcode/ZCodeProviderPanel";
import { zcodeApi, type ZCodeConfig } from "@/lib/api/zcode";
import { zcodeAccountsApi } from "@/lib/api/zcodeAccounts";
import "@/index.css";

// Only synthetic redacted API results enter this isolated component harness.
zcodeAccountsApi.recoveryStatus = async () => ({
  revision: "synthetic-recovery",
  pending: false,
  nativeUnconfirmed: false,
  records: [],
});
const count = Number(new URLSearchParams(location.search).get("models") ?? 80);
const config: ZCodeConfig = {
  revision: "initial",
  providers: [
    {
      id: "loongport-example",
      name: "Managed example",
      apiType: "openai-responses",
      baseUrl: "https://api.example/v1",
      hasApiKey: true,
      managed: true,
      models: Array.from(
        { length: count },
        (_, index) => `example-model-${index}`,
      ),
    },
  ],
};
let reads = 0;
zcodeApi.read = async () =>
  reads++ === 0
    ? config
    : {
        ...config,
        revision: "external",
        providers: [
          {
            ...config.providers[0],
            name: "External example",
            models: config.providers[0].models.map(
              (model) => `external-${model}`,
            ),
          },
        ],
      };
zcodeApi.save = async (input) => {
  if (input.revision === "initial")
    throw { code: "zcode.configuration_changed" };
  return {
    ...config,
    revision: "saved",
    providers: [
      {
        ...config.providers[0],
        name: input.name,
        baseUrl: input.baseUrl,
        apiType: input.apiType,
        models: input.models,
      },
    ],
  };
};
await i18n
  .use(initReactI18next)
  .init({ lng: "en", resources: {}, fallbackLng: "en" });
createRoot(document.getElementById("root")!).render(
  <QueryClientProvider
    client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
  >
    <ZCodeProviderPanel />
  </QueryClientProvider>,
);
