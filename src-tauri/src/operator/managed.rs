//! 「这条 provider 是不是 LoongPort 托管的」——**唯一判据来源**。
//!
//! ## 为什么要单独一个文件
//!
//! 判据是 **id 前缀 + 恰好 16 位小写 hex**（[`provision::provider_id_for`](super::provision::provider_id_for)
//! 与 [`crate::vendor::provision::provider_id_for`] 两个生成端的输出形状），
//! 用前缀而不是往 `ProviderMeta` 里加字段：那是上游的结构，加字段会扩大与上游 merge 的接触面。
//!
//! 代价是这个前缀字符串会被多处需要（生成、托盘过滤、命令层守卫），一旦散落成三个字面量，
//! 改前缀那天就会漏掉一处、于是某条绕过路径静默复活。所以常量与判据函数都收在这里，
//! 别处只许调用。
//!
//! ## 为什么需要守卫（不只是前端过滤）
//!
//! 切换托管档位的正确入口只有 `operator_switch_tier` —— 它编排的是「退出 ChatGPT →
//! 切换 → 重开」。任何绕过它直接切 provider 的路径，结果都是**界面显示切了、codex 还连着
//! 旧分组**，而用户不会收到任何提示。前端那道过滤（`isManagedProviderId`）挡不住托盘菜单与命令层，
//! 所以守卫必须落在 Rust 侧。

use crate::provider::Provider;

/// 托管 provider 的 id 前缀。
///
/// ⚠️ **改它等于所有已生成的 provider 记录当场脱管**（判据失配 → 守卫全线失效，
/// 且 provision 会为同一分组再插一条新 id）。与 Key 命名契约同属不可逆决定，别顺手改。
pub const MANAGED_ID_PREFIX: &str = "loongport-";

/// 撞到托管档位时给用户的话。指路而不是只说「不允许」—— 用户要知道去哪儿操作。
///
/// ⚠️ **指的地方必须真的存在**：这句原来写「请在 LoongPort 页面里操作」，
/// 而那个独立页 2026-08-04 已删 ⇒ 那句话在指一个不存在的入口。托管档位现在的
/// 全部操作（登录 / 获取密钥 / 切档位）都在供应商页顶部那一区。
const MANAGED_GUARD_MESSAGE: &str = "这是 LoongPort 托管的档位，请在供应商页顶部的运营商区操作";

/// vendor（官网直连）那支在前缀之后多加的一段。
///
/// 两个生成端的形状必须都被 [`is_managed`] 认出来，所以这个段也收在这里 ——
/// 它是判据的一部分，不只是命名习惯。事实源：`vendor::provision::provider_id_for`。
const VENDOR_SEGMENT: &str = "vendor-";

/// 派生 id 尾部那段 hex 的长度。**两个生成端都取 16 位**
/// （`format!("{:.16x}")`），改任一处都要改这里，否则守卫当场对全部已有记录失效。
const HEX_LEN: usize = 16;

/// 这个 provider id 是不是 LoongPort 托管的。
///
/// ## 为什么不只判前缀（2026-08-04 收紧）
///
/// 判据要回答的是「这条记录**是我们生成的**吗」（来源），而裸前缀判的是形状 ——
/// 两者不等价，而有一条真实可达的路径能让用户的 provider 撞上前缀：**live config
/// 导入**（`services/provider/live.rs` 的三个 `import_*_providers_from_live`）
/// 的 id 就是用户 CLI 配置文件里的 key，且那三处**绕过命令层的
/// [`reject_if_managed`]**、启动时无条件跑。误判的后果对他是死局：那条 provider
/// 在列表里被滤掉、编辑与删除被拦、托盘里凭空消失，而运营商区也不显示它
/// ⇒ 不可见也不可删，UI 上无逃生路径。
///
/// ⚠️ 别把入口记成「表单里手填」—— `add_provider` 那条路**早就有**
/// [`reject_if_managed`]，填 `loongport-mine` 会当场被拒、建不出记录。
/// 详见 `user_authored_ids_that_merely_start_with_the_prefix_are_not_managed` 的文档。
///
/// ## 为什么是「收紧判据」而不是「给 ProviderMeta 加字段」
///
/// 加字段要动上游结构（扩大 merge 接触面），且**已有记录没有那个字段** ——
/// 判据当场对全部存量失效，那是迁移不是重构。而收紧形状对存量是**无损的**：
/// 两个生成端产出的 id 本来就满足新判据（`{:.16x}` 恒为 16 位小写 hex），
/// 所以已装机数据一条都不用动。
///
/// 代价是这个函数现在依赖两处生成端的**格式**，而不只是前缀常量 ——
/// 那份依赖由 `both_generators_produce_ids_the_guard_recognizes` 钉住：
/// 任一生成端改了长度或字符集，那条测试会红。
pub fn is_managed(provider_id: &str) -> bool {
    let Some(rest) = provider_id.strip_prefix(MANAGED_ID_PREFIX) else {
        return false;
    };
    // vendor 那支多一段；剥掉之后两支的尾部形状相同。
    let hex = rest.strip_prefix(VENDOR_SEGMENT).unwrap_or(rest);
    // 大小写敏感是有意的：`{:x}` 恒产出小写，放行大写会把判据重新放宽到
    // 用户填得出的形状上（`loongport-ABCDEF0123456789` 并非我们生成的）。
    hex.len() == HEX_LEN && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 命令层守卫：撞到托管 id 就拦下。
///
/// **明确报错而不是静默跳过**：静默拒绝会让用户以为切成功了，那正是这轮要修的症状本身。
pub fn reject_if_managed(provider_id: &str) -> Result<(), crate::error::AppError> {
    if is_managed(provider_id) {
        return Err(crate::error::AppError::Message(
            MANAGED_GUARD_MESSAGE.to_string(),
        ));
    }
    Ok(())
}

/// 从（已按调用方规则排好序的）provider 列表里剔除托管项，保留原有顺序。
///
/// 供「点一下就直接切」的入口用（当前是托盘菜单）：那些入口没有 `operator_switch_tier`
/// 的编排，托管项出现在那里就是个陷阱。
pub fn filter_unmanaged<'a>(
    providers: Vec<(&'a String, &'a Provider)>,
) -> Vec<(&'a String, &'a Provider)> {
    providers
        .into_iter()
        .filter(|(_, provider)| !is_managed(&provider.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator::provision;

    fn provider_with_id(id: &str) -> Provider {
        Provider::with_id(id.to_string(), "t".to_string(), serde_json::json!({}), None)
    }

    /// 前缀在**前后端各存一份**（Rust 这里 + `src/config/constants.ts` 的
    /// `MANAGED_PROVIDER_ID_PREFIX`），跨语言编译器管不到 —— 两处不一致的后果是
    /// **后端拦得住、前端滤不掉**（或反之）：托管档位会同时出现在运营商区与
    /// provider 列表里，而那些编辑/删除按钮点下去才报错。
    ///
    /// 这条测试读那个 TS 文件做字面比对。**同类隐患的通用解法**：凡「同一事实散在
    /// Rust 与非 Rust 文件」，就加一条 `include_str!` 比对的测试 ——
    /// 那是唯一能让不一致从「静默失效」变成「测试红」的手段。
    #[test]
    fn prefix_matches_the_frontend_copy() {
        let ts = include_str!("../../../src/config/constants.ts");
        let expected = format!(r#"MANAGED_PROVIDER_ID_PREFIX = "{MANAGED_ID_PREFIX}""#);
        assert!(
            ts.contains(&expected),
            "src/config/constants.ts 的 MANAGED_PROVIDER_ID_PREFIX 与 Rust 侧的 \
             MANAGED_ID_PREFIX ({MANAGED_ID_PREFIX}) 不一致 —— \
             会导致后端拦得住而前端滤不掉（或反之）"
        );
    }

    /// ⭐ **判据的三个可漂移值必须与前端那份一致**（不只是前缀）。
    ///
    /// ## 为什么前缀那条闸不够了
    ///
    /// 收紧之前，跨语言只有**一个**共享事实：前缀字符串（由上面那条闸守着）。
    /// 收紧之后变成**三个**：前缀、[`HEX_LEN`]、[`VENDOR_SEGMENT`]（加上「小写敏感」
    /// 这条约定）。闸只守其中一个，剩下两个漂移时不报错。
    ///
    /// ## 漂移的症状是「换一种死局」，不是少拦一次
    ///
    /// - 只收紧 Rust ⇒ 前端仍按旧形状滤，用户的 provider 从界面消失却没有守卫解释；
    /// - 只收紧前端 ⇒ 列表里看得见，点编辑却报错、指向一个没有它的区。
    ///
    /// 两种都是「用户什么也没做错，但那条记录处置不了」。
    ///
    /// 做法沿用同文件 [`prefix_matches_the_frontend_copy`] 的形状（`include_str!`
    /// 字面比对）—— 那是本仓对「同一事实散在 Rust 与非 Rust 文件」的既定解法。
    #[test]
    fn hex_shape_matches_the_frontend_copy() {
        let ts = include_str!("../../../src/config/managedProviderId.ts");

        for expected in [
            format!("const HEX_LEN = {HEX_LEN};"),
            format!(r#"const VENDOR_SEGMENT = "{VENDOR_SEGMENT}";"#),
        ] {
            assert!(
                ts.contains(&expected),
                "src/config/managedProviderId.ts 缺少 `{expected}` —— \
                 前后端判据漂移会让用户的 provider 要么从界面消失、\
                 要么看得见却改不了（两种都无逃生路径）"
            );
        }

        // 小写敏感那条约定：TS 那份的字符类必须只含 `0-9a-f`。
        // 写成 `0-9a-fA-F` 会放行大写 ⇒ 判据比 Rust 宽，前端滤掉了后端不拦的记录。
        assert!(
            ts.contains("[0-9a-f]"),
            "TS 那份的 hex 字符类必须是小写敏感的 `[0-9a-f]` —— \
             放行大写会让它比 Rust 侧宽"
        );
    }

    #[test]
    fn managed_detection_matches_generated_ids_only() {
        // 正面：provision 真正生成的 id 必须被认出来 —— 这条把判据钉在生成器上，
        // 而不是钉在一个手写的字面量上。
        let real = provision::provider_id_for("https://bestapi.store", Some(1), 42);
        assert!(is_managed(&real));

        // 反面：用户手工配置的 provider 不能被误判成托管（否则托盘里会凭空少一项、
        // 编辑保存也会被拦）。大小写敏感是有意的：生成的 id 恒为小写。
        for id in ["custom-1", "codex-official", "", "LoongPort-1", "loongport"] {
            assert!(!is_managed(id), "id: {id}");
        }
    }

    /// **用户自己能填出来的 id 不许命中判据。**
    ///
    /// ## 它守的是什么缺陷
    ///
    /// 判据原来只判前缀（**形状**），而名字承诺的是「由 LoongPort 生成」（**来源**）。
    /// 两者不等价，而有一条**真实可达**的路径能让用户的 provider 撞上这个前缀：
    ///
    /// **live config 导入**（`services/provider/live.rs` 的三个
    /// `import_*_providers_from_live`，opencode / openclaw / hermes）——
    /// provider id **就是用户自己 CLI 配置文件里的 key**
    /// （`~/.config/opencode/opencode.json` 等），而那三个函数直接
    /// `state.db.save_provider(...)`，**不过 `reject_if_managed`**，且在启动时
    /// 无条件跑（`lib.rs` 那三处调用）。用户在自己的配置里起个叫 `loongport-mine`
    /// 的 provider，下一次启动它就进库了 —— 我们没资格也没拦他怎么命名那个文件。
    ///
    /// ⚠️ **别把入口写成「表单里手填」**：`add_provider` 那条路
    /// （`commands/provider.rs` 的 `add_provider_internal`）**早就有**
    /// `reject_if_managed`，用户在表单里填 `loongport-mine` 会当场被拒、建不出记录。
    /// 只有绕过命令层的 live import 走得通。（review 抓出：原来这里写的是表单那条，
    /// 而按那个描述这个死局压根不存在 ⇒ 会让人以为收紧判据是没必要的防御性加固。）
    ///
    /// 后果对用户是**死局**：那条 provider 在列表里被滤掉、编辑保存被拦、删除被拦、
    /// 托盘里凭空消失，而运营商区也不显示它（那里还要求 `website_url` 匹配某个站）
    /// ⇒ 一条不可见也不可删的孤儿，UI 上无逃生路径。
    ///
    /// 修法是把判据从「前缀」收紧到「前缀 + 我们真正会生成的那两种形状」。
    ///
    /// ⚠️ **这一层不是全称保护**：用户若把 key 起成恰好 16 位小写 hex
    /// （`loongport-0123456789abcdef`），live import 照样写进库、判据照样认它托管。
    /// 所以 live import 那三处也各加了一道跳过（见 `live.rs`）—— 判据收紧管
    /// 「像不像我们生成的」，那道守卫管「到底是不是从我们这儿来的」。
    #[test]
    fn user_authored_ids_that_merely_start_with_the_prefix_are_not_managed() {
        for id in [
            // 用户在 opencode / openclaw / hermes 表单里填得出来的
            "loongport-mine",
            "loongport-my-provider",
            // deeplink 的 `{name}-{timestamp}` 形状：名字填 LoongPort
            "loongport-1785818820765",
            // 长度对不上（我们恒取 16 位）
            "loongport-abc",
            "loongport-0123456789abcdef0",
            // 字符集对不上：hex 里没有 g-z
            "loongport-0123456789abcdefg",
            // 只有前缀，后面空着
            "loongport-",
            // vendor 那支的形状也要照判：前缀对、hex 段不对
            "loongport-vendor-nothex0123456",
        ] {
            assert!(
                !is_managed(id),
                "用户能造出来的 id 被误判成托管 ⇒ 他那条 provider 会改不了也删不掉：{id}"
            );
        }
    }

    /// 两个生成端的输出都必须被认出来。
    ///
    /// **判据钉在生成器上而不是手写字面量上** —— vendor 那支多一段 `vendor-`
    /// （`vendor::provision::provider_id_for`），收紧判据时最容易漏的就是它，
    /// 而漏掉的后果是官网直连那些行的守卫全线失效（能从托盘直接切、能被删）。
    #[test]
    fn both_generators_produce_ids_the_guard_recognizes() {
        let operator_id = provision::provider_id_for("https://bestapi.store", Some(1), 42);
        assert!(is_managed(&operator_id), "operator: {operator_id}");

        // 未登录那支走 "anon" 命名空间，形状必须一样。
        let anon = provision::provider_id_for("https://bestapi.store", None, 42);
        assert!(is_managed(&anon), "operator/anon: {anon}");

        let vendor_id = crate::vendor::provision::provider_id_for("deepseek", "acct-1");
        assert!(is_managed(&vendor_id), "vendor: {vendor_id}");
    }

    #[test]
    fn guard_rejects_managed_ids_with_actionable_message() {
        let real = provision::provider_id_for("https://bestapi.store", Some(1), 7);
        let err = reject_if_managed(&real).expect_err("托管 id 必须被拦下");
        // 文案要指路，不能只说「不允许」。
        assert!(err.to_string().contains("LoongPort"), "err: {err}");

        reject_if_managed("custom-1").expect("普通 id 不该被拦");
    }

    /// 故障转移队列的准入：托管档位不得进队列。
    ///
    /// 这条守的是一条**自动发生**的路径：队列里有托管项时，熔断会让
    /// `FailoverSwitchManager` 在用户没点任何按钮的情况下切到它，跳过
    /// 「退出 ChatGPT → 切换 → 重开」的编排（托盘菜单过滤在这条路上完全无效，
    /// 因为切换不是从菜单点出来的）。
    ///
    /// 队列有**两个准入口**，都必须拦（commands/failover.rs）：
    /// `add_to_failover_queue` 命令（用户手动加），以及 `set_auto_failover_enabled`
    /// 里「队列为空时自动把当前 provider 作为 P1 加入」那段 —— 后者直接调
    /// `state.db`，绕过前者的守卫，所以是独立的一道。
    #[test]
    fn managed_tiers_are_rejected_from_failover_queue() {
        let real = provision::provider_id_for("https://bestapi.store", Some(1), 3);

        // 准入口 1：手动加入 —— 走 reject_if_managed。
        assert!(
            reject_if_managed(&real).is_err(),
            "托管档位必须被挡在故障转移队列之外"
        );
        // 准入口 2：自动作为 P1 加入 —— 走 is_managed 判断后给专门的文案。
        assert!(is_managed(&real));

        // 普通 provider 不受影响：故障转移对它们是正常功能，别顺手拦死。
        for id in ["custom-1", "codex-official"] {
            assert!(reject_if_managed(id).is_ok(), "id: {id}");
            assert!(!is_managed(id), "id: {id}");
        }
    }

    #[test]
    fn filter_unmanaged_drops_managed_and_keeps_order() {
        let managed = provider_with_id(&provision::provider_id_for(
            "https://bestapi.store",
            Some(1),
            1,
        ));
        let first = provider_with_id("custom-1");
        let second = provider_with_id("custom-2");
        let input = vec![
            (&first.id, &first),
            (&managed.id, &managed),
            (&second.id, &second),
        ];

        let kept = filter_unmanaged(input);

        assert_eq!(
            kept.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["custom-1", "custom-2"],
            "托管项必须消失，其余顺序不变"
        );
    }
}
