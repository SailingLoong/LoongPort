# Tauri 打包兼容性门禁

## 2026-10-05：CI 全绿，但 beta.1 没有生成安装资产

`v6.26.4-beta.1` 指向 `7a72d08a`。对应的类型检查、前端测试、
Rust 测试、Clippy、格式与 WSL2 契约全部通过，但
[Release 37261445749](https://github.com/SailingLoong/LoongPort/actions/runs/37261445749)
的四个平台在正式 Tauri 构建前的版本检查处失败。发布和 updater manifest
作业跳过，没有 GitHub Release 或可供安装的 beta.1 资产。

根因是 [PR540](https://github.com/SailingLoong/LoongPort/pull/540) 更新了
Cargo 锁文件中的 Tauri Rust 依赖，但冻结的 NPM 对应包仍处于旧 minor：

| Rust crate           | Rust 版本 | 当时的 NPM 包版本                |
| -------------------- | --------- | -------------------------------- |
| tauri                | 2.12.1    | @tauri-apps/api 2.11.1           |
| tauri-plugin-log     | 2.10.0    | @tauri-apps/plugin-log 2.9.2     |
| tauri-plugin-dialog  | 2.8.1     | @tauri-apps/plugin-dialog 2.7.3  |
| tauri-plugin-process | 2.4.0     | @tauri-apps/plugin-process 2.3.1 |

四个平台报相同错误。这是确定的依赖边界问题，不是偶发 runner 故障，
重跑同一 tag 不会修复它。保留 beta.1 tag 作为失败记录；修复后走新的
beta.2 版本 PR 和发布流程。

## 旧 CI 为什么漏检

- Rust 编译器和 Clippy 检查 Rust 依赖；TypeScript 与前端测试检查 NPM
  依赖。两边各自通过不代表跨边界版本兼容。
- 原先只在 `tauri build` 时执行 Tauri 自身的跨生态校验，普通 CI
  没有执行它。
- Cargo-only 更新没有触发前端 job，因此冻结的 JS counterpart 不会
  因 Rust minor 更新而被检查。
- 不能直接把 `tauri info` 的退出码当门禁：固定 CLI 2.11.5 的 info
  会打印兼容错误，但仍返回 0。

## 已落实的预防

`pnpm check:tauri` 使用项目安装的官方 CLI 执行 `tauri info`。固定版本的
[info 命令](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.5/crates/tauri-cli/src/info/mod.rs)
和 [正式 build](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.5/crates/tauri-cli/src/build.rs)
调用同一个
[官方通用插件校验](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.5/crates/tauri-cli/src/info/plugins.rs)。
major/minor 判据和已知插件集合由官方 CLI 维护，本仓不复制版本比较算法。

审查又复现了官方 CLI 吞掉批量 pnpm 查询失败的路径：展示逐包版本仍成功，
实际比较集合却为空。只检查展示与 mismatch 文本会误放行。门禁现在仅在
官方 info 子进程的临时 PATH 中转发 pnpm，将实际批量查询的状态和解析结果
写入临时回执；查询未执行、失败、解析错误或 counterpart 不完整都拒绝。
不另跑预查询，不改变解析或比较算法；检查结束删除临时目录，无全局修改。
当前回执要求已安装的正式三段版本；不支持的版本格式保守拒绝，不能静默放行。

包装器把官方 mismatch 诊断变为失败状态，并检查 package.json 声明的 API
和插件的 Rust/NPM 两边都被官方报告解析。缺失报告、命令失败和超时均失败
关闭，不只关注本次四个包。诊断只读已安装包及锁文件；关闭无关的 latest
网络查询，设 120 秒上限，不编译或启动应用。

- Cargo manifest、Cargo.lock 和门禁脚本变动会触发前端 CI。
- 前端 CI 执行兼容门禁及回归，再执行既有类型、格式和测试检查。
- 每个平台 Release 在读取签名材料和构建前执行同一个门禁。
- 四个 NPM 对应包精确固定到 2.12.1 / 2.10.0 / 2.8.1 / 2.4.0；
  Rust 图和其他 NPM 依赖保持不变。
- 回归覆盖实际四项 mismatch、info 的零退出码、第五个插件、未解析
  counterpart、空报告、命令失败、超时和不影响兼容性的环境警告。

本地复现先记录固定 CLI 返回 0 并打印四项 mismatch；断言兼容的测试
因此失败，新门禁也返回 1。对齐依赖后，要求原测试和门禁转为 GREEN。
升级 CLI 时应重跑这些契约测试，并核对其诊断格式；不能静默跳过门禁。

## 发布 done 标准

1. 依赖修复与版本改动走普通 PR，精确候选的 required CI 全部终态成功。
2. annotated tag 指向已验证提交，正文说明改动和实际验收边界；旧 beta
   tag 不覆盖，不用 `--ignore-version-mismatches` 绕过检查。
3. 正常 Release 的平台构建、发布及 manifest 作业全部成功。
4. 读取真实 Release，确认 pre-release 标志与完整安装资产；实际下载
   验证大小和 digest，用提交中配置的公钥验证 updater 签名。
5. 核对 manifest 的版本、说明、平台和签名；线上 stable 渠道仍返回
   上一正式版，beta 渠道返回新 beta，下载代理实际可读。

单元测试全绿、tag 存在、构建开始、Release 页面出现，均不能单独等同于
发布验收完成。Apple 代码签名/公证与 updater 签名分别报告实际状态。
真实 GUI 和官方账户切换由用户安装 beta 后验证，自动化结果不冒充该验收。
