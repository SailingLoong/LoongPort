import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ArrowLeft, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { AppId } from "@/lib/api";
import { vendorApi, VENDOR_CATALOG } from "@/lib/api/vendor";
import { extractErrorMessage } from "@/utils/errorUtils";

/**
 * 「官方 API」页：与中转站广场并列的厂商选择页。
 *
 * 用户选一个厂商 → `vendor_open_login` 开登录窗 → 后端嗅探登录态、备好 sk、
 * 展开各平台 provider。账号行的展示仍在主页面的「官方 API」块里（`VendorBlock`），
 * 这页只负责「添加哪个」—— 与广场页（选站点 → 一键登录）交互同构。
 *
 * 登录成功后的列表刷新不走本页：后端 emit `vendor-accounts-changed`，
 * `RelaySection` 监听它刷新账号块（与 relay 侧靠 `provider-switched` 同一机制）。
 */
export interface OfficialApiPageProps {
  sourceAppId: AppId;
  onBack: () => void;
}

export function OfficialApiPage({ sourceAppId, onBack }: OfficialApiPageProps) {
  const { t } = useTranslation();
  const [loginningVendor, setLoginningVendor] = useState<string | null>(null);

  const handlePick = async (vendorId: string) => {
    if (loginningVendor) return;
    setLoginningVendor(vendorId);
    try {
      const result = await vendorApi.openLogin(vendorId, sourceAppId);
      // null = 用户自己关了窗或超时，不出提示（他知道自己干了什么）。
      if (result === null) return;
      toast.success(t("loongport.session.connected"));
      onBack();
    } catch (e) {
      toast.error(extractErrorMessage(e));
    } finally {
      setLoginningVendor(null);
    }
  };

  return (
    <main className="mx-auto flex h-full w-full max-w-[1180px] flex-col px-6 pb-6">
      <div className="flex items-start gap-3 border-b border-border-default py-4">
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="mt-0.5 h-8 w-8 shrink-0"
          onClick={onBack}
          aria-label={t("common.back")}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div>
          <h1 className="text-lg font-semibold tracking-tight">
            {t("loongport.officialApi.title")}
          </h1>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {t("loongport.officialApi.description")}
          </p>
        </div>
      </div>

      <div className="grid gap-4 py-6 sm:grid-cols-2">
        {VENDOR_CATALOG.map((vendor) => (
          <button
            key={vendor.id}
            type="button"
            disabled={loginningVendor !== null}
            onClick={() => void handlePick(vendor.id)}
            className="flex min-h-36 flex-col items-start gap-2 rounded-lg border border-border-default bg-background p-5 text-left shadow-sm transition-colors hover:border-blue-400/60 hover:bg-muted/40 disabled:cursor-not-allowed disabled:opacity-60"
          >
            <span className="text-base font-semibold">
              {vendor.displayName}
            </span>
            <span className="text-xs leading-relaxed text-muted-foreground">
              {t(vendor.descriptionKey)}
            </span>
            <span className="mt-auto inline-flex items-center gap-1.5 pt-2 text-sm font-medium text-blue-600 dark:text-blue-400">
              {loginningVendor === vendor.id && (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              )}
              {t("loongport.officialApi.connect")}
            </span>
          </button>
        ))}
      </div>
    </main>
  );
}
