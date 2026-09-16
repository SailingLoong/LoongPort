//! 品牌资产身份闸：钉住「代表产品」的三个图标的字节内容。
//!
//! ## 为什么需要（2026-09-16 事故复盘）
//!
//! 仓里历史上混进过 cc-switch 的 logo 资产：`src/assets/icons/app-icon.png`
//! 与上游同名文件逐字节相同（上游 PR 带入），`tray-32.png` 由它派生，
//! macOS 托盘模板也是同款星芒形状——三者都占了「产品图标」的位置
//! （侧边栏品牌区 / 关于卡 / 托盘），直到用户在主界面左上角认了出来。
//!
//! 上游仓至今仍带同名文件：整并时二进制冲突若顺手取 theirs，cc-switch 的
//! logo 会静默回到这些路径（`app-icon.png` 曾与上游 0 字节差异）。
//! 哈希闸让这种回退当场红。
//!
//! ## 换图标时
//!
//! 设计侧若出新的专用小图（如 Windows 托盘简化版），替换文件后同步更新
//! 这里的期望哈希即可——闸钉的是「内容是谁家的」，不是「内容永远不变」。

use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn product_icon_assets_are_not_upstream_cc_switch_logos() {
    // (路径标注, 文件字节, 期望 sha256 —— 全部从仓内龙标源 icons/icon.png 派生)
    let cases: &[(&str, &[u8], &str)] = &[
        (
            "src/assets/icons/app-icon.png (侧边栏品牌区 + 关于卡)",
            include_bytes!("../../src/assets/icons/app-icon.png"),
            "b8e80081f4147e744e628d6fe291e4af6b8a5fa7769c1b859a0d6f03ef3fb37c",
        ),
        (
            "src-tauri/icons/tray/tray-32.png (Windows 托盘)",
            include_bytes!("../icons/tray/tray-32.png"),
            "5a053f6a2cd875072b21765e943e040bd68dc74839171093a0d14cf5e3128be5",
        ),
        (
            "src-tauri/icons/tray/macos/statusbar_template_3x.png (macOS 托盘模板)",
            include_bytes!("../icons/tray/macos/statusbar_template_3x.png"),
            "3b3d922a4befce63fada5b4c7cd6743d494f2d227a5a266e5576d1abc78a4b9b",
        ),
    ];
    for (label, bytes, expect) in cases {
        let actual = sha256_hex(bytes);
        assert_eq!(&actual, expect, "{label}: 内容与登记的龙标哈希不符——若非有意换图，多半是上游整并把 cc-switch 资产带了回来");
    }
}
