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
//! 有了 enum，[`map_platform`] 的**穷尽 `match`** 就成了强制闸：加变体 ⇒ **编译失败**。
//!
//! ⚠️ **这道闸曾经是假的，别改回去**（2026-08-02 三路对抗性审查抓出，两路各自编译验证）：
//! 原先它是 `const PLATFORM_MAP: &[(Platform, PlatformMapping)]` 数组 + `.find()` +
//! `unwrap_or(NotPresented)`，闸写成断言 `PLATFORM_MAP.len() == Platform::all().count()`。
//! 那条断言**恒真** —— `all()` 是手写数组，加变体时它不会自己变长，两边同时停在 6。
//! 实测：加一个变体 + 给 `parse_platform` 加一行，不动表也不动 `all()`，**测试全绿**，
//! 运行期靠 `unwrap_or` 把新平台静默当「有意不呈现」丢掉，正是这张表声称要防的那件事。
//! （V1 那份也是手写数组，同样挡不住；V1 真正的闸是 `as_str()` 里的穷尽 `match`，
//! 本轮移植时把它丢了，闸随之失效。）
//!
//! 所以现在**映射数据就写在那个 `match` 里**：它仍是「唯一一处映射数据」（模块文档开头那句
//! 说的是不许散落到各消费点，不是不许用 `match`），但多了编译器强制。没有 fail-closed
//! 兜底分支 —— 不需要，漏一格根本编译不过。

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
/// **数据层要把两者分开**：将来真要在 UI 上告诉用户「另有 N 个分组不呈现」时，
/// 「还没实现」与「有意不做」对他是两种含义（前者会变、后者不会）。压成同一个状态
/// 之后就再也分不回来了。
///
/// ⚠️ 曾经有个 `unpresentable_count` 字段把两者合并上报，但它**恒为 0**（数真值要发网络
/// 请求，与 `operator_list_operators`「只读本地」的契约冲突）⇒ 2026-08-04 连 UI 分支
/// 一起删了。这里的区分与那个字段无关，是数据模型自己的事。
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

/// 服务端字符串 → enum。未知取值返回 `None`，由调用方计入「本客户端不呈现」。
///
/// **不做大小写与空白的宽容处理**：服务端给的是固定小写标识，宽容只会掩盖协议变化 ——
/// 上游哪天把 `openai` 改成 `OpenAI`，我们要在这里当场认不出来，而不是模糊匹配过去。
///
/// 注意这里的 `_ => None` 是**对服务端字符串**兜底（上游新加平台走这条），
/// 与 [`map_platform`] 那个不许有兜底分支的 `match` 不是一回事。
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

/// spec §一 那张映射表的**唯一**代码形态。改它之前先回查 spec §一。
///
/// ## 为什么是 `match` 而不是 `&[(Platform, PlatformMapping)]` 数组
///
/// 因为**只有穷尽 `match` 能让编译器当闸**：加一个 [`Platform`] 变体不在这里加分支 ⇒
/// `error[E0004]: non-exhaustive patterns` ⇒ 当场编译失败，不可能漏。
///
/// 数组版做不到 —— 它得靠 `.find()` 查表，查不到就落到某个兜底分支，于是漏一格是
/// **运行期静默行为**而不是编译错误。原先那版就是数组 + 断言
/// `PLATFORM_MAP.len() == Platform::all().count()`，而那条断言恒真（`all()` 是手写数组，
/// 加变体时不会自己变长）。详见模块文档开头那段。
///
/// **不许加 `_ =>` 兜底分支** —— 加了就等于把这道闸拆掉，退回原来那个假闸的状态。
pub fn map_platform(platform: Platform) -> PlatformMapping {
    match platform {
        Platform::OpenAI => PlatformMapping::Mapped(AppType::Codex),
        Platform::Anthropic => PlatformMapping::Mapped(AppType::Claude),
        Platform::Gemini => PlatformMapping::Mapped(AppType::Gemini),
        Platform::Grok => PlatformMapping::Mapped(AppType::GrokBuild),
        // cc-switch 侧没有 antigravity 对应的 app。将来接了就把这行改成 `Mapped(...)`，
        // 这正是「加平台只改这一处」的兑现方式。
        Platform::Antigravity => PlatformMapping::Unmapped,
        Platform::Composite => PlatformMapping::NotPresented,
    }
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
    /// **只有 `#[cfg(test)]` 调用方**，所以 lib target 会报 `dead_code`。
    ///
    /// ⚠️ **这个数组不是闸，别再让它当闸**：它是手写的，加 [`Platform`] 变体时不会自己变长
    /// （原先 `PLATFORM_MAP.len() == all().count()` 就是因此恒真）。真正的闸是
    /// [`map_platform`] 的穷尽 `match`。这里留着只为让测试能遍历全部变体做**逐格断言**
    /// （每格映射到 spec §一 指定的 app_type、映射目标不得是 additive 模式），
    /// 漏加一项最多让某格漏测，不会让错误的映射蒙混过关。
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

    /// spec §六 第 1 条的**替代物**：那条要求的「基数闸」现在由编译器执行
    /// （[`map_platform`] 的穷尽 `match`：加变体不加分支 ⇒ `error[E0004]`），
    /// 不再需要、也**不可能**用运行期断言表达 —— 这正是原来那条断言恒真的原因。
    ///
    /// 这条测试改为守另一件事：`all()` 与 `parse_platform` 的**往返一致**。
    /// `all()` 是手写数组，漏加一项会让下面那些逐格断言少测一格；让它与
    /// `parse_platform` 对账，漏加的那项就会在这里露出来（因为加变体的人几乎一定会
    /// 记得改 `parse_platform` —— 不改的话服务端那个平台压根解析不出来）。
    #[test]
    fn all_variants_round_trip_through_parse() {
        for platform in Platform::all() {
            let raw = match platform {
                Platform::OpenAI => "openai",
                Platform::Anthropic => "anthropic",
                Platform::Gemini => "gemini",
                Platform::Grok => "grok",
                Platform::Antigravity => "antigravity",
                Platform::Composite => "composite",
            };
            assert_eq!(
                parse_platform(raw),
                Some(platform),
                "{platform:?} 的服务端标识 {raw:?} 解析不回来"
            );
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
