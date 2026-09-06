import { Image as ImageIcon, Info } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Switch } from "@/components/ui/switch";
import { useSettingsQuery } from "@/lib/query/queries";
import { useSetImagegenMcpEnabled } from "@/lib/query/imagegen";

/**
 * 生图标签页顶部的说明 + MCP 注册开关。
 *
 * ## 为什么这一页非要有一段说明
 *
 * 这里的档位不写进任何 CLI 配置：直接生图由 LoongPort 自己调接口，CLI 对话里
 * 生图走 LoongPort 自带的 MCP 工具、生图时才现读档位。用户如果不知道这件事，
 * 会得出两个错的结论：
 *
 * 1. 「选了这个档位，我的对话也变成生图模型了」—— 于是不敢选。
 * 2. 「这里选完就能直接用了吧」—— 而 CLI 里第一次用要新开终端加载工具。
 *
 * 上一版没有这段说明，实测维护者自己都报「没看到哪里有生图的按钮」。这段文字
 * 不是装饰，是那些失败的直接修复。
 *
 * ## MCP 开关为什么放在这里
 *
 * 开关只管「要不要把生图工具注册进 codex / claude / gemini」。关掉它的是只想
 * 直接生图的用户 —— 工具注册着就占宿主每次会话的上下文，他们要的是根本不注册。
 * 空态（`empty`）不渲染开关：没有档位时本来就什么都不会注册。
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
  const { data: settings } = useSettingsQuery();
  const setMcpEnabled = useSetImagegenMcpEnabled();

  // 后端缺省（undefined）= 开：与 `settings::get_imagegen_mcp_enabled` 的回落一致。
  const mcpEnabled = settings?.imagegenMcpEnabled ?? true;

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
      {/* 标题就是那句最要紧的话，不是页面名 —— 用户扫一眼只会读加粗的这一行。 */}
      <AlertTitle>{t("loongport.imageTab.companionNotice")}</AlertTitle>
      <AlertDescription className="space-y-2 text-muted-foreground">
        <p>{t("loongport.imageTab.companionDetail")}</p>
        <div className="flex items-center gap-2">
          <Switch
            checked={mcpEnabled}
            onCheckedChange={(checked) => setMcpEnabled.mutate(checked)}
            disabled={setMcpEnabled.isPending}
            aria-label={t("loongport.imageTab.mcpSwitchLabel")}
          />
          <span className="text-xs">
            {t("loongport.imageTab.mcpSwitchLabel")}
          </span>
        </div>
        <p className="text-xs">{t("loongport.imageTab.mcpSwitchHint")}</p>
      </AlertDescription>
    </Alert>
  );
}
