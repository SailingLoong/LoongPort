# OpenCode 本机 API 凭据导入

本入口只补齐便捷导入，不宣称修复新版 Console 的 OAuth 登录，也不代表原生账号积分池、余额、自动签到或真实消费已经验收。

## 使用与边界

在「官方 API → OpenCode」选择「同意只读预览本机 OpenCode 凭据」。仅该点击会读取本机 V2 数据库，界面不接收完整密钥。选择一条已有 API key，再选择「同意加密保存所选 API key」，才会保存到 LoongPort 的既有加密 Personal key owner。

- 数据库路径复用 `opencode_config::get_opencode_db_path`，支持既有 `OPENCODE_DB` 和 `XDG_DATA_HOME` 配置。不另建路径发现规则。
- 只读取 `credential` 表中 `integration_id = 'opencode'` 的记录。OAuth access/refresh token 和其他服务密钥不被选择或导入。
- 仅接受既有 owner 能验证格式的完整 Console API key。未知类型、组织会话绑定或非官方 Console 来源安全跳过；不根据昵称猜测远端身份。
- 不读取旧 `auth.json`，因为 OpenCode V2 迁移后不会继续向它回写新凭据。只在网页或 CLI 通过 OAuth 登录，不保证本机存在 API key。
- 不新建远端密钥、不发测试消费、不修改 OpenCode 当前选择，不自动切换任何应用的当前 provider。
- 预览票据只保存来源路径和摘要，不保存明文密钥。票据单次使用、五分钟过期。保存前复读当前数据库；来源路径、记录、选中态或凭据变化都会阻止旧预览保存。WAL 更新必须可见，不使用会跳过 WAL 的 immutable 模式。
- 同一 API key 复用既有幂等身份和加密保存逻辑，不另建账号池。

## 失败与取消

取消、预览过期、数据库忙、来源变化、未知格式等发生在目标保存前，不写入 LoongPort。进入保存后沿用既有 owner 的语义：加密凭据保存是原子的，后续多平台配置不是一个全局事务。若配置失败，返回 `configurationReady: false`，准确显示「凭据已保存，应用配置未完成」，由账号行刷新重试；不会误称完全失败、重复新建账号或自动删除并发凭据。

OAuth 登录仍需独立补齐第三方 client 资格、远端用户/组织身份与安全刷新契约。不能复用官方 `opencode-cli` client_id 冒充官方客户端；Console MCP 的动态注册也不能推定为模型推理 OAuth 注册授权。

## 官方合同

以下官方内容于 2026-10-22 核对，浮动分支升级前须重新核实：

- [V2 凭据存储与迁移](https://dev.opencode.ai/v2/docs/cli/providers)：V2 当前凭据在 SQLite，旧 auth.json 仅为迁移来源。
- [credential 表](https://github.com/anomalyco/opencode/blob/dev/packages/core/src/credential/sql.ts)：本地表、integration_id、value、active 字段。
- [当前凭据 schema](https://github.com/anomalyco/opencode/blob/dev/packages/schema/src/credential.ts) 与 [旧 V2 key schema](https://github.com/anomalyco/opencode/blob/dev/packages/schema/src/credential-legacy.ts)：key/OAuth 分型，configuration 迁移为 metadata 且旧 configuration 优先。
- [官方 OpenCode Console provider](https://github.com/anomalyco/opencode/blob/dev/packages/core/src/plugin/provider/opencode.ts)：设备 OAuth 使用官方 client_id，刷新及组织上下文属于专门契约。
- [其他 coding agent 的接入方式](https://opencode.ai/v2/docs/console/harnesses/)：个人 API key、固定推理网关，以及与推理认证不同的 Console MCP OAuth。
