import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Loader2 } from "lucide-react";
import { OpenCodeLocalImportForm } from "./OpenCodeLocalImportForm";
import { OpenCodePersonalKeyForm } from "./OpenCodePersonalKeyForm";
import { Button } from "@/components/ui/button";
import type { AppId } from "@/lib/api";
import type { ConnectedService } from "./onboarding/useServiceOnboarding";
import { useOfficialApiPageViewModel } from "./useOfficialApiPageViewModel";
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
  vendorId?: string;
  onBack: () => void;
  onConnected?: (account: ConnectedService) => void;
  /** 内嵌在统一添加聚合页（`AddHubPage`）里时不带返回箭头与页面级容器。 */
  embedded?: boolean;
}
export function OfficialApiPage({
  sourceAppId,
  vendorId,
  onBack,
  embedded = false,
  onConnected,
}: OfficialApiPageProps) {
  const { t } = useTranslation();
  const openCodeKeyForms = useRef<HTMLDivElement>(null);
  const {
    loginningVendor,
    visibleVendors,
    showOpenCodeEntry,
    recoveryVisible,
    handlePick,
    back,
  } = useOfficialApiPageViewModel({
    sourceAppId,
    vendorId,
    onBack,
    onConnected,
  });
  return (
    <div
      className={
        embedded ? "flex w-full flex-col" : "page-content flex h-full flex-col"
      }
    >
      <div className="flex items-start gap-3 border-b border-border-default py-4">
        {!embedded && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="mt-0.5 h-8 w-8 shrink-0"
            onClick={back}
            aria-label={t("common.back")}
          >
            <ArrowLeft className="mr-2 h-4 w-4" />
            {t("common.back")}
          </Button>
        )}
        <div>
          <h1 className="text-lg font-semibold tracking-tight">
            {t("loongport.officialApi.title")}
          </h1>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {t("loongport.officialApi.description")}
          </p>
        </div>
      </div>

      {showOpenCodeEntry && (
        <div
          ref={openCodeKeyForms}
          role="region"
          aria-label="OpenCode API key 接入方式"
          tabIndex={-1}
          className="space-y-3"
        >
          <OpenCodeLocalImportForm
            onSaved={() => {
              /* Backend event refreshes the account list. */
            }}
          />
          <OpenCodePersonalKeyForm
            onSaved={() => {
              /* Same account owner. */
            }}
          />
        </div>
      )}
      <div className="grid gap-4 py-6 sm:grid-cols-2">
        {visibleVendors.map((vendor) => (
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
              {vendor.id === "opencode"
                ? "官网账户登录（不会自动获取密钥）"
                : t("loongport.officialApi.connect")}
            </span>
          </button>
        ))}
      </div>
      {recoveryVisible && (
        <div
          role="alert"
          className="space-y-3 rounded-lg border border-border-default p-4"
        >
          <p className="text-sm">
            网页登录未完成。若已有完整 Personal API
            key，可使用本页的个人密钥入口； 若密钥已在本机 OpenCode
            中，可先只读预览，再自行确认导入。不会自动读取、
            保存或创建密钥；保存或导入后仍尚未验证真实调用。
          </p>
          <Button
            type="button"
            variant="outline"
            onClick={() => {
              openCodeKeyForms.current?.focus();
              openCodeKeyForms.current?.scrollIntoView({ block: "nearest" });
            }}
          >
            使用已有 Personal key 或本机导入
          </Button>
        </div>
      )}
    </div>
  );
}
