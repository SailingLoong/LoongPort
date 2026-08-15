import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const read = (path: string) =>
  readFileSync(resolve(process.cwd(), path), "utf8");

describe("中转站业务事实由后端定义", () => {
  it("models.dev 自动刷新由后端调度，渲染端仅失效相关查询", () => {
    const constants = read("src/config/constants.ts");
    const main = read("src/main.tsx");
    const usageQueries = read("src/lib/query/usage.ts");

    expect(constants).toContain(
      'MODELS_DEV_PRICING_UPDATED_EVENT = "models-dev-pricing-updated"',
    );
    expect(main).toContain("listen(MODELS_DEV_PRICING_UPDATED_EVENT");
    expect(main).toContain(
      "queryClient.invalidateQueries({ queryKey: usageKeys.all })",
    );
    expect(main).toContain("queryKey: usageKeys.modelsDevSyncConfig()");
    expect(usageQueries).toContain('"models-dev-sync-config"');
    expect(main).not.toContain("syncModelsDevPricing");
    expect(main).not.toContain("setInterval");
  });

  it("VeriDrop 后台更新事件和手动刷新命令有唯一契约", () => {
    const constants = read("src/config/constants.ts");
    const relayApi = read("src/lib/api/relay.ts");

    expect(constants).toContain(
      'RELAY_DIRECTORY_UPDATED_EVENT = "relay-directory-updated"',
    );
    expect(relayApi).toContain('invoke("relay_refresh_directory", { kind })');
  });

  it("页面不编排全量刷新或汇总业务结果", () => {
    const section = read("src/components/relay/RelaySection.tsx");

    expect(section).not.toContain("Promise.allSettled");
    expect(section).not.toContain("sumTiersForApp");
    expect(section).not.toContain("vendorProviderIds");
    expect(section).not.toContain('msg.includes("100")');
    expect(section).not.toContain("reportProvision");
    expect(section).not.toContain("removeConfirmMessageKey");
    expect(section).toContain("relayApi.refreshAll(appId)");
  });

  it("旧的前端业务规则 helper 已移除", () => {
    for (const path of [
      "src/components/relay/lowBalance.ts",
      "src/components/relay/provisionScope.ts",
      "src/components/relay/reportProvision.ts",
      "src/components/relay/removeConfirmWording.ts",
    ]) {
      expect(() => read(path)).toThrow();
    }
  });
});

describe("供应商业务事实由后端定义", () => {
  it("Provider 页面不再从原始配置或额外接口推导展示状态", () => {
    const list = read("src/components/providers/ProviderList.tsx");
    const card = read("src/components/providers/ProviderCard.tsx");
    const usageModal = read("src/components/UsageScriptModal.tsx");
    const editDialog = read("src/components/providers/EditProviderDialog.tsx");
    const queries = read("src/lib/query/queries.ts");
    const mutations = read("src/lib/query/mutations.ts");
    const providerForm = read(
      "src/components/providers/forms/ProviderForm.tsx",
    );
    const omoModelSource = read(
      "src/components/providers/forms/hooks/useOmoModelSource.ts",
    );
    const providerApi = read("src/lib/api/providers.ts");

    expect(list).not.toContain("isManagedProviderId");
    expect(list).not.toContain("useOpenClawLiveProviderIds");
    expect(list).not.toContain("useOpenClawDefaultModel");
    expect(list).not.toContain("useHermesLiveProviderIds");
    expect(list).not.toContain("useHermesModelConfig");
    expect(list).not.toContain("useCurrentOmoProviderId");
    expect(list).not.toContain("currentProviderId");
    expect(list).not.toContain("isProviderInConfig");
    expect(list).not.toContain("isProviderDefaultModel");
    expect(card).not.toContain("isHermesReadOnlyProvider");
    expect(queries).not.toContain("providersApi.getCurrent(appId)");
    expect(usageModal).not.toContain("isOfficialSubscriptionProvider");
    expect(card).not.toContain("supportsOfficialSubscription");
    expect(card).not.toContain("TEMPLATE_TYPES.OFFICIAL_SUBSCRIPTION");
    expect(card).toContain(
      "provider.presentation?.usesOfficialSubscriptionUsage === true",
    );
    expect(editDialog).not.toContain("providersApi.getCurrent(appId)");
    expect(editDialog).not.toContain("vscodeApi.getLiveProviderSettings");
    expect(editDialog).not.toContain("openclawApi.getLiveProvider");
    expect(editDialog).toContain("providersApi.getEditSettings(provider.id");
    expect(mutations).not.toContain("generateUUID");
    expect(providerForm).not.toContain("getOpenCodeLiveProviderIds");
    expect(providerForm).not.toContain("useOpenClawLiveProviderIds");
    expect(providerForm).not.toContain("useHermesLiveProviderIds");
    expect(omoModelSource).not.toContain("getOpenCodeLiveProviderIds");
    expect(omoModelSource).toContain("provider.presentation?.isInConfig");
    expect(providerApi).not.toContain("getOpenCodeLiveProviderIds");
    expect(providerApi).not.toContain("getOpenClawLiveProviderIds");
    expect(providerApi).not.toContain("getHermesLiveProviderIds");
    expect(read("src/App.tsx")).not.toContain("getOpenCodeLiveProviderIds");
    expect(read("src/App.tsx")).not.toContain("getOpenClawLiveProviderIds");
    expect(read("src/App.tsx")).not.toContain("getHermesLiveProviderIds");
  });

  it("官网账号刷新由后端主动核验登录态与最新额度", () => {
    const vendorCommands = read("src-tauri/src/commands/vendor.rs");

    expect(vendorCommands).toContain("refresh_vendor_session_balance");
    // 余额/密钥调用按厂商分发（`crate::vendor::balance(vendor, …)`），
    // 命令层不再直连某一家厂商的模块。
    expect(vendorCommands).toContain(
      "crate::vendor::balance(vendor, &row.auth_token)",
    );
  });
});
