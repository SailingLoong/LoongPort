//! 「支持范围」那张表的唯一真相源，外加一道钉住各处副本的闸。
//!
//! # 为什么需要它
//!
//! 「已支持什么 / 在做什么」这个事实同时出现在**四处**，而它们跨仓库、跨语言：
//!
//! | 处 | 位置 | 谁看 |
//! |---|---|---|
//! | 1 | `README.md` 的「支持范围」表 | GitHub 访客 |
//! | 2 | `README_EN.md` 的 What it supports 表 | 同上（英文） |
//! | 3 | `LOONGPORT.md` 的「当前进度」表 | 要改这个仓的人 |
//! | 4 | 官网仓 `src/lib/site.ts` 的 `SUPPORT_MATRIX` | loongport.dev 访客 |
//!
//! 这正是 CLAUDE.md §三点六 说的那类：**编译器管不到 md 与另一个仓的 ts，
//! 不一致时不崩不报，只是某处静默过期**。而这张表过期的后果不只是「文档旧了」——
//! 它是对外承诺的一部分：new-api 那边如果看到我们没列他们，可能就自己另搞一套，
//! 而我们其实早就把登录标识设计成中立的 `login_identifier` 正是为了接他们。
//!
//! # 这道闸覆盖到哪，覆盖不到哪
//!
//! **覆盖**：本仓的三处 md（1、2、3）—— `include_str!` 编译期读进来比对。
//!
//! **覆盖不到**：官网那处（4）。它在**另一个 git 仓**里，`include_str!("../../../
//! LoongPort-website/...")` 在别人 clone 本仓时会**编译失败** —— 那不是把闸做严，
//! 是把仓做坏。官网那处靠 `site.ts` 里那句注释交叉引用（写明「与 LOONGPORT.md
//! 那张表同一份事实」），以及改这里时的自觉。
//!
//! ⇒ 结论：**改这个常量时，官网那处要手工同步**。这是已知的缺口，写在这里而不是
//! 假装不存在。
//!
//! # ⚠️ 别把闸写成恒真断言
//!
//! `platform_map.rs` 记着一个真实教训：那里原本断言
//! `PLATFORM_MAP.len() == Platform::all().count()`，而两边都是手写数组 ⇒
//! **加一个变体时两边同时不变，断言恒真、闸形同虚设**。
//!
//! 所以这里的断言方向是「**常量里的每一项都必须在文档里出现**」——
//! 常量是唯一真相源，文档是副本。加一项时若忘了改文档，测试当场红。
//! （反方向不做检查：文档里多写一个词不算错，那可能只是叙述。）

/// 中转服务：已支持的。
const OPERATORS_SHIPPED: &[&str] = &["sub2api"];

/// 中转服务：在做的。
///
/// ⚠️ **`new-api` 在这里是一句对外承诺**，不是随手写的占位 —— 登录标识已按它设计成
/// 中立的 `login_identifier`（而不是 sub2api 那侧的具体字段名）。删它之前先想清楚
/// 那个设计还要不要。
const OPERATORS_PLANNED: &[&str] = &["new-api"];

/// AI CLI：已支持的（**指对话档位**）。
const CLIS_SHIPPED: &[&str] = &["codex", "claude"];

/// AI CLI：在做的。
///
/// ⚠️ `gemini` 的状态是**分裂的**，别简化成一个词：`platform_map` 里它已经是
/// `Mapped(AppType::Gemini)`、生图 MCP 也装进了 gemini CLI，但**对话档位**还不能
/// 落到它（缺配置写入形状）。所以它列在 planned，而两份 README 各有一句限定说明
/// 那行讲的是对话档位。
const CLIS_PLANNED: &[&str] = &["gemini", "grok"];

/// 平台：已支持的。
const PLATFORMS_SHIPPED: &[&str] = &["macOS", "Windows"];

/// 平台：在做的。
const PLATFORMS_PLANNED: &[&str] = &["Linux"];

#[cfg(test)]
mod tests {
    use super::*;

    const README_ZH: &str = include_str!("../../README.md");
    const README_EN: &str = include_str!("../../README_EN.md");
    const LOONGPORT_MD: &str = include_str!("../../LOONGPORT.md");

    /// 三份文档里都必须出现常量里的每一项。
    ///
    /// 判据方向是「常量 → 文档」：常量是真相源，加一项时忘了改文档就当场红。
    /// 反方向不查 —— 文档里多出现某个词可能只是叙述（例如正文里提到 `gemini`），
    /// 那不构成错误。
    fn assert_all_present(doc: &str, doc_name: &str) {
        for (label, items) in [
            ("已支持的中转服务", OPERATORS_SHIPPED),
            ("在做的中转服务", OPERATORS_PLANNED),
            ("已支持的 CLI", CLIS_SHIPPED),
            ("在做的 CLI", CLIS_PLANNED),
            ("已支持的平台", PLATFORMS_SHIPPED),
            ("在做的平台", PLATFORMS_PLANNED),
        ] {
            for item in items {
                assert!(
                    doc.contains(item),
                    "{doc_name} 里找不到「{item}」（{label}）—— \
                     `support_matrix.rs` 改了但那份文档没跟上。\
                     记得官网仓 `src/lib/site.ts` 的 SUPPORT_MATRIX 也要手工同步。"
                );
            }
        }
    }

    #[test]
    fn readme_zh_lists_every_entry() {
        assert_all_present(README_ZH, "README.md");
    }

    #[test]
    fn readme_en_lists_every_entry() {
        assert_all_present(README_EN, "README_EN.md");
    }

    #[test]
    fn loongport_md_lists_every_entry() {
        assert_all_present(LOONGPORT_MD, "LOONGPORT.md");
    }

    /// ⭐ **这条盯的是「闸本身还有效吗」。**
    ///
    /// `platform_map.rs` 那个教训：断言看着在、实际恒真。这里的风险是常量被清空 ——
    /// 空数组上的 `for` 循环一次都不执行，上面三条测试会**全绿**。
    ///
    /// 所以显式断言每一维都非空。加维度时这条也要跟着加，那是有意的：
    /// 一道闸的覆盖范围应该是显式声明的，不该靠遍历某个可能为空的集合。
    #[test]
    fn the_gate_itself_is_not_vacuous() {
        for (name, items) in [
            ("OPERATORS_SHIPPED", OPERATORS_SHIPPED),
            ("OPERATORS_PLANNED", OPERATORS_PLANNED),
            ("CLIS_SHIPPED", CLIS_SHIPPED),
            ("CLIS_PLANNED", CLIS_PLANNED),
            ("PLATFORMS_SHIPPED", PLATFORMS_SHIPPED),
            ("PLATFORMS_PLANNED", PLATFORMS_PLANNED),
        ] {
            assert!(
                !items.is_empty(),
                "{name} 是空的 —— 空数组会让上面那三条测试的 for 循环一次都不跑、\
                 于是闸恒绿。要清空某一维，先把对应的断言一起删掉，别留一个假闸。"
            );
        }
    }
}
