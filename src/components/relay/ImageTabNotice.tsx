import { Image as ImageIcon, Info } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";

/**
 * 生图标签页顶部的说明。
 *
 * ## 为什么这一页非要有一段说明
 *
 * 它是唯一一个**不能独立使用**的标签（与 `claude-desktop` 那种「同一个 CLI 的另一种
 * 用法」还不同）：这里选的档位不写进任何 CLI 配置，而是被 LoongPort 自带的 MCP 工具
 * 在生图时现读。用户如果不知道这件事，会得出两个错的结论：
 *
 * 1. 「选了这个档位，我的对话也变成生图模型了」—— 于是不敢选。
 * 2. 「这里选完就能直接用了吧」—— 而第一次装工具确实要新开一个终端。
 *
 * 上一版没有这段说明（入口是档位行上 hover 才出现的一个小按钮），实测的结果是维护者
 * 自己都报「没看到哪里有生图的按钮」，找到之后又顺手点了「启用」把聊天档位切成生图档位
 * ⇒ 连着两次 503。所以这段文字不是装饰，它是那两次失败的直接修复。
 *
 * ## 为什么用 `Alert` 而不是自己画一个框
 *
 * 上游已有（`components/ui/alert.tsx`，标准 shadcn 封装）。CLAUDE.md §一：
 * 能复用就复用，视觉 token 跟着上游走，新页面与旧页面放一起看不出是两个人写的。
 *
 * ## `empty` 那一支
 *
 * 没有任何生图档位时换一套文案：这时最该说的不是「怎么用」，而是**「你的站可能压根
 * 没有生图分组，这一页空着是正常的」** —— 否则用户会以为是自己漏了某个步骤，
 * 去反复点「获取密钥」。
 */
export function ImageTabNotice({ empty }: { empty: boolean }) {
  const { t } = useTranslation();

  if (empty) {
    return (
      <Alert className="border-violet-500/30 bg-violet-500/5">
        <ImageIcon className="h-4 w-4 text-violet-600 dark:text-violet-400" />
        <AlertTitle>{t("loongport.imageTab.emptyTitle")}</AlertTitle>
        <AlertDescription className="text-muted-foreground">
          {t("loongport.imageTab.emptyBody")}
        </AlertDescription>
      </Alert>
    );
  }

  return (
    <Alert className="border-violet-500/30 bg-violet-500/5">
      <Info className="h-4 w-4 text-violet-600 dark:text-violet-400" />
      {/* 标题就是那句最要紧的话（「不能单独使用」），不是页面名 ——
          用户扫一眼只会读加粗的这一行。 */}
      <AlertTitle>{t("loongport.imageTab.companionNotice")}</AlertTitle>
      <AlertDescription className="space-y-1.5 text-muted-foreground">
        <p>{t("loongport.imageTab.companionDetail")}</p>
        <p>{t("loongport.imageTab.howTo")}</p>
      </AlertDescription>
    </Alert>
  );
}
