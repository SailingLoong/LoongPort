import type { AppId } from "@/lib/api";
import type { ProvisionSummary } from "@/lib/api/relay";

/**
 * 「这次 provision 拉到的档位，有几条落在了**别的**平台」。
 *
 * ## 为什么需要它（TODO 债 11）
 *
 * `provision` 一次探**全部平台**，每个分组按自己的 `platform` 落到对应 CLI。
 * 于是某个站压根没有 anthropic 分组时，claude 那一屏是零档位 —— 而它与
 * **拉取失败**在界面上长得一样（都落到「该账号在此平台下没有可用分组」）。
 *
 * 那句话没说错，但它把两种处境说成了一种：
 *
 * - **真的没有** —— 这个站不卖 Claude 分组，再点一百次「获取密钥」也不会有。
 *   用户该做的是切到别的平台 tab，或换一个站。
 * - **拉失败了** —— 网络 / sk 被撤销 / 服务端 5xx，重试有意义。
 *
 * 区分它们所需的信息本来就在 `ProvisionSummary` 里（每条档位带 `appId`），
 * 少的只是把它说出来。
 *
 * ## 判据只在「当前平台一条都没拿到」时才成立
 *
 * 当前平台拿到了档位就什么都不用说 —— 那一行会显示档位数，用户看得见结果。
 * 此时别的平台也拿到了是**常态**（一次探全部平台），报出来只是噪音。
 *
 * @returns 落在别的平台的档位数；`0` = 没什么要说的
 */
export function tiersLandedElsewhere(counts: {
  currentAppTiers: number;
  totalTiers: number;
  /** 这次有几个分组建密钥失败了（`ProvisionSummary.failures.length`）。 */
  failureCount?: number;
}): number {
  // 当前平台有档位 ⇒ 结果看得见，不必解释。
  if (counts.currentAppTiers > 0) return 0;
  // ⚠️ **有分组失败时保持沉默** —— 此刻「当前平台零档位」的原因可能是那条分组
  // **建密钥失败了**（Key 上限 / 服务端 5xx），而不是「这个平台没有分组」。
  // 说成后者是错误归因，会把用户指向别的 tab ⇒ 他永远不去重试那条真能修好的路。
  // 失败原因由 `failures` 那几条 warning 逐条说，这里不抢话。
  //
  // ⚠️ **代价：这一类会漏报**（review 抓出，知情接受）。「站上真的没有当前平台的
  // 分组」+「别的平台有一条建 sk 失败」同时成立时，本该指路却沉默了 ⇒ 用户只看到
  // 一条与他这屏无关的 warning，继续点「获取密钥」。选择沉默是因为**错误归因比
  // 漏报更糟**：前者会让他跑去别的 tab 找一个不存在的东西。
  //
  // 真修要给 `ProvisionSummary.failures` 带上 platform（现在是
  // `(group_name, reason)`，分不出失败的那条属于哪个平台）—— 那是另一笔债。
  if ((counts.failureCount ?? 0) > 0) return 0;
  // 全平台皆空 ⇒ 没有「别的平台」可指，说了就是编一个不存在的去处。
  // （后端此时本来就报错走 catch 分支，这里仍要挡住 —— 将来它放宽成
  // 「返回空 summary」时才不会静默说错话。）
  return counts.totalTiers;
}

/**
 * 从一次 provision 的结果里数出「落在当前平台的」有几条。
 *
 * ⚠️ **别拿 `r.tiers.length` 当当前平台的档位数** —— 那是全平台的总数，
 * 用它判断会得出「明明拉到了」而用户那一屏还是空的。
 */
export function countTiersForApp(r: ProvisionSummary, appId: AppId): number {
  return r.tiers.filter((t) => t.appId === appId).length;
}

/**
 * 跨多个中转站累加「落在当前平台的档位数」——顶部「刷新」那条批量路径用。
 *
 * ⚠️ **同样别累 `r.tiers.length`**：三个中转站各有 codex/claude/gemini 分组时，
 * 用户在 codex 屏会被告知「共 9 个档位」而他眼前只有 3 个。数字对不上时他分不清
 * 是提示错了还是界面漏了，而那句还是绿色的成功语气。
 *
 * ## 为什么值得单独一个函数（而不是在组件里 `+=`）
 *
 * 它是**这条判据唯一能被闸钉住的形态**。内联在 `handleRefreshAll` 的 `forEach`
 * 里时，要测它就得渲染整个面板并 mock 十几个命令 —— 那种测试脆且贵，实际结果是
 * 那一行没有任何覆盖（实测：把它改回 `tiers.length`，全部 678 条测试照样绿）。
 *
 * 只跳过失败项由调用方决定（它还要收集失败原因），所以这里吃的是**已筛出的成功项**。
 */
export function sumTiersForApp(
  summaries: readonly ProvisionSummary[],
  appId: AppId,
): number {
  return summaries.reduce((sum, r) => sum + countTiersForApp(r, appId), 0);
}
