//! sub2api 的 `platform` ↔ cc-switch 的 [`AppType`] 映射（纯数据 + 纯函数，零 IO）。
//!
//! **这是一张表而不是散落的 `if`**（spec §一）：加平台时只改 [`PLATFORM_MAP`] 一行，
//! 不必去翻每个消费点。两侧命名不同名（`openai` ≠ `codex`、`anthropic` ≠ `claude`），
//! 所以映射必须显式写出来，不能靠字符串相等碰运气。
//!
//! ## 为什么要有 [`Platform`] 这个 enum 而不是直接吃 `&str`
//!
//! 因为 `&str` 没有**基数**。表若以字符串为键，服务端加第 7 个 platform 时不会有任何东西
//! 报错 —— 那张自称「数据」的表会退化成一个无人看守的 `match`，新平台被静默当成未知丢掉。
//! 有了 enum，`PLATFORM_MAP.len() == Platform::all().count()` 这条断言就成了强制闸：
//! 加变体的人必须同时给表加一行，否则测试红。

use crate::app_config::AppType;

/// sub2api 侧的 platform 标识。取值域来自服务端分组数据（6 个），不是我们自定义的。
///
/// **`Composite` 必须是一个变体，而不是解析成「未知」**：`GET /api/v1/groups/available`
/// 服务端不按 platform 过滤，拉回的列表里真的会出现 composite 组，我们要**显式跳过**它，
/// 而不是靠「碰巧没遇到」。判为未知就分不清「该跳过的已知平台」与「上游新加的平台」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    OpenAI,
    Anthropic,
    Gemini,
    Grok,
    Antigravity,
    Composite,
}

/// 一个 platform 在本客户端里的呈现方式。
///
/// 三个变体**不能压成 `Option<AppType>`**：`Unmapped` 与 `NotPresented` 的语义不同 ——
/// 前者是「我们还没实现对应的 CLI」（将来支持 antigravity 时会变成 `Mapped`），后者是
/// 「有意不做」（composite 一把 Key 跨多平台，与「一分组一 provider」的展开模型不对齐）。
/// UI 上两者合并成一个 `unpresentable_count`（spec §三），但**数据层要分开** —— UI 合并
/// ≠ 数据合并，压成同一个状态之后就再也分不回来了。
///
/// 不派生 `Copy`：上游的 [`AppType`] 没派生 `Copy`，而给它补一个 derive 会在上游文件上留
/// 改动、扩大将来 merge 的接触面。这里 clone 一个无字段枚举是零成本的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformMapping {
    /// 映射到某个 cc-switch app_type，正常展开成一条 provider。
    Mapped(AppType),
    /// 认得这个 platform，但 cc-switch 侧没有对应的 app（→ 计入「本客户端暂不呈现」）。
    Unmapped,
    /// 有意不呈现：composite 一把 Key 跨多平台，与「一分组一 provider」不对齐。
    NotPresented,
}

/// spec §一 那张映射表的**唯一**代码形态。改它之前先回查 spec §一。
///
/// 基数由 `map_len_equals_platform_cardinality` 守：加 [`Platform`] 变体不加这里的行 ⇒ 测试红。
const PLATFORM_MAP: &[(Platform, PlatformMapping)] = &[
    (Platform::OpenAI, PlatformMapping::Mapped(AppType::Codex)),
    (
        Platform::Anthropic,
        PlatformMapping::Mapped(AppType::Claude),
    ),
    (Platform::Gemini, PlatformMapping::Mapped(AppType::Gemini)),
    (Platform::Grok, PlatformMapping::Mapped(AppType::GrokBuild)),
    // cc-switch 侧没有 antigravity 对应的 app。将来接了就把这行改成 `Mapped(...)`，
    // 这正是「加平台只改这张表」的兑现方式。
    (Platform::Antigravity, PlatformMapping::Unmapped),
    (Platform::Composite, PlatformMapping::NotPresented),
];

/// 服务端字符串 → enum。未知取值返回 `None`，由调用方计入「本客户端不呈现」。
///
/// **不做大小写与空白的宽容处理**：服务端给的是固定小写标识，宽容只会掩盖协议变化 ——
/// 上游哪天把 `openai` 改成 `OpenAI`，我们要在这里当场认不出来，而不是模糊匹配过去。
pub fn parse_platform(s: &str) -> Option<Platform> {
    match s {
        "openai" => Some(Platform::OpenAI),
        "anthropic" => Some(Platform::Anthropic),
        "gemini" => Some(Platform::Gemini),
        "grok" => Some(Platform::Grok),
        "antigravity" => Some(Platform::Antigravity),
        "composite" => Some(Platform::Composite),
        _ => None,
    }
}

/// 查表得出呈现方式。**表驱动而不是 `match`** —— 保证映射数据只有一处。
pub fn map_platform(platform: Platform) -> PlatformMapping {
    PLATFORM_MAP
        .iter()
        .find(|(candidate, _)| *candidate == platform)
        .map(|(_, mapping)| mapping.clone())
        // 表覆盖全部变体，由基数闸守。真漏一格时按「不呈现」处理（fail-closed：
        // 宁可不展开，也不能误绑到某个 CLI 上去）。
        .unwrap_or(PlatformMapping::NotPresented)
}

impl Platform {
    /// 便捷取法：只有 `Mapped` 才给出 app_type。
    pub fn app_type(self) -> Option<AppType> {
        match map_platform(self) {
            PlatformMapping::Mapped(app_type) => Some(app_type),
            PlatformMapping::Unmapped | PlatformMapping::NotPresented => None,
        }
    }

    /// 遍历全部 platform。
    ///
    /// **只有 `#[cfg(test)]` 调用方**（基数闸那几条断言），所以 lib target 会报 `dead_code`。
    /// **不删**：删了那条基数断言就只能写成手写数字 `6`，而手写的数字不会在加第七个
    /// platform 时报错 —— 那等于把这张表的唯一守卫拆掉。
    #[allow(dead_code)]
    pub fn all() -> impl Iterator<Item = Platform> {
        [
            Platform::OpenAI,
            Platform::Anthropic,
            Platform::Gemini,
            Platform::Grok,
            Platform::Antigravity,
            Platform::Composite,
        ]
        .into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// spec §六 第 1 条：**基数闸**。这是这张表唯一真正的守卫 —— 服务端加第 7 个 platform、
    /// 有人给 enum 加了变体却忘了加表行时，就靠这条报红。
    #[test]
    fn map_len_equals_platform_cardinality() {
        assert_eq!(
            PLATFORM_MAP.len(),
            Platform::all().count(),
            "映射表与 Platform 枚举的基数必须一致：加平台时两处都要改"
        );
        // 每个变体有且只有一条记录：光比长度挡不住「复制粘贴写重了一行、另一行漏了」
        // ——那时长度仍然相等，而被重复的那格会静默遮蔽 `find` 的后一条。
        for platform in Platform::all() {
            let hits = PLATFORM_MAP
                .iter()
                .filter(|(candidate, _)| *candidate == platform)
                .count();
            assert_eq!(hits, 1, "{platform:?} 在映射表里出现 {hits} 次");
        }
    }

    /// spec §六 第 2 条：`NotPresented` 与 `Unmapped` **必须可区分**。
    #[test]
    fn composite_is_not_presented_and_distinct_from_unmapped() {
        assert_eq!(
            map_platform(Platform::Composite),
            PlatformMapping::NotPresented
        );
        assert_eq!(
            map_platform(Platform::Antigravity),
            PlatformMapping::Unmapped
        );
        assert_ne!(
            map_platform(Platform::Composite),
            map_platform(Platform::Antigravity),
            "composite 是「有意不做」、antigravity 是「还没实现」，语义不同不许压成同一个状态"
        );
        // 但两者对「能不能拿到 app_type」的回答一致 —— UI 合并计数正是建立在这上面。
        assert_eq!(Platform::Composite.app_type(), None);
        assert_eq!(Platform::Antigravity.app_type(), None);
    }

    /// 四条映射逐格钉住（spec §一 的表）。
    #[test]
    fn mapped_platforms_point_at_the_spec_app_types() {
        assert_eq!(Platform::OpenAI.app_type(), Some(AppType::Codex));
        assert_eq!(Platform::Anthropic.app_type(), Some(AppType::Claude));
        assert_eq!(Platform::Gemini.app_type(), Some(AppType::Gemini));
        assert_eq!(Platform::Grok.app_type(), Some(AppType::GrokBuild));
    }

    #[test]
    fn parse_rejects_unknown_and_does_not_normalize() {
        assert_eq!(parse_platform("openai"), Some(Platform::OpenAI));
        // 上游新加平台走这条：不 panic 也不猜。
        assert_eq!(parse_platform("bedrock"), None);
        // 宽容匹配会掩盖协议变化，所以下面三条都必须是 None。
        assert_eq!(parse_platform("OpenAI"), None);
        assert_eq!(parse_platform(" openai"), None);
        assert_eq!(parse_platform(""), None);
    }

    /// 映射目标不得是 additive 模式的 app（`opencode` / `openclaw` / `hermes`）——
    /// 那几个的 live 配置写入是「全部 provider 都写」，与托管档位「当前那条才生效」的
    /// 切换语义不兼容，误绑上去会让所有档位同时生效。
    #[test]
    fn mapped_targets_are_switch_mode_apps() {
        for platform in Platform::all() {
            if let Some(app_type) = platform.app_type() {
                assert!(
                    !app_type.is_additive_mode(),
                    "{platform:?} 映射到了 additive 模式的 {}",
                    app_type.as_str()
                );
            }
        }
    }
}
