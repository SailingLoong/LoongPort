/**
 * 生图标签页的页级外壳：顶部说明（含 MCP 注册开关）+「生成 / 档位」分段视图。
 *
 * ## 为什么 codex-image 要有自己的页组件
 *
 * 其它 app 标签页直接渲染 [`RelaySection`]（档位管理）；生图页在它外面还有
 * 「直接生图」这条第二条链路，且它是用户进这个页的主要目的 —— 所以「生成」是
 * 默认视图，「档位」原样内嵌 `RelaySection`。分段按钮的形状与主页面
 * 「省心 / 自主」（`AppModeSegmented`）一致，只是配色跟本页的 violet 主题。
 *
 * 空态（没有任何生图档位）保持旧行为：整页只渲染那一段「可能没有生图分组」的
 * 说明，不出现分段与生成 UI —— 生图档位不是单独添加的，用户在这儿没有可做的事。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";

import { RelaySection } from "./RelaySection";
import { ImageTabNotice } from "./ImageTabNotice";
import { ImagegenPlayground } from "./ImagegenPlayground";
import { useImageTiers } from "@/lib/query/imagegen";

interface ImageTabPageProps {
  onOpenAddHub: () => void;
}

export function ImageTabPage({ onOpenAddHub }: ImageTabPageProps) {
  const { t } = useTranslation();
  const [view, setView] = useState<"generate" | "tiers">("generate");
  const tiersQuery = useImageTiers();

  const rows = tiersQuery.data;
  // 与 RelaySection 的 bothEmpty 同判据（生图栏没有 vendor 行，relays 即全部）。
  if (rows != null && rows.length === 0) {
    return <ImageTabNotice empty />;
  }

  return (
    <div className="space-y-3">
      <ImageTabNotice empty={false} />
      <div className="grid w-fit grid-cols-2 gap-2" role="group">
        <SegmentedButton
          active={view === "generate"}
          onClick={() => setView("generate")}
        >
          {t("loongport.imagegenPlayground.view.generate")}
        </SegmentedButton>
        <SegmentedButton
          active={view === "tiers"}
          onClick={() => setView("tiers")}
        >
          {t("loongport.imagegenPlayground.view.tiers")}
        </SegmentedButton>
      </div>
      {view === "generate" ? (
        <ImagegenPlayground />
      ) : (
        <RelaySection appId="codex-image" onOpenAddHub={onOpenAddHub} />
      )}
    </div>
  );
}

/** 二选一分段按钮（形状与主页面「省心 / 自主」一致，配色跟本页 violet 主题）。 */
function SegmentedButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={
        active
          ? "rounded-md border border-violet-500/60 bg-violet-500/10 px-3 py-1.5 text-sm text-violet-600 transition-colors dark:text-violet-400"
          : "rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-accent"
      }
    >
      {children}
    </button>
  );
}
