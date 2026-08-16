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
  // 顶栏大「+」统一添加入口的触发提示。
  "addEntry.title",
  // 独立中转站广场。四个榜单、数据来源、评分证据、认证动作、缓存状态和分页
  // 都是用户直接看到的主流程文案；漏一个 locale 会直接显示 key 名。
  "directory.title",
  "directory.description",
  "directory.tabs.overall",
  "directory.tabs.claude",
  "directory.tabs.openai",
  "directory.tabs.gemini",
  "directory.source.syncedAt",
  "directory.searchPlaceholder",
  "directory.loading",
  "directory.errorTitle",
  "directory.errorBody",
  "directory.refreshFailed",
  "directory.empty",
  "directory.scoreLabel",
  "directory.signatureHint",
  "directory.compatibilityNote",
  // 独立「官方 API」页（厂商选择 → 登录 → 备钥），与广场并列的子流程。
  "officialApi.title",
  "officialApi.description",
  "officialApi.connect",
  // 厂商卡片的说明文案 —— 每接一家都要有，漏一个 locale 那张卡就显示 key 名。
  "officialApi.deepseekDesc",
  "officialApi.bigmodelDesc",
  "officialApi.opencodeDesc",
  "directory.provisionFailed",
  "directory.meta.samples",
  "directory.meta.latest",
  "directory.actions.authenticate",
  "directory.actions.autoAddHint",
  "directory.actions.history",
  "directory.actions.refresh",
  "directory.actions.retry",
  "directory.pagination.range",
  "directory.pagination.previous",
  "directory.pagination.next",
  "tierList.empty",
  "tierList.addSite",
  "tierList.refresh",
  "row.expand",
  "row.collapse",
  "row.notLoggedIn",
  "row.login",
  "row.sessionExpired",
  "row.reLogin",
  // 「登录过期但密钥还能用」这个反直觉状态**唯一的解释**（两类行共用一句）。
  // 后端把「会话」与「密钥」有意分开（`creds::clear_session` 只清会话），
  // 缺它用户会以为分组和密钥一起没了，转而去做一堆不必要的重建。
  "row.sessionExpiredUsable",
  "row.noTiers",
  "row.getKeys",
  "row.tierCount",
  "row.dragHandle",
  "row.refetchGroups",
  "row.purchaseHint",
  // 余额偏低时那个叹号的 title。**缺了最伤**：叹号本身不带文字，
  // title 退化成 key 名 ⇒ 用户看到一个说不清为什么的警示图标。
  "row.lowBalanceHint",
  // 删这一行的入口 + 「有档位在用」的预告。后者是强删弹窗之外唯一的预告：
  // 缺了它用户会在毫无预期的情况下点开一个「点名 app」的强删确认框。
  // 强删弹窗那两条（清单项格式 + 主文案）是知情前提 —— 点名哪些 app 在用、
  // 删了会失去什么，退化成 key 名等于让人盲点一个破坏性操作。
  "row.remove",
  "row.removeInUseHint",
  "row.removeConfirmTitle",
  "row.removeConfirmMessage",
  "row.removeConfirmUsageItem",
  "row.removeConfirmMessageInUse",
  // 从没登录过的行走另一句 —— 无条件那句会说错两处（不存在的登录态、没充过的
  // 余额）。判据由后端 `removeConfirmation` 字段给出。
  "row.removeConfirmMessageNeverLoggedIn",
  // 「分组都落在别的平台了」—— 缺了它用户会一遍遍点「获取密钥」，
  // 而那条路对他永远不会有结果（债 11）。
  "provision.landedOnOtherPlatforms",
  // ⚠️ `tier.current` / `tier.use` **有意不在这里** —— 那两处文案复用上游的
  // `provider.inUse` / `provider.enable`（四 locale 早就齐了）。
  // 自建一份会让同一个操作在 provider 页叫「启用」、在这里叫「使用」。
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
  // 官网直连账号（vendor）行。**十一条都要有** —— 这一整行的文案全在这个命名空间里，
  // 缺一条那一行就显示 key 名。
  //
  // 尤其这两条：
  // - `removeConfirmMessage` —— 破坏性操作的知情前提（会删掉什么、官网那把 key
  //   不受影响），退化成 key 名等于让人盲点一个删除
  // - `openKeyPage` —— 超上限时那个「去官网删」的入口（spec §4.3 要求指路而不是
  //   只说不允许），它是个 toast action 的 label，退化成 key 名按钮就没法读
  //
  // ⚠️ 「登录过期但密钥还能用」那句已提升为 `row.sessionExpiredUsable`（见上）——
  // 两类行现在是同一个反直觉状态、同一句解释，不该各留一份。
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

function flattenStringValues(root: unknown): string[] {
  if (typeof root === "string") return [root];
  if (root == null || typeof root !== "object") return [];
  return Object.values(root as Translations).flatMap(flattenStringValues);
}

function visibleCopy(root: unknown): string {
  return flattenStringValues(root)
    .map((value) => value.replace(/\{\{[^}]+\}\}/g, ""))
    .join("\n");
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

  it.each([
    ["zh", zh.loongport, "接入配置", /档位/u],
    ["zh-TW", zhTW.loongport, "連線設定檔", /檔位|方案/u],
    ["en", en.loongport, "connection profile", /\btier(?:s)?\b/iu],
    ["ja", ja.loongport, "接続プロファイル", /プラン|ティア|枠/u],
  ] as const)(
    "%s uses the approved connection-profile terminology",
    (_locale, ns, approvedTerm, deprecatedTerm) => {
      const copy = visibleCopy(ns);

      expect(copy).toContain(approvedTerm);
      expect(copy).not.toMatch(deprecatedTerm);
    },
  );

  it.each([
    ["zh", zh.loongport, /去登录|收编|会上报|不会上报|本地代理观察/u],
    ["zh-TW", zhTW.loongport, /去登入|收編|會上報|不會上報|本機代理/u],
    ["en", en.loongport, /Get keys|Re-fetch available groups|local proxy/iu],
    ["ja", ja.loongport, /キーを取得|ローカルプロキシ/u],
  ] as const)(
    "%s omits internal and conversational wording",
    (_locale, ns, deprecatedWording) => {
      expect(visibleCopy(ns)).not.toMatch(deprecatedWording);
    },
  );

  it.each([
    [
      "zh",
      zh.loongport,
      {
        trusted: "验证通过",
        suspicious: "需要复核",
        anomaly: "检测到异常",
        inconclusive: "证据不足",
      },
    ],
    [
      "zh-TW",
      zhTW.loongport,
      {
        trusted: "驗證通過",
        suspicious: "需要複核",
        anomaly: "偵測到異常",
        inconclusive: "證據不足",
      },
    ],
    [
      "en",
      en.loongport,
      {
        trusted: "Verified",
        suspicious: "Review needed",
        anomaly: "Anomaly detected",
        inconclusive: "Insufficient evidence",
      },
    ],
    [
      "ja",
      ja.loongport,
      {
        trusted: "検証済み",
        suspicious: "要確認",
        anomaly: "異常を検出",
        inconclusive: "証拠不足",
      },
    ],
  ] as const)(
    "%s keeps the approved model-verification verdict semantics",
    (_locale, ns, expectedVerdicts) => {
      expect(ns.modelVerification.verdict).toMatchObject(expectedVerdicts);
    },
  );

  it.each([
    [
      "zh",
      zh.loongport,
      {
        sendsLabel: "将收集",
        neverLabel: "不会收集",
        accept: "允许分享",
        decline: "不分享",
      },
    ],
    [
      "zh-TW",
      zhTW.loongport,
      {
        sendsLabel: "將收集",
        neverLabel: "不會收集",
        accept: "允許分享",
        decline: "不分享",
      },
    ],
    [
      "en",
      en.loongport,
      {
        sendsLabel: "Will collect",
        neverLabel: "Will not collect",
        accept: "Allow sharing",
        decline: "Don't share",
      },
    ],
    [
      "ja",
      ja.loongport,
      {
        sendsLabel: "収集する情報",
        neverLabel: "収集しない情報",
        accept: "共有を許可",
        decline: "共有しない",
      },
    ],
  ] as const)(
    "%s states the approved sharing facts and consent actions",
    (_locale, ns, expected) => {
      expect(ns.stats).toMatchObject({
        sendsLabel: expected.sendsLabel,
        neverLabel: expected.neverLabel,
        accept: expected.accept,
        decline: expected.decline,
      });
    },
  );
});
