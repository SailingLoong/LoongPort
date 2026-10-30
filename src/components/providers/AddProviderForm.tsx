import { useTranslation } from "react-i18next";
import { Loader2, Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import { ProviderForm } from "@/components/providers/forms/ProviderForm";
import { PreservedView } from "@/components/ui/PreservedView";
import { AuthSettingsPanel } from "@/components/providers/AuthSettingsPanel";
import { UniversalProviderFormModal } from "@/components/universal/UniversalProviderFormModal";
import { UniversalProviderPanel } from "@/components/universal";
import type { OpenClawSuggestedDefaults } from "@/config/openclawProviderPresets";
import { useAddProviderFormViewModel } from "./useAddProviderFormViewModel";

/**
 * 手动添加供应商的表单体（无外壳）：App 专属/统一供应商两个 tab + 表单 + 动作条。
 *
 * 原来装在 `AddProviderDialog` 的全屏弹窗里、由顶栏大「+」直接打开；现在「+」
 * 进的是统一添加聚合页（`AddHubPage`），本组件作为其中「手动添加」标签的内容
 * 内嵌渲染 —— 弹窗外壳因此没有调用方了，表单逻辑整层搬过来，onDone 语义 =
 * 「这一轮添加结束」（提交成功 / 用户取消），由宿主决定回哪去。
 */
export interface AddProviderFormProps {
  initialPresetName?: string;
  onSaveStart?: () => number;
  appId: AppId;
  onSubmit: (
    provider: Omit<Provider, "id"> & {
      providerKey?: string;
      suggestedDefaults?: OpenClawSuggestedDefaults;
      ensureClaudeDesktopOfficialSeed?: boolean;
      ensureGrokBuildOfficialSeed?: boolean;
    },
  ) => Promise<void> | void;
  /** 提交成功或用户取消 —— 宿主收尾（聚合页回供应商列表）。 */
  onDone: (reason?: "saved" | "cancel", saveEpoch?: number) => void;
}

export function AddProviderForm({
  initialPresetName,
  onSaveStart,
  appId,
  onSubmit,
  onDone,
}: AddProviderFormProps) {
  const { t } = useTranslation();
  const {
    showUniversalTab,
    activeTab,
    universalFormOpen,
    appSaveCompleted,
    selectedUniversalPreset,
    isFormSubmitting,
    setIsFormSubmitting,
    authSettingsTarget,
    setAuthSettingsTarget,
    isFormReady,
    handleSubmitReadyChange,
    handleUniversalProviderSave,
    handleUniversalFormClose,
    handleSubmit,
    cancel,
    openUniversalForm,
    changeTab,
  } = useAddProviderFormViewModel({ appId, onSubmit, onDone, onSaveStart });

  const footer =
    !showUniversalTab || activeTab === "app-specific" ? (
      <>
        <span className="mr-auto min-w-0 text-xs text-muted-foreground truncate">
          {appSaveCompleted
            ? t("provider.addSavedInBackground", {
                defaultValue: "已保存。请重新打开表单以添加新供应商。",
              })
            : t("provider.addFooterHint")}
        </span>
        <Button
          variant="outline"
          onClick={cancel}
          className="border-border/20 hover:bg-accent hover:text-accent-foreground"
        >
          {t("common.cancel")}
        </Button>
        <Button
          type="submit"
          form="provider-form"
          disabled={isFormSubmitting || appSaveCompleted || !isFormReady}
          className="bg-primary text-primary-foreground hover:bg-primary/90"
        >
          {isFormSubmitting ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Plus className="mr-2 h-4 w-4" />
          )}
          {t("common.add")}
        </Button>
      </>
    ) : (
      <>
        <Button
          variant="outline"
          onClick={cancel}
          className="border-border/20 hover:bg-accent hover:text-accent-foreground"
        >
          {t("common.cancel")}
        </Button>
        <Button
          onClick={openUniversalForm}
          className="bg-primary text-primary-foreground hover:bg-primary/90"
        >
          <Plus className="h-4 w-4 mr-2" />
          {t("universalProvider.add")}
        </Button>
      </>
    );

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div
        className={`min-h-0 flex-1 space-y-6 overflow-y-auto ${appId === "pi" ? "pt-3 pb-0" : "pt-3 pb-6"}`}
      >
        {showUniversalTab ? (
          <Tabs
            value={activeTab}
            onValueChange={(v) => changeTab(v as "app-specific" | "universal")}
          >
            <TabsList className="grid w-full grid-cols-2 mb-6">
              <TabsTrigger value="app-specific">
                {t(`apps.${appId}`)} {t("provider.tabProvider")}
              </TabsTrigger>
              <TabsTrigger value="universal">
                {t("provider.tabUniversal")}
              </TabsTrigger>
            </TabsList>

            <TabsContent
              value="app-specific"
              forceMount
              className="mt-0 data-[state=inactive]:hidden"
            >
              <PreservedView active={activeTab === "app-specific"}>
                <ProviderForm
                  initialPresetName={initialPresetName}
                  appId={appId}
                  submitLabel={t("common.add")}
                  onSubmit={handleSubmit}
                  onCancel={cancel}
                  onManageAuthAccounts={setAuthSettingsTarget}
                  onSubmittingChange={setIsFormSubmitting}
                  onSubmitReadyChange={handleSubmitReadyChange}
                  showButtons={false}
                />
              </PreservedView>
            </TabsContent>

            <TabsContent
              value="universal"
              forceMount
              className="mt-0 data-[state=inactive]:hidden"
            >
              <PreservedView active={activeTab === "universal"}>
                <UniversalProviderPanel />
              </PreservedView>
            </TabsContent>
          </Tabs>
        ) : (
          // OpenCode/OpenClaw: directly show form without tabs
          <ProviderForm
            initialPresetName={initialPresetName}
            appId={appId}
            submitLabel={t("common.add")}
            onSubmit={handleSubmit}
            onCancel={cancel}
            onManageAuthAccounts={setAuthSettingsTarget}
            onSubmittingChange={setIsFormSubmitting}
            onSubmitReadyChange={handleSubmitReadyChange}
            showButtons={false}
          />
        )}
      </div>

      <div className="flex shrink-0 items-center justify-end gap-3 border-t border-border-default py-4">
        {footer}
      </div>

      {showUniversalTab && (
        <UniversalProviderFormModal
          isOpen={universalFormOpen}
          onClose={handleUniversalFormClose}
          onSave={handleUniversalProviderSave}
          initialPreset={selectedUniversalPreset}
        />
      )}

      <AuthSettingsPanel
        target={authSettingsTarget}
        onClose={() => setAuthSettingsTarget(null)}
      />
    </div>
  );
}
