//! `loongport-cli`：无桌面环境（服务器、老发行版）的一次性配置入口。
//!
//! 与 GUI 二进制共享同一份库（`cc_switch_lib`），但用 `--no-default-features`
//! 构建——不带 tauri/GTK/webkit，静态链接（musl）后可在任何 x86_64 Linux 上
//! 跑，包括 GUI 版无法安装的 Ubuntu 20.04（webkit2gtk-4.1 缺失 + glibc 符号墙）。
//! 逻辑全部在 [`cc_switch_lib::cli`]，这里只做进程入口。

fn main() {
    std::process::exit(cc_switch_lib::cli::run_add_site());
}
