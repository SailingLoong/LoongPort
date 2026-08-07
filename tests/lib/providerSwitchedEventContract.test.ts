import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * 闸：**每条切换供应商的路径都要广播 `provider-switched`，且两侧都有人在听。**
 *
 * ## 这道闸修的 bug（2026-08-04 实测）
 *
 * `tier.isCurrent` 来自后端的**全局** current（`get_effective_current_provider`）——
 * cc-switch 自建的 provider 与 LoongPort 的托管档位**共用同一个字段**。而两个消费者
 * 用的是**两套互不相通的状态**：
 *
 * | 消费者 | 状态机制 | 靠什么刷新 |
 * |---|---|---|
 * | provider 列表 | react-query `["providers", app]` | mutation 的 `onSuccess` invalidate |
 * | 运营商区（`OperatorSection`） | 自己的 `useState` + 手工 `reload()` | **只有自己的动作** |
 *
 * 于是启用一个 cc-switch 的 sk 之后，运营商区里那个托管档位仍显示「使用中」
 * ⇒ `hasCurrentTier` 仍为 true ⇒ **运营商行的删除按钮一直是灰的**，
 * 用户明明切走了却删不掉。反方向同理（切档位后 provider 列表陈旧）。
 *
 * 后端一直是对的（`ProviderService::switch` 确实更新了 current），坏的只是
 * 「不重开窗口就看不到」这一段 —— **静默的界面陈旧**，不报错不崩，
 * 用户会以为是删除功能坏了。
 *
 * ⚠️ 两个消费者现在同处一页（运营商区在 provider 页顶部），但**事件仍然必须发** ——
 * 它们是两套独立的状态，同页不等于同步。
 *
 * ## 为什么是闸而不是「记得发」
 *
 * 上游只在托盘 / 故障转移 / 应用项目三处发这个事件，`switch_provider` 不发 ——
 * 上游没事（它只有一个消费者）。**这个缺口是 fork 特有的**，而将来上游新增第四条
 * 切换路径时同样不会想到我们这个额外消费者。所以判据要做成
 * 「凡切换路径必发事件」，而不是钉死当前那两处。
 *
 * 会红的改法：任一条切换路径不再发事件；`OperatorSection` 不再监听；
 * 事件名在任一侧被改。
 */
const read = (rel: string) => readFileSync(resolve(process.cwd(), rel), "utf8");

describe("provider-switched 的跨页面契约", () => {
  it("两条切换命令都广播事件", () => {
    // `switch_provider`（provider 页那个「启用」）—— 上游漏的就是这处。
    // ⚠️ emit_provider_switched 的实现定义在 events.rs（2026-08-07 从 commands/provider.rs
    // 移过去，因为 deeplink 等 crate 级模块访问不到私有的 commands::provider），
    // 命令层 import 它来发事件。
    const eventsRs = read("src-tauri/src/events.rs");
    expect(
      eventsRs,
      "events.rs 里找不到 emit_provider_switched 的定义",
    ).toMatch(/fn emit_provider_switched/);

    const providerRs = read("src-tauri/src/commands/provider.rs");
    expect(
      providerRs,
      "switch_provider 成功后没有广播 —— 启用别的 provider 之后，" +
        "运营商区里那个档位会一直显示「使用中」、删除按钮一直是灰的",
    ).toMatch(/emit_provider_switched\(&emit_handle/);

    // `operator_switch_tier`（运营商区切档位）—— 镜像方向。
    const operatorRs = read("src-tauri/src/commands/operator.rs");
    expect(
      operatorRs,
      "operator_switch_tier 没有广播 —— 切完档位之后 provider 列表会陈旧",
    ).toMatch(/emit_provider_switched\(/);
  });

  it("发射用的事件名与前端监听的逐字相同", () => {
    // 事件名常量收在 events.rs / events.ts 两侧（一致性闸在 events.rs 的
    // consistency_tests 里守逐字一致）。这里验证三件事：
    // 1) 两侧常量值确实是 "provider-switched"
    // 2) emit_provider_switched 用的是常量而不是裸字面量（以后改事件名只改一处）
    // 3) 前端两个消费者都从常量 import，而不是自己写裸字符串
    const eventsRs = read("src-tauri/src/events.rs");
    expect(eventsRs).toMatch(
      /pub const PROVIDER_SWITCHED:\s*&str\s*=\s*"provider-switched"/,
    );

    expect(
      eventsRs,
      "emit_provider_switched 该用 PROVIDER_SWITCHED 常量 —— 裸字面量会让事件名失去唯一源",
    ).toMatch(/\.emit\(PROVIDER_SWITCHED,\s*payload\)/);

    const eventsTs = read("src/lib/api/events.ts");
    expect(eventsTs).toMatch(
      /export const PROVIDER_SWITCHED\s*=\s*"provider-switched"/,
    );

    // 前端两个消费者都用常量。
    expect(read("src/lib/api/providers.ts")).toMatch(
      /listen\(PROVIDER_SWITCHED/,
    );
    expect(read("src/components/operator/OperatorSection.tsx")).toMatch(
      /useTauriEvent<ProviderSwitchEvent>\(PROVIDER_SWITCHED/,
    );
  });

  it("payload 带 appType 与 providerId（前端契约要求的两个字段）", () => {
    // 函数定义在 events.rs（见上面那个 it 的说明）。
    const eventsRs = read("src-tauri/src/events.rs");
    // 从 emit_provider_switched 的函数体里取 payload 那段。
    const body = eventsRs.slice(eventsRs.indexOf("fn emit_provider_switched"));
    const payload = body.slice(0, body.indexOf(".emit("));
    expect(
      payload,
      "payload 少了 appType —— 前端靠它过滤自己那个 app",
    ).toContain('"appType"');
    expect(payload, "payload 少了 providerId").toContain('"providerId"');
  });

  it("OperatorSection 只在事件属于本 tab 那个 app 时才刷新", () => {
    // 不过滤会让「切 claude 的供应商」也触发 codex 这一区重新拉一遍 —— 多余的
    // 网络请求（每个档位一次倍率查询）。
    const section = read("src/components/operator/OperatorSection.tsx");
    expect(section).toMatch(/appType\s*!==\s*appId/);
    expect(section).toMatch(/void reload\(\)/);
  });
});
