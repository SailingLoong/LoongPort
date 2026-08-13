import type { TFunction } from "i18next";
import { toast } from "sonner";

import type { AppId } from "@/lib/api";
import type { ProvisionSummary } from "@/lib/api/relay";
import { countTiersForApp, tiersLandedElsewhere } from "./provisionScope";

/**
 * 「备好密钥」这一步的结果播报。
 *
 * ## 为什么提成独立模块（2026-08-04）
 *
 * 它有**两个跨文件消费者**：`RelaySection`（行内登录 / 获取密钥两条路）与
 * `RelayDirectoryPage`（推荐站 / 手动站点那条路）。放同目录的局部模块而不是内联复制 ——
 * 尺子3 说的是「共享代码先放局部 helper，跨文件复用到 3+ 才提 package 级」，
 * 同目录模块正是那个局部落点，`lowBalance.ts` 是同形先例（2 个消费者、同目录、纯逻辑）。
 *
 * 而这里要守的正是「两处一致」：`failures` 是「某个分组的密钥没建出来」，
 * 静默吞掉它的后果是用户以为全部分组都备好了，直到点那一条、拿到一个看不懂的
 * 401 才发现。哪一个入口漏播报，那个入口的用户就少这一次预警。
 *
 * ## 为什么 `t` 是参数而不是在模块里调 i18n 单例
 *
 * 两个消费者都是组件，`useTranslation()` 拿到的 `t` 已经在手上；这里再取一次
 * 单例会绕开 React 的语言切换订阅（切语言后这几条 toast 会停在旧语言）。
 * 仓里 `lib/api/model-fetch.ts` 与 `lib/errors/skillErrorParser.ts` 都是这个形状。
 */
export function reportProvision(
  t: TFunction,
  r: ProvisionSummary,
  appId: AppId,
): void {
  // ⚠️ **报的是「当前这一屏拿到了几个」，不是 `r.tiers.length`** —— 后者是
  // 全平台总数（provision 一次探全部平台）。用总数会说出「已备好 4 个档位」
  // 而用户面前那一屏是空的，那比不说更糟（他会以为是渲染坏了）。
  const mine = countTiersForApp(r, appId);
  const elsewhere = tiersLandedElsewhere({
    currentAppTiers: mine,
    totalTiers: r.tiers.length,
    // 有分组失败时不说「落在别的平台」—— 那会把「建密钥失败、重试有意义」
    // 错归成「这个平台压根没有分组」。见 `tiersLandedElsewhere`。
    failureCount: r.failures.length,
  });

  if (elsewhere > 0) {
    // 这个站有分组，只是没有当前平台的那种。**说清「去哪儿看」** ——
    // 不然用户只会一遍遍点「获取密钥」，而那条路对他永远不会有结果（债 11）。
    toast.info(
      t("loongport.provision.landedOnOtherPlatforms", { count: elsewhere }),
    );
  } else {
    // 两个分支各是一个**完整句**，不是「主句 + 可拼接片段」。原来中文靠
    // 「，新建 N 把密钥」这种前置逗号粘在后面 —— 那种拼法在英/日语序下会散架。
    //
    // ⚠️ **`count` 与 `keys` 不同源，这是有意的**：前者是当前平台的档位数，
    // 后者是这次**为这个账号**新建的 sk 总数（跨平台）。语义上后者本来就是账号级的
    // ——「你账号里多了几把密钥」，而它的价值正在于跨平台可见（每次都在新建
    // 说明认领逻辑坏了，正在堆垃圾 Key）。把它也筛成当前平台会把那个信号削弱。
    toast.success(
      r.keysCreated > 0
        ? t("loongport.provision.readyWithKeys", {
            count: mine,
            keys: r.keysCreated,
          })
        : t("loongport.provision.ready", { count: mine }),
    );
  }
  // 部分失败如实说出来，但不阻断 —— 成功的那些能用。
  for (const f of r.failures) {
    toast.warning(
      t("loongport.provision.groupFailed", {
        group: f.groupName,
        reason: f.reason,
      }),
    );
  }
  if (r.mergedProviders.length > 0) {
    toast.info(
      t("loongport.provision.mergedProviders", {
        count: r.mergedProviders.length,
      }),
    );
  }
}
