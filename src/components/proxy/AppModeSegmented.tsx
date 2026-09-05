/**
 * 主页面顶部的「省心 / 自主」运行方式分段选择器（当前 app 维度）。
 *
 * 这是每个 app 模式切换的**唯一入口**（唯源）：省心 = 系统在托管档位间自动
 * 选路与切换；自主 = 用户指定唯一供应商（provider 页全部能力）。开启走与
 * 设置页同一编排（首次开启弹同款一次性授权），切自主走 [`useDisableAutoMode`]
 * （收该 app 的路由接管，不停全局路由）。按钮形状与 EasyBoard 的
 * ChoiceButton 一致（同款二选一，样式互指，别改一处忘一处）。
 */

import { useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  hasConfirmedAutoMode,
  markAutoModeConfirmed,
} from "@/components/proxy/autoModeConfirm";
import {
  useAutoModeStatus,
  useDisableAutoMode,
  useEnableAutoMode,
} from "@/lib/query/autoMode";
import { useTranslation } from "react-i18next";
import type { ProxyAppId } from "@/config/appConfig";

interface AppModeSegmentedProps {
  activeApp: ProxyAppId;
}

export function AppModeSegmented({ activeApp }: AppModeSegmentedProps) {
  const { t } = useTranslation();
  const { data: status, isLoading } = useAutoModeStatus(activeApp);
  const enableFlow = useEnableAutoMode();
  const disableFlow = useDisableAutoMode();
  const [showConfirm, setShowConfirm] = useState(false);

  const isEasy = status?.enabled ?? false;
  const hasCandidates = status?.hasCandidates ?? false;
  const cliInstalled = status?.cliInstalled ?? true;
  const busy = isLoading || enableFlow.isPending || disableFlow.isPending;
  // 开启省心需要托管档位 + 该 CLI 已装（与设置页同判据）；切自主随时可用
  const easyBlocked = !hasCandidates || !cliInstalled;
  const blockedTitle = !cliInstalled
    ? t("autoMode.runMode.cliMissing", {
        defaultValue: "该 CLI 未安装，省心模式不可用",
      })
    : !hasCandidates
      ? t("autoMode.runMode.noCandidates", {
          defaultValue: "还没有托管档位，先在自主模式添加中转站",
        })
      : undefined;

  const pick = (mode: "easy" | "self") => {
    if (busy) return;
    if (mode === "self") {
      if (isEasy) disableFlow.mutate({ appType: activeApp });
      return;
    }
    if (isEasy || easyBlocked) return;
    if (hasConfirmedAutoMode()) {
      enableFlow.mutate({ appType: activeApp });
    } else {
      setShowConfirm(true);
    }
  };

  return (
    <div className="flex flex-wrap items-center gap-3">
      <div className="grid grid-cols-2 gap-2" role="group">
        <SegmentedButton
          active={isEasy}
          disabled={busy || (!isEasy && easyBlocked)}
          title={isEasy ? undefined : blockedTitle}
          onClick={() => pick("easy")}
        >
          {t("autoMode.runMode.easy", { defaultValue: "省心" })}
        </SegmentedButton>
        <SegmentedButton
          active={!isEasy}
          disabled={busy}
          onClick={() => pick("self")}
        >
          {t("autoMode.runMode.self", { defaultValue: "自主" })}
        </SegmentedButton>
      </div>
      <p className="text-xs text-muted-foreground">
        {isEasy
          ? t("autoMode.runMode.easySubtitle", {
              defaultValue: "系统在托管档位间自动选路与切换",
            })
          : t("autoMode.runMode.selfSubtitle", {
              defaultValue: "由你指定唯一的供应商",
            })}
      </p>
      <ConfirmDialog
        isOpen={showConfirm}
        variant="info"
        title={t("autoMode.confirm.title", "开启省心模式")}
        message={t(
          "autoMode.confirm.message",
          "系统将按所选策略自动挑选并切换托管档位：同一会话内保持当前档位不变（避免丢失提示词缓存），当前档位故障或闲置后才切换到更合适的一档。若本地路由未开启，将一并开启并接管该 CLI 的配置（关闭省心模式时会一并恢复）。确定开启？",
        )}
        confirmText={t("autoMode.confirm.confirm", "开启")}
        onConfirm={() => {
          markAutoModeConfirmed();
          setShowConfirm(false);
          enableFlow.mutate({ appType: activeApp });
        }}
        onCancel={() => setShowConfirm(false)}
      />
    </div>
  );
}

/** 二选一分段按钮（形状与 EasyBoard 的 ChoiceButton 一致）。 */
function SegmentedButton({
  active,
  disabled,
  title,
  onClick,
  children,
}: {
  active: boolean;
  disabled?: boolean;
  title?: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      title={title}
      className={
        active
          ? "rounded-md border border-emerald-500/60 bg-emerald-500/10 px-3 py-1.5 text-sm text-emerald-600 transition-colors dark:text-emerald-400"
          : "rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50"
      }
    >
      {children}
    </button>
  );
}
