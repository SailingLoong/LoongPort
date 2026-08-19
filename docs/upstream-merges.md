# 上游合并台账（冲突面监控）

**为什么有这份台账**：LoongPort 是 cc-switch 的 fork，长期可持续性取决于
「每次吸收上游的成本是否仍可承受」。冲突面只会随自有功能增长而变宽——
它是**要监控的指标**，不是背景噪音。CLAUDE.md §一/§三 定了怎么合并，
这里记**每次合并花了多少**，以及什么时候该动结构。

## 记录规则

每次同步上游（整并或定点 cherry-pick）后追加一行：

| 日期 | PR | 形态 | 上游提交数 | 冲突文件数 | 依赖栈适配 | 备注 |
|---|---|---|---|---|---|---|

- **冲突文件数**：`git merge` 实际停下要手工解的文件数（不是 diff 大小——
  大小随上游波动，冲突数才反映接缝宽度）。
- **依赖栈适配**：上游代码用到本仓依赖版本不支持的 API 等类适配
  （例：`{:x}` 格式化在钉版 sha2 上不实现 `LowerHex`）。
- **结构性动作阈值**：冲突文件数**连续三次同步上升** ⇒ 停下来做结构收敛
  （把该次冲突最集中的接缝从上游文件里再往外抽一层），而不是继续硬解。
  判断与做法归 CLAUDE.md §一「改上游文件时改动面越小越好」管辖。

## 台账

| 日期 | PR | 形态 | 上游提交数 | 冲突文件数 | 依赖栈适配 | 备注 |
|---|---|---|---|---|---|---|
| 2026-08-14 | #116 | 整并 upstream main | 67 | （未单列，主要在 Cargo.lock） | 有（依赖大版本栈） | merge-tree 干跑定冲突面；Cargo.lock `--theirs` + `cargo check` 收口；WSL2 job flake 重跑即绿 |
| 2026-08-16 | #145 | 定点 cherry-pick | 2 | 2（database/mod.rs、schema.rs） | 1（sha2 `LowerHex`→`hex::encode`） | SCHEMA_VERSION 16→17 跟上游走，口径注释保留；首次建立本台账 |
| 2026-08-19 | #200 | 定点 cherry-pick（预收冲突） | 1（3d126f45） | 1（UsageTrendChart.tsx，取上游版整体替换本地 3c43cfca） | 无 | 上游 #6337 与本地 #144 同根修复的会合：主动吸收上游版使文件回到与上游一致，下次整并该文件不再冲突；上游 PR #6488 已被取代关闭 |
| 2026-08-19 | #202 | 整并 upstream tag v3.20.0 | 30 | 49（11 纯上游文档保删、4 版本号保 6.2.0、4 语言保我方、30 接缝逐解） | 2（sha2 `LowerHex`、toml 1.0 `Value::from_str` 不收文档） | 接缝集中在 provider 服务/选路/表单三簇；**修根一处**：`has_explicit_codex_third_party_upstream` 的 TOML 解析自 toml 1.0 bump 起静默失效（上游测试照出），改 `toml::from_str::<Table>`；**语义合流一处**：preserve_codex_official_auth_on_switch 遇托管账号 live auth（marker 在）时不再保留 auth.json，交由上游托管事务替换+清理；聚合页形态保留（上游 AddProviderForm→Dialog 改名未采纳，AuthSettingsPanel 移植进页面） |

## 关联

- 合并纪律与验收三问：`CLAUDE.md` §一、§三
- 回传路线（减少 diff 面积的另一半）：design 档案仓「可回传上游的修复盘点」
