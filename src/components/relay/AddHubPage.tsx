import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PLAZA_VISIBLE_DEFAULT, type AppId } from "@/lib/api";
import { useSettingsQuery } from "@/lib/query";
import { useVendorSupportedQuery } from "@/lib/query/vendor";
import {
  AddProviderForm,
  type AddProviderFormProps,
} from "@/components/providers/AddProviderForm";

import { RelayDirectoryPage } from "./directory/RelayDirectoryPage";
import { OfficialApiPage } from "./OfficialApiPage";

/** 聚合页的三个标签。 */
export type AddHubTab = "directory" | "official" | "manual";

/**
 * 顶栏大「+」的统一添加聚合页：点一次「+」直接进来，三个标签就地切换 ——
 * **中转站广场（默认）** / 官方 API / 手动添加。用户不用先在菜单里选一遍
 * 再跳页面（下拉菜单那版要两次点击，这正是它被替换的原因）。
 *
 * 三个标签的内容都是既有组件的 `embedded` 形态（不带返回箭头与页面级容器）：
 * - 中转站 → `RelayDirectoryPage`（单列表，按实测健康序排）；白名单外的站
 *   可在页尾手填域名直连；
 * - 官方 API → `OfficialApiPage`（是否出现这个标签由后端
 *   `vendor_list_accounts.supported` 说了算，与 `VendorBlock` 整块同一来源）；
 * - 手动添加 → `AddProviderForm`（原 `AddProviderDialog` 的表单体）。
 *
 * `initialTab` 让区块空态这类「指名道姓」的入口直落对应标签（如官方 API
 * 区块的空态占位点进来就落「官方 API」）。
 *
 * 返回与「添加成功/取消」的收尾统一走 `onBack`；本页是供应商页的临时子流程，
 * 不进 `LAST_VIEW`（与被它替换的两个独立视图同一条规则）。
 */
export function AddHubPage({
  sourceAppId,
  initialTab = "directory",
  onBack,
  onAddProvider,
  firstVisit = false,
}: {
  sourceAppId: AppId;
  /** 进来落在哪个标签；顶栏大「+」与首启引导不传（落默认「中转站」）。 */
  initialTab?: AddHubTab;
  onBack: () => void;
  onAddProvider: AddProviderFormProps["onSubmit"];
  /** 新人首启落广场：同时弹一次「手填域名直达」（见 `FirstVisitDomainDialog`）。 */
  firstVisit?: boolean;
}) {
  const { t } = useTranslation();
  const vendorSupported = useVendorSupportedQuery(sourceAppId);
  // 广场开关（默认值由后端按首启归因播种，设置页可手动翻转）。false 时广场
  // tab 整个不出现 —— 添加站点的路只剩官方 API 与手动添加（搜索框照样可加）。
  // 未加载/未播种都按「展示」处理（未归因默认），且用派生值而非初值快照，
  // 设置晚到也能把已落在「广场」上的选中态拨回「手动添加」。
  const { data: settings } = useSettingsQuery();
  const plazaVisible = settings?.plazaVisible ?? PLAZA_VISIBLE_DEFAULT;
  const [tab, setTab] = useState<AddHubTab>(initialTab);
  const effectiveTab: AddHubTab =
    !plazaVisible && tab === "directory" ? "manual" : tab;

  return (
    <Tabs
      value={effectiveTab}
      onValueChange={(v) => setTab(v as AddHubTab)}
      className="mx-auto flex h-full w-full max-w-[1180px] flex-col px-6 pb-6"
    >
      <div className="flex shrink-0 items-center gap-4 border-b border-border-default py-3">
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-8 w-8 shrink-0"
          onClick={onBack}
          aria-label={t("common.back")}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <TabsList>
          {plazaVisible && (
            <TabsTrigger value="directory">
              {t("loongport.sections.relay")}
            </TabsTrigger>
          )}
          {vendorSupported && (
            <TabsTrigger value="official">
              {t("loongport.sections.official")}
            </TabsTrigger>
          )}
          <TabsTrigger value="manual">
            <Plus className="mr-1.5 h-3.5 w-3.5" />
            {t("loongport.addEntry.manual")}
          </TabsTrigger>
        </TabsList>
      </div>

      {plazaVisible && (
        <TabsContent value="directory" className="mt-0 min-h-0 flex-1">
          <RelayDirectoryPage
            sourceAppId={sourceAppId}
            onBack={onBack}
            embedded
            firstVisit={firstVisit}
          />
        </TabsContent>
      )}

      {vendorSupported && (
        <TabsContent value="official" className="mt-0 min-h-0 flex-1">
          <OfficialApiPage sourceAppId={sourceAppId} onBack={onBack} embedded />
        </TabsContent>
      )}

      <TabsContent value="manual" className="mt-0 min-h-0 flex-1">
        <AddProviderForm
          appId={sourceAppId}
          onSubmit={onAddProvider}
          onDone={onBack}
        />
      </TabsContent>
    </Tabs>
  );
}
