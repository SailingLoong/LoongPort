# ZCode 账号功能与验证范围

本页说明 `LOONG-ZCODE-ACCOUNTS-V2.0` 与 `V2.0.1 / U1-Q` 的代码范围与验证限制。产品流程见[账号流程 V2](zcode-accounts-v2.md)，账号包格式及来源见[格式说明](zcode-zsb-format.md)。

## 已有功能与兼容边界

已发布 beta.3 提供原生账号捕获、加密保存、手动切换、恢复记录及 `.zsb` 导入。原生写入仅支持已核对完整指纹的 macOS ZCode 3.14.4（build 7912）。未知构建、Windows 和跨环境恢复不能据此视为已支持。

本次代码增加：

- 官方 BigModel / z.ai 登录协议、对应 HTTP 适配、个人项目和已有 Coding Key 查询。
- 针对确切个人项目的显式创建 Key 同意、单次创建保护和原操作查询。创建结果不明时不自动再次创建；明确未发送或已复制原项目 Key 后可精确清理本地记录。
- 业务 token、Start JWT、Coding Key 各自的连接核验与额度数据。网络失败保留同一凭据上次已确认事实及原时间，不把未知显示成零或过期。
- 账号与保存收据在同一加密 catalog 中提交，响应丢失可按原请求查询。取消或到期清除凭据草稿，仍可查询已经发生的保存或本地清理结果。
- 登录账号与 API 配置两个页签；账号库入口不依赖原生客户端准入，原生捕获与切换仍分别检查兼容性。已有账号可在保留身份与其他凭据的前提下补齐 Coding 连接。
- 完整 `.zsb` 会话预览、逐项选择与明确授权核验；按同一选择提交，取消后不接受迟到结果。有效独立能力不因另一个业务 token 失败而被一并拒绝。
- 加密备份选择与口令确认、禁止覆盖既有文件、私有文件权限、持久写入及认证回读；响应不明时查询原请求收据。
- 单账号手动连接及额度检查、显示名编辑、显式读取本机当前身份。检查结果与原生当前账号分别表达；切换、刷新本地列表和页面打开不触发官方查询。

## 接口与数据边界

登录命令包括 `begin_zcode_official_login`、`get_zcode_login_progress`、`confirm_zcode_login_key`、`decline_zcode_login_key`、`save_zcode_login_account`、`cancel_zcode_official_login`。`get_zcode_account_library` 读取账号库，`begin_saved_zcode_coding` 补齐已存账号的 Coding 连接。`check_zcode_account_connections` 与 `cancel_zcode_connection_check` 拥有单条手动检查的请求生命周期；本机捕获和切换继续使用已有独立原生资格检查。

命令返回脱敏身份、阶段、连接状态、公开错误和保存结果，不返回凭据。`accountIdentity` 只表示本地缓存槽，不能代替上游授权证明。完整外部会话不因缺少额外同人证明而一律要求重新登录；不同凭据的能力分别核验，不拼接来自不同账号包的字段。

`export_zcode_account_bundle` 接收结构化 `input`，`get_zcode_bundle_export_result` 只查询原请求 ID；只有已认证保存结果返回目标路径和条数。预览、核验及导入分别使用 `preview_zcode_account_bundle`、`check_zcode_account_bundle`、`get_zcode_bundle_check_progress`、`import_zcode_account_bundle`。

包编码器沿用外层 v1 / 内层 v2、10 MiB / 50 条上限，只导出选择的账号会话；不包含设备 ID、机器密钥、项目历史、其他应用秘密或 LoongPort 内部能力证据。首版只承诺原系统、用户与 home 环境恢复。

## 已执行验证与限制

验证均使用合成凭据、隔离文件根或模拟传输：

- 实际产品核心模块回归 384 项通过、3 项既有忽略；包含 OAuth / Key 不确定性、能力独立性、重复导入、取消及迟到结果、加密导出和文件保护。
- 整仓前端 1691 项通过、1 项预期失败、14 项跳过（232 个测试文件通过、1 个跳过）；包括账号界面、备份结构化输入与 API 页签集成。
- 后续边界修正的受影响回归：OAuth / 已存 Coding 53 项、会话核验 21 项、账号目录 24 项、相关界面 107 项通过。覆盖异常日期、明确拒绝后网络失败、Key 创建前目录变化，以及只改标签后的原请求恢复；上述整合全套未为这些窄修正重复运行。
- TypeScript、整仓 Prettier 与 Rust 格式检查通过。
- 实际运行态及命令函数体已在私有 no-GUI 编译图中检查；保留真实 Database、Vault、文件 IO 和命令函数体，仅适配 GUI 条件及 Tauri 状态参数。它不覆盖 Tauri 宏、分发、桌面 GUI 或原生进程。
- 严格根 Clippy 在 Linux 系统依赖 `glib-2.0.pc` 缺失处阻断，尚未完成源码 lint。核心测试图的严格 Clippy 也因图外运行态调用者未纳入而出现 dead-code 诊断；不能代替完整根检查。仓库原 CI 安装 GTK / WebKit 依赖后执行完整后端检查。

最终候选仍需精确提交的 CI 与跨平台验收。真实 OAuth、新 Key、模型调用、macOS 原生切换、浏览器视觉及完整第三方应用兼容性未据此验证。不能将这些合成结果表述为生产或真实账号验收通过。
