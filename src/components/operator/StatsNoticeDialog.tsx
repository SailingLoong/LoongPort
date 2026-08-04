import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Check, X } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { operatorApi, settingsApi } from "@/lib/api";
import { generateUUID } from "@/utils/uuid";
import type { Settings } from "@/types";

/**
 * 匿名使用统计的首启告知。
 *
 * ## 为什么必须有这一屏
 *
 * 统计**默认开**（维护者拍板：默认关的实际参与率不到 5%，那时数据严重偏向折腾型用户，
 * 比没有数据更误导）。而默认开的前提是**用户真的知道**这件事 ——
 * 「设置里有个开关」不算告知，那要用户自己去翻。
 *
 * 所以：**在第一次上报发生之前**弹一次，明确列出报什么、不报什么，两个按钮同等显眼
 * （「不参与」是 ghost 而非小字链接：两个都可见、主次分明，
 * 不做把拒绝按钮藏起来那种暗黑模式）。
 *
 * ## ⚠️ 端点还没配时**这一屏根本不弹**（2026-08-04 加的前置条件）
 *
 * `stats::ENDPOINT` 目前还是占位（含 `.invalid`），上报端还没建 ⇒
 * **同意与不同意的实际后果完全相同**，一个字节都不会发出去。
 *
 * 那时弹这一屏是**向用户征求一个没有意义的同意**：消耗掉用户对弹窗的注意力与信任，
 * 却换不到任何数据。所以触发条件是「端点已配 且 用户没表态过」两条。
 *
 * ⇒ **端点配好那天自动开始弹**，不需要有人记得回来撤掉一个开关 ——
 * 判据是端点本身（`operatorApi.statsEndpointConfigured`，后端同一个
 * `stats::is_configured`，与上报任务那道闸共用），不是另行维护的标记。
 *
 * ⚠️ **别因为「现在不弹」就把这一屏、后端上报链路或文案删掉** —— 端点建好之后
 * 立刻就要用。它现在是完整可用的，只是在等一个外部条件（记在 `TODO.md`）。
 *
 * ## 上报在用户表态之前不会发生
 *
 * 后端那个上报任务的门禁是 `stats_notice_confirmed === true`
 * （`lib.rs` 里那段，还额外要求 `stats_install_id` 存在）——
 * 用户没点过这个弹窗，一个字节都不会发出去。「告知之前先报一次」是自相矛盾的。
 *
 * ## `installId` 在用户点「好」的那一刻才生成
 *
 * 不在装机时预生成：那样即便用户选了不参与，机器上也躺着一个为统计准备的 id。
 * 用户同意了才有 id，这样「不参与」是真的什么都没有。
 */
export function StatsNoticeDialog() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    // 两个事实都要：端点配好了没（后端 `stats::is_configured`）、用户表过态没。
    Promise.all([operatorApi.statsEndpointConfigured(), settingsApi.get()])
      .then(([endpointConfigured, s]) => {
        if (cancelled) return;
        setSettings(s);
        // 两个条件都成立才弹：
        // - 端点已配 —— 没配时同意与不同意后果相同，问了也白问（见组件文档）
        // - `undefined` = 还没表态过；已经表过态的（无论选了什么）不该再被打扰
        setOpen(endpointConfigured && s.statsNoticeConfirmed === undefined);
      })
      // 任何一个读失败就不弹：宁可这次不告知（那时也不会上报，因为门禁读的是同一份
      // 设置与同一个端点判据），也不能因为一个统计功能在启动时弹一个报错。
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const respond = async (participate: boolean) => {
    if (!settings || saving) return;
    setSaving(true);
    try {
      // `webdavSync` 是只读的聚合字段，save 时要摘掉（与仓里其它调用点一致）。
      const { webdavSync: _webdavSync, ...rest } = settings;
      await settingsApi.save({
        ...rest,
        statsNoticeConfirmed: true,
        enableAnonymousStats: participate,
        // 同意才生成 id；不参与就**根本不生成**（见组件文档最后一节）。
        ...(participate
          ? // 用仓里那个 `generateUUID`（`utils/uuid.ts`）而不是裸
            // `crypto.randomUUID()`：后者在旧 WebView 上不存在会直接 throw ⇒
            // 落到下面的 catch ⇒ 弹窗静默关掉、用户以为参与了却根本没有 id。
            // 那个工具带 `getRandomValues` 的 fallback（§一：能复用就复用）。
            { statsInstallId: settings.statsInstallId ?? generateUUID() }
          : {}),
      });
      setOpen(false);
    } catch {
      // 存不进去就下次再弹（`statsNoticeConfirmed` 没写成功 ⇒ 下次启动还是 undefined）。
      // 而没写成功也意味着不会上报 —— 门禁与这个标记是同一个字段，天然一致。
      setOpen(false);
    } finally {
      setSaving(false);
    }
  };

  if (!open) return null;

  return (
    <Dialog open onOpenChange={() => {}}>
      {/* 有意不给关闭途径（没有 X、点外面不关）：这一屏就是要一个明确表态。
          两个按钮都能让它消失，所以不是死锁。

          ## 为什么不用 DialogHeader / DialogFooter
          
          那两个壳自带 `border-b` / `border-t` + `bg-muted/20`（见 `ui/dialog.tsx`），
          是给**表单类弹窗**设计的分区（标题栏 / 操作栏）。用在这一屏上会把
          「一段要读的说明」切成三段，看起来像系统报错框而不是邀请。
          所以这里直接在 DialogContent 里排版 —— 仍然用官方组件，只是不套那两个分区。 */}
      <DialogContent className="max-w-[26rem] gap-0 p-6" zIndex="top">
        <DialogTitle className="text-base font-semibold">
          {t("loongport.stats.title")}
        </DialogTitle>
        <DialogDescription className="mt-1.5 text-sm leading-relaxed">
          {t("loongport.stats.intro")}
        </DialogDescription>

        {/* 「会上报 / 不会上报」是**语义相反**的两块，所以给它们相反的视觉：
            绿勾 vs 红叉。只加粗标题的话两块长得一样，用户得逐字读才分得开 ——
            而这一屏的全部意义就是让他一眼看懂边界。
            图标形状抄 `AboutSection.tsx:1071`（`mt-0.5 h-4 w-4 shrink-0` + 语义色）。 */}
        <div className="mt-4 space-y-2.5 rounded-lg bg-muted/40 p-3.5">
          <div className="flex gap-2.5">
            <Check className="mt-0.5 h-4 w-4 shrink-0 text-green-500" />
            <p className="text-[13px] leading-relaxed">
              <span className="font-medium">
                {t("loongport.stats.sendsLabel")}
              </span>
              <span className="text-muted-foreground">
                {" "}
                {t("loongport.stats.sendsBody")}
              </span>
            </p>
          </div>
          <div className="flex gap-2.5">
            <X className="mt-0.5 h-4 w-4 shrink-0 text-red-500" />
            <p className="text-[13px] leading-relaxed">
              <span className="font-medium">
                {t("loongport.stats.neverLabel")}
              </span>
              <span className="text-muted-foreground">
                {" "}
                {t("loongport.stats.neverBody")}
              </span>
            </p>
          </div>
        </div>

        {/* ⚠️ **必须披露那个持久的安装标识**（review 纠正）。
            原来的文案说「不报任何能认出你的标识」—— 那是**过强的表述**：
            `install_id` 是稳定持久的，加上稀有 host 的组合与接收端看得到的 IP，
            足以把多次上报关联到同一个安装。说「完全匿名」是不诚实的。
            所以这一行单独把它讲出来，而不是藏在「不报」那一栏的反面。 */}
        <p className="mt-3 text-xs leading-relaxed text-muted-foreground">
          {t("loongport.stats.idNote")}
        </p>
        <p className="mt-1.5 text-xs text-muted-foreground">
          {t("loongport.stats.canChange")}
        </p>

        <div className="mt-5 flex justify-end gap-2">
          {/* 「不参与」用 `ghost` 而不是 `outline`：后者在这个主题下带 primary 描边，
              于是它比主按钮还抢眼（实机截图里就是这个毛病）。
              ghost 保证「两个选项都可见」但主次分明 —— 那才是默认开该有的样子，
              而不是把拒绝按钮做得更显眼。 */}
          <Button
            variant="ghost"
            size="sm"
            disabled={saving}
            onClick={() => void respond(false)}
          >
            {t("loongport.stats.decline")}
          </Button>
          <Button
            size="sm"
            disabled={saving}
            onClick={() => void respond(true)}
          >
            {t("loongport.stats.accept")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
