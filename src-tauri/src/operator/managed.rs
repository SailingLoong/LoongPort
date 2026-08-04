//! 「这条 provider 是不是 LoongPort 托管的」——**唯一判据来源**。
//!
//! ## 为什么要单独一个文件
//!
//! 判据是 id 前缀（[`provision::provider_id_for`](super::provision::provider_id_for) 生成），
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
//! 旧分组**，而用户不会收到任何提示。前端 `startsWith` 过滤挡不住托盘菜单与命令层，
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

/// 这个 provider id 是不是 LoongPort 托管的。
pub fn is_managed(provider_id: &str) -> bool {
    provider_id.starts_with(MANAGED_ID_PREFIX)
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
