import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import zh from "@/i18n/locales/zh.json";

/**
 * 「中转站 × 分组」页用到的全部 i18n key（`loongport.*` 命名空间）。
 *
 * ## 为什么必须有这条测试
 *
 * 托管档位原本显示在**已 i18n** 的 `ProviderList` 里（27 处 `t()`）。用硬编码中文的
 * 新组件替换它，会让 en / ja / zh-TW 用户的**主屏幕退化** —— 那是回归，不是「暂不支持」。
 * 所以四个 locale 必须同时齐全，而「同时齐全」这件事只有测试守得住：
 * 加一个 key 时漏改三份 locale，界面上表现为直接显示 key 名（如 `loongport.tier.resetConfig`），
 * 而中文用户完全看不到这个问题。
 *
 * 照 `toolManagementLocales.test.ts` / `xaiOauthLocales.test.ts` 的形状写（仓库已有惯例）。
 */
const requiredKeys = [
  // 三大块的区块标题。
  "sections.relay",
  "sections.official",
  "sections.other",
  "tierList.empty",
  "tierList.addSite",
  "tierList.refresh",
  "row.expand",
  "row.collapse",
  "row.notLoggedIn",
  "row.login",
  "row.sessionExpired",
  "row.reLogin",
  "row.noTiers",
  "row.getKeys",
  "row.tierCount",
  "row.dragHandle",
  "row.refetchGroups",
  "row.purchaseHint",
  // 余额偏低时那个叹号的 title。**缺了最伤**：叹号本身不带文字，
  // title 退化成 key 名 ⇒ 用户看到一个说不清为什么的警示图标。
  "row.lowBalanceHint",
  // 删这一行的入口 + 「有档位在用所以不能删」的解释。
  // 后者缺了最伤：按钮变灰但没有 title ⇒ 用户完全不知道为什么点不动。
  "row.remove",
  "row.removeBlockedByCurrent",
  "row.removeConfirmTitle",
  "row.removeConfirmMessage",
  // 从没登录过的行走另一句 —— 无条件那句会说错两处（不存在的登录态、没充过的
  // 余额）。判据见 `components/relay/removeConfirmWording.ts`。
  "row.removeConfirmMessageNeverLoggedIn",
  // 「分组都落在别的平台了」—— 缺了它用户会一遍遍点「获取密钥」，
  // 而那条路对他永远不会有结果（债 11）。
  "provision.landedOnOtherPlatforms",
  // ⚠️ `tier.current` / `tier.use` **有意不在这里** —— 那两处文案复用上游的
  // `provider.inUse` / `provider.enable`（四 locale 早就齐了）。
  // 自建一份会让同一个操作在 provider 页叫「启用」、在这里叫「使用」。
  "tier.rateUnknown",
  "tier.rate",
  "tier.switching",
  "tier.checkConnectivity",
  // 「恢复默认配置」那条兜底路径。**五条都要有** —— 它覆盖用户的全部编辑，
  // 确认弹窗退化成 key 名等于让人在看不懂的提示上点「确定」。
  "tier.resetConfig",
  "tier.resetConfirmTitle",
  "tier.resetConfirmMessage",
  "tier.resetConfirmButton",
  "tier.resetDone",
  // 「编辑配置」入口 + 它的事前警告。**四条都要有** —— 那个警告是用户接手维护
  // 这个档位的知情前提（保存后刷新不再覆盖它），退化成 key 名等于没警告。
  "tier.edit",
  "tier.editConfirmTitle",
  "tier.editConfirmMessage",
  "tier.editConfirmButton",
  // 编辑**当前正在用**的档位时多给的一句：改完可能被 ChatGPT 回写覆盖。
  // `update_provider` 那条路没有「退 ChatGPT → 写 → 重开」的编排（只有切换才有），
  // 所以这句是用户唯一会收到的提醒 —— 缺它他会以为改动丢了。
  "tier.editCurrentNote",
  // 「记录不在了」那条：数据不一致时的出路。缺它用户会收到一个 key 名当错误。
  "tier.editMissing",
  // 「已手动维护」标记与它的说明。**两条都要有** —— 标记是给用户看的状态，
  // 显示成 key 名比不显示更糟（用户会以为界面坏了）。
  "tier.userEdited",
  "tier.userEditedHint",
  // 使用统计的首启告知。**十条都要有** —— 缺一条那一屏就会显示 key 名，
  // 而那一屏是「默认开」的知情前提，退化成 key 名等于没告知。
  // 尤其 `stats.idNote`（披露那个持久安装标识）—— 缺它就变成不诚实的告知。
  "stats.title",
  "stats.intro",
  "stats.sendsLabel",
  "stats.sendsBody",
  "stats.neverLabel",
  "stats.neverBody",
  "stats.idNote",
  "stats.canChange",
  "stats.accept",
  "stats.decline",
  // 一键「切回官方登录」。它会删掉用户的 codex 登录态（OAuth refresh token），
  // 所以确认弹窗那三条尤其不能退化成 key 名 —— 那等于让人盲点一个破坏性操作。
  "official.button",
  "official.hint",
  "official.confirmTitle",
  "official.confirmMessage",
  "official.confirmButton",
  "official.done",
  "official.doneWithBackup",
  // 按语义分组而不按宿主组件命名 —— 这几条原本写在已删的 `OperatorPanel` 里，
  // 因为分组是语义的，清那个页面时 `RelaySection` 直接接着用，不用重命名一轮。
  // 「添加中转站」弹窗。首启那次它就是整个第一屏，退化成 key 名等于让新用户
  // 对着一屏看不懂的东西输域名。
  "addSite.firstRunTitle",
  "addSite.firstRunBody",
  "addSite.title",
  "addSite.body",
  "addSite.inputLabel",
  "addSite.connected",
  "site.removed",
  // 启动探活发现凭据在服务端已失效时那句。缺它用户会收到一个 key 名当提示，
  // 而这条 toast 是他知道「要重新登录」的唯一途径（界面其余部分看不出区别）。
  "session.expired",
  "session.connected",
  // ⚠️ `ready` / `readyWithKeys` 与 `done` / `doneRelaunched` / `doneNeedsRestart`
  // 是**完整句的分支**，不是可拼接的片段。原来中文靠「，新建 N 把密钥」这种前置逗号
  // 粘在主句后面，那种拼法在英/日语序下会散架 —— 所以每个分支各给一个完整 key。
  "provision.ready",
  "provision.readyWithKeys",
  "provision.groupFailed",
  "provision.refreshed",
  "provision.refreshedWithKeys",
  // ⚠️ 只有带原因那条 —— 无原因的 `refreshFailed` 已删（它只说「刷新失败」，
  // 用户拿不到任何可处置的信息，维护者实测踩过：定位一次要手工 curl 一遍）。
  "provision.refreshFailedWithReason",
  // 「切换前要不要退 ChatGPT」的确认框。**两个宿主 × 两类 provider 共用它**
  // （LoongPort 档位、cc-switch 自带的供应商），所以它的文案不能退化成 key 名 ——
  // 那三条说明（要重启才生效、点取消会中止、部分系统退不掉）是用户做决定的全部依据。
  "quitConfirm.title",
  "quitConfirm.body",
  "quitConfirm.declineNote",
  // Windows 专属：那边只能强制关闭，必须替换掉 declineNote 那句
  // 「会弹确认框、可以取消」的承诺（那边不会弹）。
  "quitConfirm.forceKillWarning",
  "quitConfirm.platformNote",
  "quitConfirm.switchOnly",
  "quitConfirm.quitAndSwitch",
  "switch.done",
  "switch.doneRelaunched",
  "switch.doneNeedsRestart",
  // 官网直连账号（vendor）行。**十二条都要有** —— 这一整行的文案全在这个命名空间里，
  // 缺一条那一行就显示 key 名。
  //
  // 尤其这三条：
  // - `removeConfirmMessage` —— 破坏性操作的知情前提（会删掉什么、官网那把 key
  //   不受影响），退化成 key 名等于让人盲点一个删除
  // - `sessionExpiredUsable` —— 「登录过期但密钥还能用」这个反直觉状态**唯一的解释**
  //   （后端有意把 keyReady 与 loggedIn 分开，缺它用户会以为界面坏了）
  // - `openKeyPage` —— 超上限时那个「去官网删」的入口（spec §4.3 要求指路而不是
  //   只说不允许），它是个 toast action 的 label，退化成 key 名按钮就没法读
  "vendor.add",
  // 官方 API 块的空态占位。
  "vendor.empty",
  "vendor.remove",
  "vendor.removeConfirmTitle",
  "vendor.removeConfirmMessage",
  "vendor.removed",
  "vendor.noKey",
  "vendor.keyReady",
  "vendor.keyCreated",
  "vendor.refreshKey",
  "vendor.sessionExpiredUsable",
  "vendor.loginFailed",
  "vendor.openKeyPage",
  // 官网行专用的两条编辑文案。**不能复用 `tier.*` 那两条** —— 它们写死了
  // 「档位」与「刷新分组」，而官网行不是档位、没有分组（他按的是「获取密钥」），
  // 且一行对应六个平台，所以要说「这个账号在当前这个应用下的配置」。
  "vendor.userEditedHint",
  "vendor.editConfirmMessage",
  "modelVerification.title",
  "modelVerification.titleWithTier",
  "modelVerification.description",
  "modelVerification.globalScopeNote",
  "modelVerification.model.label",
  "modelVerification.model.placeholder",
  "modelVerification.model.loading",
  "modelVerification.model.empty",
  "modelVerification.model.error",
  "modelVerification.actions.start",
  "modelVerification.actions.stop",
  "modelVerification.actions.retry",
  "modelVerification.status.running",
  "modelVerification.tierVerdict.suspicious",
  "modelVerification.tierVerdict.anomaly",
  "modelVerification.tierVerdict.suspiciousHint",
  "modelVerification.tierVerdict.anomalyHint",
  "modelVerification.verdict.trusted",
  "modelVerification.verdict.suspicious",
  "modelVerification.verdict.anomaly",
  "modelVerification.verdict.inconclusive",
  "modelVerification.evidence.title",
  "modelVerification.evidence.level.cryptographic",
  "modelVerification.evidence.level.protocolBehavior",
  "modelVerification.evidence.level.insufficient",
  "modelVerification.evidence.outcome.passed",
  "modelVerification.evidence.outcome.failed",
  "modelVerification.evidence.outcome.skipped",
  "modelVerification.evidence.fact.basicEnvelope",
  "modelVerification.evidence.fact.modelMatch",
  "modelVerification.evidence.fact.streamLifecycle",
  "modelVerification.evidence.fact.usageConsistency",
  "modelVerification.evidence.fact.toolCallShape",
  "modelVerification.evidence.fact.structuredOutput",
  "modelVerification.evidence.fact.thinkingSignature",
  "modelVerification.evidence.fact.signatureContinuation",
  "modelVerification.evidence.fact.foreignProtocol",
  "modelVerification.evidence.fact.foreignSelfIdentification",
  "modelVerification.failure.authentication",
  "modelVerification.failure.rateLimited",
  "modelVerification.failure.insufficientBalance",
  "modelVerification.failure.network",
  "modelVerification.failure.upstream",
  "modelVerification.failure.timeout",
  "modelVerification.failure.modelUnavailable",
  "modelVerification.failure.cancelled",
  "modelVerification.failure.invalidResponse",
] as const;

type Translations = Record<string, unknown>;

const locales = [
  ["en", en.loongport],
  ["ja", ja.loongport],
  ["zh", zh.loongport],
  ["zh-TW", zhTW.loongport],
] as const;

/** 按 `a.b` 取嵌套值。locale 是嵌套结构而 requiredKeys 是点号路径。 */
function lookup(root: unknown, path: string): unknown {
  return path
    .split(".")
    .reduce<unknown>(
      (acc, seg) =>
        acc != null && typeof acc === "object"
          ? (acc as Translations)[seg]
          : undefined,
      root,
    );
}

function interpolationVariables(value: string): string[] {
  return Array.from(value.matchAll(/\{\{([^}]+)\}\}/g), ([, name]) =>
    name.trim(),
  ).sort();
}

describe("LoongPort tier page locale coverage", () => {
  it.each(locales)("defines every loongport key in %s", (_locale, ns) => {
    const missing = requiredKeys.filter((key) => {
      const value = lookup(ns, key);
      return typeof value !== "string" || value.trim().length === 0;
    });

    expect(missing).toEqual([]);
  });

  // 插值变量对不上会让文案里出现空洞（如「另有  个分组」）—— 那种缺陷只在
  // 对应语言下可见，中文测过了也发现不了。
  it.each(locales.slice(1))(
    "preserves interpolation variables in %s",
    (_locale, ns) => {
      const mismatched = requiredKeys.filter((key) => {
        const base = lookup(en.loongport, key);
        const value = lookup(ns, key);
        if (typeof base !== "string" || typeof value !== "string") return false;
        return (
          interpolationVariables(base).join(",") !==
          interpolationVariables(value).join(",")
        );
      });

      expect(mismatched).toEqual([]);
    },
  );
});
