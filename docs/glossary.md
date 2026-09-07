# LoongPort 术语表（唯源）

本仓的中转站域概念多、来源杂（服务端协议、上游 cc-switch、本地投影），同一事物曾用
tier / group / plan / provider 四个词交替指称。本表是这些术语的**唯一定义处**。

使用规则三条：

1. **新代码命名前先查本表**——概念用词跟着表走，别再造同义词。
2. **冻结名单是 wire/DB 契约名**（读宽写窄，认全历史值、只写当前值），别改。
3. **新概念先入表**（一行定义 + 代码落点）再写代码。

## 概念层级（一句话版）

```
中转站（relay site）── 一个域名
  └─ 账号行（relay account row）── 站点 × 一个登录账号
       └─ 分组（group，服务端事实）＝ 档位（tier，本地投影）
                                          └─ 存储为一条 provider 记录
```

## 核心术语

| 术语                     | 定义                                                                            | 代码落点                                                                                          | UI 用词     | 备注                                                       |
| ------------------------ | ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- | ----------- | ---------------------------------------------------------- |
| 中转站 relay site        | 一个域名背后聚合多家 CLI 模型转发的服务                                         | `site_origin`（归一化 origin；apex/取数地址的区分见 `relay/identity.rs`）；表 `loongport_relay`   | 中转站      | 「relay」取站点义时只用 site_origin 表述，别再造站名       |
| 账号行 relay account row | 中转站 × 一个登录账号（含登录态与凭据）；多账号同站并列展示的单位               | `creds::RelayAccount`（2026-09-07 前叫 `Relay`，一词三义之一，已改名）；参数名约定 `site_account` | 行          |                                                            |
| 分组 group               | **服务端事实**：站点把一个 sk 按一档价卖给账号                                  | wire `group_name` / `group_id`；`sub2api::Group` / `newapi::Group`                                | 分组        | 服务端字典；倍率、定价挂在它身上                           |
| 档位 tier                | 分组的**本地投影**：一条托管 provider 记录，可切换、可编辑                      | `TierInfo`（wire）、远端覆盖键 `tier_configs`、`relay_reset_tier_config`                          | 档位        | relay 域一律用 tier；一个分组可投影出多平台档位            |
| 套餐 plan                | **vendor 域**与档位对应的概念：同一官网账号的不同接入变体（Zen / Go）           | `VendorPlanInfo`、`plan_by_segment`、`vendor_reset_plan_config`                                   | 套餐        | ⚠️ vendor 域代码**禁用 tier**（曾混用，2026-09-07 修正）   |
| provider 记录            | 存储层统一载体（cc-switch 上游遗留）：两域的档位/套餐都存成它                   | `Provider`、`providers` 表、`providerId`                                                          | —（不外露） | 底层词；业务语义按所属域读（relay 的档位 / vendor 的套餐） |
| 官方 API vendor          | 官网直连账号域，与中转站平级并列                                                | `vendor/`、`Vendor` 枚举                                                                          | 官方 API    |                                                            |
| 平台 platform            | **sub2api 服务端**对 CLI 家族的称呼（openai / anthropic / gemini / grok）       | `platform_map::Platform`                                                                          | —           | 平台（服务端词）→ app（本地词）映射唯一源在 platform_map   |
| App                      | 本 app 管理的 CLI 应用（codex / claude / gemini / grokbuild / codex-image / …） | `AppType`（Rust）、`AppId`（TS）                                                                  | 页签名      | 「平台」是服务端视角、「app」是本地视角，别混用            |
| 广场 / 榜单 directory    | 中转站发现页与榜单                                                              | `relay_list_directory`、`LeaderboardKind`                                                         | 中转站广场  | VeriDrop 是**数据源**名（外部），不是功能名                |
| 实测 / 众测 crowd        | 用户共享的匿名实测指标                                                          | `crowd/`、metrics Worker                                                                          | 实测        |                                                            |
| 验真 verification        | 模型真伪探查（手动主动探针 + 被动异常上报）                                     | `relay/model_verification/`                                                                       | 验真        | 与熔断区分：验真管「真不真」，熔断管「还能不能用」         |
| 熔断 breaker             | 账号 / 站点级故障隔离与恢复                                                     | `CircuitBreaker`                                                                                  | —           |                                                            |
| 省心模式 easy mode       | 自动选路模式（与自主模式相对）                                                  | `auto_mode`                                                                                       | 省心        |                                                            |
| 生图栏 codex-image       | 纯生图档位所在的独立页签                                                        | `AppType::CodexImage`                                                                             | 生图        | 生图模型家族唯一源 `IMAGE_MODEL_FAMILIES`                  |
| 登录态 session           | 浏览器拿到的会话凭据及其寿命                                                    | refresh 链、`token_expires_at`                                                                    | 登录态      | 与凭据（credentials / sk）区分：登录态换 token，凭据是 key |

## 冻结名单（wire / DB 契约，读宽写窄，别改）

- `providerId`、`group_name` / `group_id`：wire + DB 双契约。
- `user_edited`：DB 列，DTO 字段同名跟进。
- 事件名（`events.rs` 主表 + 跨语言闸）与既有命令名（注册闸守着）。
- DB 表名 `loongport_relay`（行类型已改名 `RelayAccount`，表名与历史数据不动）。

改这些 = 迁移 / 跨端同步，不是重构；确有必要时按「读宽写窄」走（认全历史值、
写入只产当前值，参考 `codex_history_migration.rs` 的 legacy 数组先例）。

## 命名规矩（新代码硬规则）

1. 禁 `op` 这类未定义黑话参数名——账号行参数叫 `site_account`。
2. 禁 `do_` 前缀——命令实现直接用动词短语（如 `login_via_browser`）。
3. relay 域档位用 tier、vendor 域用 plan、存储层用 provider；跨域引用时带域定语
   （「vendor 的套餐」不说「vendor 的档位」）。
4. UI 文案用「UI 用词」列的中文；代码标识符用英文术语列，不自造缩写。

## 演进记录

- 2026-09-07：建表（09-06 全仓架构审查「四词一义」的收口）。随本表落地的改名：
  `creds::Relay` → `RelayAccount`、`op` 参数 → `site_account`、`do_login` →
  `login_via_browser`（relay 与 vendor 两处）、`vendor_reset_tier_config` →
  `vendor_reset_plan_config`。均为编译器可验证的纯内部名，wire/DB 契约未动。
