import { useTranslation } from "react-i18next";
import { Building2, Globe, Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { AppId } from "@/lib/api";
import { useVendorSupportedQuery } from "@/lib/query/vendor";

/**
 * 顶栏大「+」的统一添加入口：中转站广场 / 官方 API / 手动表单三选一。
 *
 * 原来三个入口散在两处 —— 两个区块头各有一个小「+」（`RelayTierList` /
 * `VendorBlock`），顶栏大「+」则直接开手填表单。现在小「+」已删，
 * 「加东西」在界面上的位置只剩这一处。
 *
 * 「官方 API」一项是否出现由后端说了算（`vendor_list_accounts` 的
 * `supported`）—— 与 `VendorBlock` 整块的出现条件同一来源，前端不
 * 自己维护厂商支持列表。
 */
export function AddEntryMenu({
  appId,
  onOpenRelayDirectory,
  onOpenOfficialApi,
  onOpenManual,
}: {
  appId: AppId;
  onOpenRelayDirectory: () => void;
  onOpenOfficialApi: () => void;
  onOpenManual: () => void;
}) {
  const { t } = useTranslation();
  const vendorSupported = useVendorSupportedQuery(appId);
  const title = t("loongport.addEntry.title");

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          size="icon"
          className="ml-2 bg-orange-500 hover:bg-orange-600 dark:bg-orange-500 dark:hover:bg-orange-600 text-white shadow-lg shadow-orange-500/30 dark:shadow-orange-500/40 rounded-full w-8 h-8"
          aria-label={title}
          title={title}
        >
          <Plus className="w-5 h-5" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={onOpenRelayDirectory}>
          <Globe className="mr-2 h-4 w-4" />
          {t("loongport.tierList.addSite")}
        </DropdownMenuItem>
        {vendorSupported && (
          <DropdownMenuItem onClick={onOpenOfficialApi}>
            <Building2 className="mr-2 h-4 w-4" />
            {t("loongport.sections.official")}
          </DropdownMenuItem>
        )}
        <DropdownMenuItem onClick={onOpenManual}>
          <Plus className="mr-2 h-4 w-4" />
          {t("provider.addNewProvider")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
