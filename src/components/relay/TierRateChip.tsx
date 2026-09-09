/**
 * 档位倍率标签：分组名右侧那枚高亮小 chip 的唯一形状源。
 *
 * 三处消费共用（provider 页档位行 `RelayRow`、省心看板档位卡 `TierList`、
 * 生图档位选择器 `ImagegenPlayground`），改样式只改这里。
 *
 * ⚠️ **`rate` 为 null 时整体不渲染，绝不能显示成 0 或「免费」** ——
 * 列表命令只读本地，倍率是服务端定价，要 provision 才有值
 * （`relay.rs` 的 `TierInfo` 注释钉过这条）。显示成 0 会让用户以为这是
 * 最便宜的一档。省心看板原来 null 显示「×?」，与档位行「不显示」是同一
 * 事实两套纪律，这次一并对齐到不显示。
 *
 * 配色是**中性高亮**：蓝=在用、绿=代理接管、琥珀=需留意、紫=生图/OMO，
 * 都有主了；倍率是定价事实，不抢任何语义色，靠实底 + 半粗 + 全前景色
 * 提对比度（原来散在各处的 `text-muted-foreground` 灰小字看不清）。
 * 暗色模式实底单独提亮一档（`bg-muted` 在暗色下和卡片底几乎同色）。
 */
import { useTranslation } from "react-i18next";

export function TierRateChip({ rate }: { rate: number | null }) {
  const { t } = useTranslation();
  if (rate === null) return null;
  return (
    <span className="inline-flex shrink-0 items-center rounded bg-muted px-1.5 py-0.5 text-[10px] font-semibold tabular-nums text-foreground dark:bg-muted-foreground/20">
      {t("loongport.tier.rate", { value: rate })}
    </span>
  );
}
