use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::services::skill::{SkillStorageLocation, SyncMethod};

/// 自定义端点配置（历史兼容，实际存储在 provider.meta.custom_endpoints）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomEndpoint {
    pub url: String,
    pub added_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<i64>,
}

fn default_true() -> bool {
    true
}

/// 主页面显示的应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisibleApps {
    #[serde(default = "default_true")]
    pub claude: bool,
    #[serde(
        rename = "claude-desktop",
        alias = "claudeDesktop",
        alias = "claude_desktop",
        default = "default_true"
    )]
    pub claude_desktop: bool,
    #[serde(default = "default_true")]
    pub codex: bool,
    /// 生图标签是否可见。
    ///
    /// **默认可见**（与 codex 同步）：它不给用户增加负担 —— 没有任何生图分组时那一页
    /// 只有一句说明，不会误导人去买东西。反过来默认隐藏的话，买了生图分组的用户
    /// 找不到入口（上一版就是这么踩的：入口藏在 hover 里，用户报「没看到哪里有生图按钮」）。
    #[serde(
        rename = "codex-image",
        alias = "codexImage",
        alias = "codex_image",
        default = "default_true"
    )]
    pub codex_image: bool,
    #[serde(default = "default_true")]
    pub gemini: bool,
    #[serde(default = "default_true")]
    pub grokbuild: bool,
    #[serde(default = "default_true")]
    pub opencode: bool,
    #[serde(default = "default_true")]
    pub openclaw: bool,
    #[serde(default)]
    pub hermes: bool,
    #[serde(default = "default_true")]
    pub pi: bool,
}

impl Default for VisibleApps {
    /// 新装用户的默认可见集：只放 4 个主流量入口（Claude / Codex / Grok / 生图），
    /// 其余一律藏在「+」里由用户自己补回 —— 10 个 tab 平铺对新人是噪音，且
    /// tab 条带放不下时要靠滚动才能看全。已保存过 `visible_apps` 的用户不受
    /// 影响（读宽写窄：字段级 serde default 只服务旧配置文件，见上方各字段）。
    /// 与前端 `DEFAULT_VISIBLE_APPS`（appConfig.tsx）是同一事实，一致性由
    /// `default_visible_apps_match_the_frontend_copy` 钉住。
    fn default() -> Self {
        Self {
            claude: true,
            claude_desktop: false,
            codex: true,
            codex_image: true,
            gemini: false,
            grokbuild: true,
            opencode: false,
            openclaw: false,
            hermes: false,
            pi: false,
        }
    }
}

impl VisibleApps {
    /// Check if the specified app is visible
    pub fn is_visible(&self, app: &AppType) -> bool {
        match app {
            AppType::Claude => self.claude,
            AppType::ClaudeDesktop => self.claude_desktop,
            AppType::Codex => self.codex,
            AppType::CodexImage => self.codex_image,
            AppType::Gemini => self.gemini,
            AppType::GrokBuild => self.grokbuild,
            AppType::OpenCode => self.opencode,
            AppType::OpenClaw => self.openclaw,
            AppType::Hermes => self.hermes,
            AppType::Pi => self.pi,
        }
    }
}

/// WebDAV 同步状态（持久化同步进度信息）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSyncStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_remote_etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_local_manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_remote_manifest_hash: Option<String>,
}

fn default_remote_root() -> String {
    "loongport-sync".to_string()
}
fn default_profile() -> String {
    "default".to_string()
}

/// WebDAV 同步设置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSyncSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_remote_root")]
    pub remote_root: String,
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default)]
    pub status: WebDavSyncStatus,
}

impl Default for WebDavSyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_sync: false,
            base_url: String::new(),
            username: String::new(),
            password: String::new(),
            remote_root: default_remote_root(),
            profile: default_profile(),
            status: WebDavSyncStatus::default(),
        }
    }
}

impl WebDavSyncSettings {
    pub fn validate(&self) -> Result<(), crate::error::AppError> {
        if self.base_url.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "webdav.base_url.required",
                "WebDAV 地址不能为空",
                "WebDAV URL is required.",
            ));
        }
        if self.username.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "webdav.username.required",
                "WebDAV 用户名不能为空",
                "WebDAV username is required.",
            ));
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.base_url = self.base_url.trim().to_string();
        self.username = self.username.trim().to_string();
        self.remote_root = self.remote_root.trim().to_string();
        self.profile = self.profile.trim().to_string();
        if self.remote_root.is_empty() {
            self.remote_root = default_remote_root();
        }
        if self.profile.is_empty() {
            self.profile = default_profile();
        }
    }

    /// Returns true if all credential fields are blank (no config to persist).
    fn is_empty(&self) -> bool {
        self.base_url.is_empty() && self.username.is_empty() && self.password.is_empty()
    }
}

/// S3 同步设置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct S3SyncSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub access_key_id: String,
    #[serde(default)]
    pub secret_access_key: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default = "default_remote_root")]
    pub remote_root: String,
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default)]
    pub status: WebDavSyncStatus,
}

impl Default for S3SyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_sync: false,
            region: String::new(),
            bucket: String::new(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
            endpoint: String::new(),
            remote_root: default_remote_root(),
            profile: default_profile(),
            status: WebDavSyncStatus::default(),
        }
    }
}

impl S3SyncSettings {
    pub fn validate(&self) -> Result<(), crate::error::AppError> {
        if self.bucket.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "s3.bucket.required",
                "S3 存储桶不能为空",
                "S3 bucket is required.",
            ));
        }
        if self.region.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "s3.region.required",
                "S3 区域不能为空",
                "S3 region is required.",
            ));
        }
        if self.access_key_id.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "s3.access_key_id.required",
                "S3 Access Key ID 不能为空",
                "S3 Access Key ID is required.",
            ));
        }
        if self.secret_access_key.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "s3.secret_access_key.required",
                "S3 Secret Access Key 不能为空",
                "S3 Secret Access Key is required.",
            ));
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.region = self.region.trim().to_string();
        self.bucket = self.bucket.trim().to_string();
        self.access_key_id = self.access_key_id.trim().to_string();
        self.endpoint = self.endpoint.trim().to_string();
        self.remote_root = self.remote_root.trim().to_string();
        self.profile = self.profile.trim().to_string();
        if self.remote_root.is_empty() {
            self.remote_root = default_remote_root();
        }
        if self.profile.is_empty() {
            self.profile = default_profile();
        }
    }

    /// Returns true if all credential fields are blank (no config to persist).
    fn is_empty(&self) -> bool {
        self.bucket.is_empty()
            && self.region.is_empty()
            && self.access_key_id.is_empty()
            && self.secret_access_key.is_empty()
    }
}

/// 本机自动迁移状态。
///
/// 这里记录的是本机启动时执行过的一次性迁移；标记不随数据库同步。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalMigrations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_third_party_history_provider_bucket_v1:
        Option<CodexThirdPartyHistoryProviderBucketMigration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_provider_template_v1: Option<CodexProviderTemplateMigration>,
    /// 统一会话开关的官方历史迁移标记。开关关闭时会被清除，
    /// 这样重新开启能把"关闭期间"落入 openai 桶的官方会话补迁进来。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_official_history_unify_v1: Option<CodexOfficialHistoryUnifyMigration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThirdPartyHistoryProviderBucketMigration {
    pub completed_at: String,
    pub target_provider_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_provider_ids: Vec<String>,
    #[serde(default)]
    pub migrated_jsonl_files: usize,
    #[serde(default)]
    pub migrated_state_rows: usize,
    #[serde(default)]
    pub scanned_history_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexProviderTemplateMigration {
    pub completed_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub migrated_provider_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexOfficialHistoryUnifyMigration {
    pub completed_at: String,
    pub target_provider_id: String,
    #[serde(default)]
    pub migrated_jsonl_files: usize,
    #[serde(default)]
    pub migrated_state_rows: usize,
    /// 迁移时的规范化 Codex 目录。标记只对同一目录生效：
    /// 切换 codex_config_dir 后旧标记不会挡住新目录的迁移。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_config_dir: Option<String>,
}

/// 应用设置结构
///
/// 存储设备级别设置，保存在本地 `~/.cc-switch/settings.json`，不随数据库同步。
/// 这确保了云同步场景下多设备可以独立运作。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    // ===== 设备级 UI 设置 =====
    #[serde(default = "default_show_in_tray")]
    pub show_in_tray: bool,
    #[serde(default = "default_minimize_to_tray_on_close")]
    pub minimize_to_tray_on_close: bool,
    #[serde(default)]
    pub use_app_window_controls: bool,
    /// 是否启用 Claude 插件联动
    #[serde(default)]
    pub enable_claude_plugin_integration: bool,
    /// 是否跳过 Claude Code 初次安装确认
    #[serde(default)]
    pub skip_claude_onboarding: bool,
    /// 是否开机自启
    #[serde(default)]
    pub launch_on_startup: bool,
    /// 静默启动（程序启动时不显示主窗口，仅托盘运行）
    #[serde(default)]
    pub silent_startup: bool,
    /// 是否在主页面启用本地代理功能（默认关闭）
    #[serde(default)]
    pub enable_local_proxy: bool,
    /// User has confirmed the local proxy first-run notice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_confirmed: Option<bool>,
    /// User has confirmed the usage query first-run notice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_confirmed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_dashboard_refresh_interval_ms: Option<u32>,
    /// 匿名使用统计：上报安装 id / 版本 / OS / 站点域名（载荷边界见 `relay::stats` 模块文档）。
    ///
    /// Fresh installations keep sharing off until an explicit choice is saved.
    /// The serde default preserves the previous behavior for existing settings.
    #[serde(default = "default_true")]
    pub enable_anonymous_stats: bool,
    /// 用户看过那条「匿名统计上报什么」的首启告知了没。
    ///
    /// Legacy acknowledgement, retained for existing settings compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats_notice_confirmed: Option<bool>,
    /// 匿名统计**专属**的随机安装 id。
    ///
    /// ⚠️ **绝不复用 `creds` 的 `device_id`**，也别拿中转站账号 id 当它。
    ///
    /// 原来的理由是「`device_id` 会被写进中转站服务器上的 API key 名
    /// （`LoongPort/<device_id>/…`），看得到它的中转站就能把一条上报对回一个付费账号」。
    /// 那个命名 2026-08-04 起改成了按账号（`LoongPort/a<account-id>/…`），
    /// 所以 `device_id` 现在不出现在任何服务端数据里 —— **但这条禁令照旧**：
    /// 统计 id 与任何「能对回某个人」的标识永不交叉，是这个字段存在的全部理由，
    /// 不该因为其中一条关联恰好消失就放松。
    ///
    /// 这个 id 只用于统计去重。由**后端在首次上报时自生成**（只在开关开着时 ——
    /// 从一开始就关的用户机器上不落地 id），跨启动复用：「关了再开」仍是同一个
    /// 安装，不许被计成两个。
    ///
    /// 随机 UUID、不含任何设备指纹（不取硬件序列号 / MAC / hostname）——
    /// 它回答「有多少个安装」，不是「这是谁」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats_install_id: Option<String>,
    /// 站点实测数据共建：上传本地聚合指标（小时桶，见 `crowd` 模块文档那张表），
    /// 同时解锁广场的实测数据展示（对等条款）。
    ///
    /// Fresh installations default off; existing installations retain their choice.
    #[serde(default = "default_true")]
    pub crowd_metrics_enabled: bool,
    /// Legacy acknowledgement; onboarding now records the sharing decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crowd_metrics_notice_confirmed: Option<bool>,
    /// 首启「是否一键导入 cc-switch 配置」问过没有。
    ///
    /// `None` = 还没问过 ⇒ 前端在第一次打开时弹一次（前提是检测到 `~/.cc-switch/cc-switch.db`）。
    /// 与 `stats_notice_confirmed` / `proxy_confirmed` / `usage_confirmed` 同一个惯例。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc_switch_import_prompted: Option<bool>,
    /// 「点 Star 领注册礼」的礼领过没有。
    ///
    /// `None` = 还没领 ⇒ 顶栏 GitHub 入口的红点保持亮（前提是远端配置还有这个
    /// 活动），点击仍会弹邀请窗。领过即永久置位 —— 码只发一次，之后的红点、
    /// 邀请窗都退场，按钮回到「直接开仓库」。与 `cc_switch_import_prompted`
    /// 等一次性标志同一个惯例。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub star_reward_claimed: Option<bool>,
    /// 用户明确「跳过本版本」的应用更新版本号。
    ///
    /// `None` = 没跳过过任何版本。启动闸门（`services::app_update` 的
    /// 待装更新应用）读到它等于待装版本时放弃自动安装，保留预下载产物供
    /// 手动升级；前端更新徽章与「跳过本版本」按钮同源读写。设备级、不随
    /// 云同步 —— 要不要装新版本是单台机器上的决定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_update_version: Option<String>,
    /// Whether to show the failover toggle independently on the main page
    #[serde(default)]
    pub enable_failover_toggle: bool,
    /// Whether to show the project profile switcher on the main page header
    #[serde(default = "default_show_profile_switcher")]
    pub show_profile_switcher: bool,
    /// 切到第三方 provider 时保住 `~/.codex/auth.json` 里的官方 ChatGPT 登录凭据。
    ///
    /// **LoongPort 默认开（上游默认关）。** `~/.codex/auth.json` 就是 ChatGPT 桌面版的登录
    /// 凭据所在 —— 那个 app（bundle id `com.openai.codex`）自带一份 codex 核心二进制，与命令行
    /// codex 共用同一个 `~/.codex`。关掉这个开关意味着每次切分组都整体覆写 auth.json，把用户的
    /// ChatGPT 登录一起清掉，重开 app 就要求重新登录。
    ///
    /// 开着时 sub2api 的 sk 走 `[model_providers.custom].experimental_bearer_token`
    /// 落在 config.toml 里，auth.json 全程不碰 —— 这也正是「只切 config.toml」要的语义。
    ///
    /// 注意默认值要改**两处**：这里的 serde default（决定已有 settings.json 缺这个键时读成
    /// 什么）与 `Default` impl（决定文件不存在时的新装机值）。只改后者对老用户无效。
    #[serde(default = "default_true")]
    pub preserve_codex_official_auth_on_switch: bool,
    /// 让官方 Codex provider 也跑在共享的 `custom` model_provider id 下，从而与第三方
    /// provider 共用一个会话历史桶。
    ///
    /// **LoongPort 默认开**，让后续创建的官方与第三方会话出现在同一历史列表中；用户仍可
    /// 手动关闭。存量官方会话迁移由 `unify_codex_migrate_existing` 单独记录用户意愿，默认开启
    /// 本开关不会自动迁移历史。
    ///
    /// 与 `preserve_codex_official_auth_on_switch` 一样，字段级 serde default 负责已有配置缺键
    /// 的场景，`Default` impl 负责新装机，两处必须保持一致。
    #[serde(default = "default_true")]
    pub unify_codex_session_history: bool,
    /// User opted in (via the enable dialog checkbox) to migrate existing
    /// official sessions ("openai" bucket) into the shared bucket. Persisted so
    /// a failed migration retries at startup; cleared when the toggle turns off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unify_codex_migrate_existing: Option<bool>,
    /// User has confirmed the failover toggle first-run notice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failover_confirmed: Option<bool>,
    /// User has confirmed the first-run welcome notice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_run_notice_confirmed: Option<bool>,
    /// User has confirmed the common config first-run notice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_config_confirmed: Option<bool>,
    /// 「点 Star 领注册礼」的主动邀请**已经弹过**一次（2026-08-17 起挂在首个
    /// 站点接入成功之后，见 `commands::onboarding`）。
    ///
    // （`star_reward_offered` 一次性标志随 2026-09-06 删除主动弹窗一起移除：
    // 它存在的唯一意义是防弹窗重复触发，弹窗没了字段就是死状态。旧
    // settings.json 里的该键会被 serde 忽略并在下次全量保存时自然挤掉。）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    // ===== 主页面显示的应用 =====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_apps: Option<VisibleApps>,

    /// 中转站广场开关（设置页那个开关的本体）。
    ///
    /// None means unclassified and hidden. Explicit user choices always win.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plaza_visible: Option<bool>,
    /// First explicitly submitted domain, retained until attribution succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plaza_first_site_domain: Option<String>,
    /// Existing settings predate onboarding; only fresh installations start pending.
    #[serde(default = "default_true")]
    pub service_onboarding_completed: bool,
    #[serde(default)]
    pub service_onboarding_dismissed: bool,

    // ===== 设备级目录覆盖 =====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grok_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openclaw_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hermes_config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_config_dir: Option<String>,

    // ===== 当前供应商 ID（设备级）=====
    /// 当前 Claude 供应商 ID（本地存储，优先于数据库 is_current）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_claude: Option<String>,
    /// 当前 Claude Desktop 供应商 ID（本地存储，优先于数据库 is_current）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_claude_desktop: Option<String>,
    /// 当前 Codex 供应商 ID（本地存储，优先于数据库 is_current）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_codex: Option<String>,
    /// 当前**生图**档位 ID（本地存储，优先于数据库 is_current）。
    ///
    /// 与 `current_provider_codex` **各自独立**：用户可以一边用 DeepSeek 聊天、
    /// 一边用鑫旺的 4K 分组生图。见 [`AppType::CodexImage`](crate::app_config::AppType::CodexImage)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_codex_image: Option<String>,
    /// 是否把生图工具注册进 codex / claude / gemini（MCP）。`None` = 开（默认）。
    ///
    /// 关掉它的是「只想直接生图」的用户：工具只要注册着，其描述就占宿主每次会话的
    /// 上下文、模型还可能主动调用 —— 他们要的是**根本不注册**。开关只管注册，
    /// 不管 App 内直接生图（生图页「生成」视图永远可用，见 `relay::imagegen`）。
    /// 生效点在 `relay::imagegen_mcp::sync_registration`（幂等，切开关即对齐）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imagegen_mcp_enabled: Option<bool>,
    /// 生图文件的存储目录（**绝对路径**）。`None` = 默认 `<数据目录>/generated_images/`。
    ///
    /// 设备级（不进云同步 —— 另一台机器的磁盘布局不同）；由生图页「更改存储位置」
    /// 写入。读取方是 `relay::imagegen::output_dir`（**直读 settings.json 文件**，
    /// MCP 子进程没有主程序的设置缓存，读缓存会两边分叉）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imagegen_output_dir: Option<String>,
    /// 当前 Gemini 供应商 ID（本地存储，优先于数据库 is_current）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_gemini: Option<String>,
    /// 当前 Grok Build 供应商 ID（本地存储，优先于数据库 is_current）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_grokbuild: Option<String>,
    /// 当前 OpenCode 供应商 ID（本地存储，对 OpenCode 可能无意义，但保持结构一致）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_opencode: Option<String>,
    /// 当前 OpenClaw 供应商 ID（本地存储，对 OpenClaw 可能无意义，但保持结构一致）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_openclaw: Option<String>,
    /// 当前 Hermes 供应商 ID（本地存储，保持结构一致）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_hermes: Option<String>,

    // ===== Skill 同步设置 =====
    /// Skill 同步方式：auto（默认，优先 symlink）、symlink、copy
    #[serde(default)]
    pub skill_sync_method: SyncMethod,
    /// Skill 存储位置：loongport（默认，~/.loongport/skills/）或 unified（~/.agents/skills/）
    #[serde(default)]
    pub skill_storage_location: SkillStorageLocation,

    // ===== WebDAV 同步设置 =====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_sync: Option<WebDavSyncSettings>,

    // ===== S3 同步设置 =====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3_sync: Option<S3SyncSettings>,

    // ===== WebDAV 备份设置（旧版，保留向后兼容）=====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_backup: Option<serde_json::Value>,

    // ===== 备份策略设置 =====
    /// Auto-backup interval in hours (default 24, 0 = disabled)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_interval_hours: Option<u32>,
    /// Maximum number of backup files to retain (default 10)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_retain_count: Option<u32>,

    // ===== 终端设置 =====
    /// 首选终端应用（可选，默认使用系统默认终端）
    /// - macOS: "terminal" | "iterm2" | "warp" | "alacritty" | "kitty" | "ghostty" | "wezterm" | "kaku"
    /// - Windows: "cmd" | "powershell" | "wt" (Windows Terminal)
    /// - Linux: "gnome-terminal" | "konsole" | "xfce4-terminal" | "alacritty" | "kitty" | "ghostty"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_terminal: Option<String>,

    // ===== 本机自动迁移状态 =====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_migrations: Option<LocalMigrations>,
}

fn default_show_in_tray() -> bool {
    true
}

fn default_minimize_to_tray_on_close() -> bool {
    true
}

fn default_show_profile_switcher() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_in_tray: true,
            minimize_to_tray_on_close: true,
            use_app_window_controls: false,
            enable_claude_plugin_integration: false,
            skip_claude_onboarding: false,
            launch_on_startup: false,
            silent_startup: false,
            enable_local_proxy: false,
            proxy_confirmed: None,
            usage_confirmed: None,
            usage_dashboard_refresh_interval_ms: None,
            // New installations await an explicit sharing decision.
            enable_anonymous_stats: false,
            stats_notice_confirmed: None,
            stats_install_id: None,

            crowd_metrics_enabled: false,
            crowd_metrics_notice_confirmed: None,
            cc_switch_import_prompted: None,
            star_reward_claimed: None,
            dismissed_update_version: None,
            enable_failover_toggle: false,
            show_profile_switcher: true,
            // 见字段上的说明：这条保的是 ChatGPT 桌面版的登录凭据，LoongPort 必须默认开。
            preserve_codex_official_auth_on_switch: true,
            unify_codex_session_history: true,
            unify_codex_migrate_existing: None,
            failover_confirmed: None,
            first_run_notice_confirmed: None,
            common_config_confirmed: None,
            language: None,
            visible_apps: None,
            plaza_visible: None,
            plaza_first_site_domain: None,
            service_onboarding_completed: false,
            service_onboarding_dismissed: false,
            claude_config_dir: None,
            codex_config_dir: None,
            gemini_config_dir: None,
            grok_config_dir: None,
            opencode_config_dir: None,
            openclaw_config_dir: None,
            hermes_config_dir: None,
            pi_config_dir: None,
            current_provider_claude: None,
            current_provider_claude_desktop: None,
            current_provider_codex: None,
            current_provider_codex_image: None,
            imagegen_mcp_enabled: None,
            imagegen_output_dir: None,
            current_provider_gemini: None,
            current_provider_grokbuild: None,
            current_provider_opencode: None,
            current_provider_openclaw: None,
            current_provider_hermes: None,
            skill_sync_method: SyncMethod::default(),
            skill_storage_location: SkillStorageLocation::default(),
            webdav_sync: None,
            s3_sync: None,
            webdav_backup: None,
            backup_interval_hours: None,
            backup_retain_count: None,
            preferred_terminal: None,
            local_migrations: None,
        }
    }
}

impl AppSettings {
    fn settings_path() -> Option<PathBuf> {
        // settings.json 保留用于旧版本迁移和无数据库场景
        Some(
            crate::config::get_home_dir()
                .join(crate::config::APP_DIR_NAME)
                .join("settings.json"),
        )
    }

    fn normalize_paths(&mut self) {
        self.claude_config_dir = self
            .claude_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.codex_config_dir = self
            .codex_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.gemini_config_dir = self
            .gemini_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.grok_config_dir = self
            .grok_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.opencode_config_dir = self
            .opencode_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.openclaw_config_dir = self
            .openclaw_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.hermes_config_dir = self
            .hermes_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.pi_config_dir = self
            .pi_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.language = self
            .language
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| matches!(*s, "en" | "zh" | "zh-TW" | "ja"))
            .map(|s| s.to_string());

        if let Some(sync) = &mut self.webdav_sync {
            sync.normalize();
            if sync.is_empty() {
                self.webdav_sync = None;
            }
        }

        if let Some(s3) = &mut self.s3_sync {
            s3.normalize();
            if s3.is_empty() {
                self.s3_sync = None;
            }
        }
    }

    fn load_from_file() -> Self {
        let Some(path) = Self::settings_path() else {
            return Self::default();
        };
        if let Ok(content) = fs::read_to_string(&path) {
            match serde_json::from_str::<AppSettings>(&content) {
                Ok(mut settings) => {
                    settings.normalize_paths();
                    settings
                }
                Err(err) => {
                    log::warn!(
                        "解析设置文件失败，将使用默认设置。路径: {}, 错误: {}",
                        path.display(),
                        err
                    );
                    Self::default()
                }
            }
        } else {
            Self::default()
        }
    }
}

fn save_settings_file(settings: &AppSettings) -> Result<(), AppError> {
    let mut normalized = settings.clone();
    normalized.normalize_paths();
    let Some(path) = AppSettings::settings_path() else {
        return Err(AppError::Config("无法获取用户主目录".to_string()));
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let json = serde_json::to_string_pretty(&normalized)
        .map_err(|e| AppError::JsonSerialize { source: e })?;
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| AppError::io(&path, e))?;
        file.write_all(json.as_bytes())
            .map_err(|e| AppError::io(&path, e))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(&path, json).map_err(|e| AppError::io(&path, e))?;
    }

    Ok(())
}

static SETTINGS_STORE: OnceLock<RwLock<AppSettings>> = OnceLock::new();

fn settings_store() -> &'static RwLock<AppSettings> {
    SETTINGS_STORE.get_or_init(|| RwLock::new(AppSettings::load_from_file()))
}

pub(crate) fn resolve_override_path(raw: &str) -> PathBuf {
    let join_home = |home: PathBuf, suffix: &str| {
        suffix
            .split(['/', '\\'])
            .filter(|component| !component.is_empty())
            .fold(home, |path, component| path.join(component))
    };

    if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    } else if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return join_home(home, stripped);
        }
    } else if let Some(stripped) = raw.strip_prefix("~\\") {
        if let Some(home) = dirs::home_dir() {
            return join_home(home, stripped);
        }
    }

    PathBuf::from(raw)
}

pub fn get_settings() -> AppSettings {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .clone()
}

fn prepare_settings_for_frontend(mut settings: AppSettings) -> AppSettings {
    settings
        .visible_apps
        .get_or_insert_with(VisibleApps::default);
    if let Some(sync) = &mut settings.webdav_sync {
        sync.password.clear();
    }
    if let Some(s3) = &mut settings.s3_sync {
        s3.secret_access_key.clear();
    }
    settings.webdav_backup = None;
    settings
}

pub fn get_settings_for_frontend() -> AppSettings {
    prepare_settings_for_frontend(get_settings())
}

pub fn update_settings(mut new_settings: AppSettings) -> Result<(), AppError> {
    new_settings.normalize_paths();
    save_settings_file(&new_settings)?;

    let mut guard = settings_store().write().unwrap_or_else(|e| {
        log::warn!("设置锁已毒化，使用恢复值: {e}");
        e.into_inner()
    });
    *guard = new_settings;
    Ok(())
}

// pub(crate)：LoongPort 的窄命令（如 `star_reward_mark_claimed`）用它做后端
// RMW 写专有字段，与上面的迁移标记写入者同一形状。
pub(crate) fn mutate_settings<F>(mutator: F) -> Result<(), AppError>
where
    F: FnOnce(&mut AppSettings),
{
    let mut guard = settings_store().write().unwrap_or_else(|e| {
        log::warn!("设置锁已毒化，使用恢复值: {e}");
        e.into_inner()
    });
    let mut next = guard.clone();
    mutator(&mut next);
    next.normalize_paths();
    save_settings_file(&next)?;
    *guard = next;
    Ok(())
}

pub fn is_codex_third_party_history_provider_bucket_migrated() -> bool {
    get_settings()
        .local_migrations
        .as_ref()
        .and_then(|migrations| {
            migrations
                .codex_third_party_history_provider_bucket_v1
                .as_ref()
        })
        .is_some_and(|m| m.scanned_history_files)
}

pub fn mark_codex_third_party_history_provider_bucket_migrated(
    migration: CodexThirdPartyHistoryProviderBucketMigration,
) -> Result<(), AppError> {
    mutate_settings(|settings| {
        let migrations = settings
            .local_migrations
            .get_or_insert_with(Default::default);
        migrations.codex_third_party_history_provider_bucket_v1 = Some(migration);
    })
}

pub fn is_codex_provider_template_migrated() -> bool {
    get_settings()
        .local_migrations
        .as_ref()
        .and_then(|migrations| migrations.codex_provider_template_v1.as_ref())
        .is_some()
}

pub fn mark_codex_provider_template_migrated(
    migration: CodexProviderTemplateMigration,
) -> Result<(), AppError> {
    mutate_settings(|settings| {
        let migrations = settings
            .local_migrations
            .get_or_insert_with(Default::default);
        migrations.codex_provider_template_v1 = Some(migration);
    })
}

/// 统一会话迁移标记是否覆盖指定目录。标记里没记目录（不应出现的旧格式）
/// 视为不匹配——重跑迁移是幂等的，宁可重迁也不漏迁。
pub fn is_codex_official_history_unify_migrated_for_dir(codex_dir: &str) -> bool {
    get_settings()
        .local_migrations
        .as_ref()
        .and_then(|migrations| migrations.codex_official_history_unify_v1.as_ref())
        .is_some_and(|migration| migration.codex_config_dir.as_deref() == Some(codex_dir))
}

/// 条件写入迁移完成标记：仅当此刻开关仍开启且迁移意愿仍在时才写。
/// 检查与写入在 settings 写锁内原子完成，与关闭开关路径
/// （`update_settings` / 清标记）串行，消除"迁移线程复查开关后、写标记前
/// 用户恰好关闭开关"的竞态窗口。返回是否实际写入。
pub fn mark_codex_official_history_unify_migrated_if_enabled(
    migration: CodexOfficialHistoryUnifyMigration,
) -> Result<bool, AppError> {
    let mut written = false;
    mutate_settings(|settings| {
        if settings.unify_codex_session_history
            && settings.unify_codex_migrate_existing.unwrap_or(false)
        {
            settings
                .local_migrations
                .get_or_insert_with(Default::default)
                .codex_official_history_unify_v1 = Some(migration);
            written = true;
        }
    })?;
    Ok(written)
}

pub fn clear_codex_official_history_unify_migration() -> Result<(), AppError> {
    mutate_settings(|settings| {
        if let Some(migrations) = settings.local_migrations.as_mut() {
            migrations.codex_official_history_unify_v1 = None;
        }
    })
}

pub fn unify_codex_migrate_existing_requested() -> bool {
    get_settings().unify_codex_migrate_existing.unwrap_or(false)
}

pub fn clear_codex_unify_migrate_existing() -> Result<(), AppError> {
    mutate_settings(|settings| {
        settings.unify_codex_migrate_existing = None;
    })
}

/// 从文件重新加载设置到内存缓存
/// 用于导入配置等场景，确保内存缓存与文件同步
pub fn reload_settings() -> Result<(), AppError> {
    let fresh_settings = AppSettings::load_from_file();
    let mut guard = settings_store().write().unwrap_or_else(|e| {
        log::warn!("设置锁已毒化，使用恢复值: {e}");
        e.into_inner()
    });
    *guard = fresh_settings;
    Ok(())
}

pub fn get_claude_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .claude_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_codex_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .codex_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_gemini_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .gemini_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_grok_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .grok_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_opencode_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .opencode_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_openclaw_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .openclaw_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_hermes_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .hermes_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}

pub fn get_pi_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .pi_config_dir
        .as_ref()
        .map(|path| resolve_override_path(path))
}

pub fn preserve_codex_official_auth_on_switch() -> bool {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .preserve_codex_official_auth_on_switch
}

pub fn unify_codex_session_history() -> bool {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .unify_codex_session_history
}

// ===== 当前供应商管理函数 =====

/// 获取指定应用类型的当前供应商 ID（从本地 settings 读取）
///
/// 这是设备级别的设置，不随数据库同步。
/// 如果本地没有设置，调用者应该 fallback 到数据库的 `is_current` 字段。
pub fn get_current_provider(app_type: &AppType) -> Option<String> {
    let settings = settings_store().read().ok()?;
    match app_type {
        AppType::Claude => settings.current_provider_claude.clone(),
        AppType::ClaudeDesktop => settings.current_provider_claude_desktop.clone(),
        AppType::Codex => settings.current_provider_codex.clone(),
        AppType::CodexImage => settings.current_provider_codex_image.clone(),
        AppType::Gemini => settings.current_provider_gemini.clone(),
        AppType::GrokBuild => settings.current_provider_grokbuild.clone(),
        AppType::OpenCode => settings.current_provider_opencode.clone(),
        AppType::OpenClaw => settings.current_provider_openclaw.clone(),
        AppType::Hermes => settings.current_provider_hermes.clone(),
        AppType::Pi => None,
    }
}

/// 生图工具是否注册进 codex / claude / gemini（MCP）。缺省 = 开：
/// 升级用户的注册行为不变，这是一个明确的产品决定，不是随手默认。
pub fn get_imagegen_mcp_enabled() -> bool {
    settings_store()
        .read()
        .map(|settings| settings.imagegen_mcp_enabled.unwrap_or(true))
        .unwrap_or(true)
}

/// 设置生图 MCP 注册开关并落盘。注册状态的对齐由调用方触发
/// （`relay_set_imagegen_mcp_enabled` 命令里跟着跑一次 `sync_registration`）。
pub fn set_imagegen_mcp_enabled(enabled: bool) -> Result<(), AppError> {
    mutate_settings(|settings| {
        settings.imagegen_mcp_enabled = Some(enabled);
    })
}

/// 设置生图存储目录并落盘（绝对路径，校验在命令层）。**写入即生效**：
/// 两个入口（App 内与 MCP）每次生图/列表都现读 `imagegen::output_dir()`，
/// 没有「要重启才认新路径」这回事。
pub fn set_imagegen_output_dir(dir: String) -> Result<(), AppError> {
    mutate_settings(|settings| {
        settings.imagegen_output_dir = Some(dir);
    })
}

/// 设置指定应用类型的当前供应商 ID（保存到本地 settings）
///
/// 这是设备级别的设置，不随数据库同步。
/// 传入 `None` 会清除当前供应商设置。
pub fn set_current_provider(app_type: &AppType, id: Option<&str>) -> Result<(), AppError> {
    let id_owned = id.map(|s| s.to_string());
    mutate_settings(|settings| match app_type {
        AppType::Claude => settings.current_provider_claude = id_owned.clone(),
        AppType::ClaudeDesktop => settings.current_provider_claude_desktop = id_owned.clone(),
        AppType::Codex => settings.current_provider_codex = id_owned.clone(),
        AppType::CodexImage => settings.current_provider_codex_image = id_owned.clone(),
        AppType::Gemini => settings.current_provider_gemini = id_owned.clone(),
        AppType::GrokBuild => settings.current_provider_grokbuild = id_owned.clone(),
        AppType::OpenCode => settings.current_provider_opencode = id_owned.clone(),
        AppType::OpenClaw => settings.current_provider_openclaw = id_owned.clone(),
        AppType::Hermes => settings.current_provider_hermes = id_owned.clone(),
        AppType::Pi => {}
    })
}

/// 获取有效的当前供应商 ID（验证存在性）
///
/// 逻辑：
/// 1. 从本地 settings 读取当前供应商 ID
/// 2. 验证该 ID 在数据库中存在
/// 3. 如果不存在则清理本地 settings，fallback 到数据库的 is_current
///
/// 这确保了返回的 ID 一定是有效的（在数据库中存在）。
/// 多设备云同步场景下，配置导入后本地 ID 可能失效，此函数会自动修复。
pub fn get_effective_current_provider(
    db: &crate::database::Database,
    app_type: &AppType,
) -> Result<Option<String>, AppError> {
    // 1. 从本地 settings 读取
    if let Some(local_id) = get_current_provider(app_type) {
        // 2. 验证该 ID 在数据库中存在
        let providers = db.get_all_providers(app_type.as_str())?;
        if providers.contains_key(&local_id) {
            // 存在，直接返回
            return Ok(Some(local_id));
        }

        // 3. 不存在，清理本地 settings
        log::warn!(
            "本地 settings 中的供应商 {} ({}) 在数据库中不存在，将清理并 fallback 到数据库",
            local_id,
            app_type.as_str()
        );
        let _ = set_current_provider(app_type, None);
    }

    // Fallback 到数据库的 is_current
    db.get_current_provider(app_type.as_str())
}

// ===== Skill 同步方式管理函数 =====

/// 获取 Skill 同步方式配置
pub fn get_skill_sync_method() -> SyncMethod {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .skill_sync_method
}

// ===== Skill 存储位置管理函数 =====

/// 获取 Skill 存储位置配置
pub fn get_skill_storage_location() -> SkillStorageLocation {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .skill_storage_location
}

/// 设置 Skill 存储位置
pub fn set_skill_storage_location(location: SkillStorageLocation) -> Result<(), AppError> {
    mutate_settings(|s| {
        s.skill_storage_location = location;
    })
}

// ===== 备份策略管理函数 =====

/// Get the effective auto-backup interval in hours (default 24)
pub fn effective_backup_interval_hours() -> u32 {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .backup_interval_hours
        .unwrap_or(24)
}

/// Get the effective backup retain count (default 10, minimum 1)
pub fn effective_backup_retain_count() -> usize {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .backup_retain_count
        .map(|n| (n as usize).max(1))
        .unwrap_or(10)
}

// ===== 终端设置管理函数 =====

/// 获取首选终端应用
pub fn get_preferred_terminal() -> Option<String> {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .preferred_terminal
        .clone()
}

// ===== WebDAV 同步设置管理函数 =====

/// 获取 WebDAV 同步设置
pub fn get_webdav_sync_settings() -> Option<WebDavSyncSettings> {
    settings_store().read().ok()?.webdav_sync.clone()
}

/// 保存 WebDAV 同步设置
pub fn set_webdav_sync_settings(settings: Option<WebDavSyncSettings>) -> Result<(), AppError> {
    mutate_settings(|current| {
        current.webdav_sync = settings;
    })
}

/// 仅更新 WebDAV 同步状态，避免覆写 credentials/root/profile 等字段
pub fn update_webdav_sync_status(status: WebDavSyncStatus) -> Result<(), AppError> {
    mutate_settings(|current| {
        if let Some(sync) = current.webdav_sync.as_mut() {
            sync.status = status;
        }
    })
}

// ===== S3 同步设置管理函数 =====

pub fn get_s3_sync_settings() -> Option<S3SyncSettings> {
    settings_store().read().ok()?.s3_sync.clone()
}

pub fn set_s3_sync_settings(settings: Option<S3SyncSettings>) -> Result<(), AppError> {
    mutate_settings(|current| {
        current.s3_sync = settings;
    })
}

pub fn update_s3_sync_status(status: WebDavSyncStatus) -> Result<(), AppError> {
    mutate_settings(|current| {
        if let Some(s3) = current.s3_sync.as_mut() {
            s3.status = status;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppType;

    #[test]
    fn preserve_codex_official_auth_defaults_on_in_both_paths() {
        // 这条守的是「切分组不会打掉 ChatGPT 桌面版的登录」。
        //
        // 必须两条路径都验：`Default` impl 管「settings.json 不存在」（新装机），
        // 字段级 serde default 管「文件存在但缺这个键」（从上游版本升上来的用户）。
        // 只改一个的话另一条路会静默回落到 false，然后每次切分组覆写 auth.json。
        assert!(
            AppSettings::default().preserve_codex_official_auth_on_switch,
            "新装机默认必须开"
        );

        let from_partial: AppSettings = serde_json::from_str("{}").expect("空对象应能解析");
        assert!(
            from_partial.preserve_codex_official_auth_on_switch,
            "settings.json 缺这个键时必须读成 true"
        );

        // 用户显式关掉时要尊重他的选择，不能被默认值强行盖回。
        let explicit_off: AppSettings =
            serde_json::from_str(r#"{"preserveCodexOfficialAuthOnSwitch":false}"#)
                .expect("显式 false 应能解析");
        assert!(!explicit_off.preserve_codex_official_auth_on_switch);
    }

    #[test]
    fn unify_codex_session_history_defaults_on_in_both_paths() {
        assert!(AppSettings::default().unify_codex_session_history);

        let from_partial: AppSettings = serde_json::from_str("{}").expect("空对象应能解析");
        assert!(from_partial.unify_codex_session_history);
        // 存量迁移仍必须由用户单独选择，不能随开关默认开启。
        assert!(from_partial.unify_codex_migrate_existing.is_none());

        let explicit_off: AppSettings =
            serde_json::from_str(r#"{"unifyCodexSessionHistory":false}"#)
                .expect("显式 false 应能解析");
        assert!(!explicit_off.unify_codex_session_history);
    }

    #[test]
    fn fresh_sharing_is_off_and_existing_preferences_are_preserved() {
        assert!(!AppSettings::default().crowd_metrics_enabled);
        assert!(!AppSettings::default().enable_anonymous_stats);
        let fresh = AppSettings::default();
        let restored: AppSettings =
            serde_json::from_value(serde_json::to_value(&fresh).unwrap()).unwrap();
        assert!(!restored.service_onboarding_completed);
        assert!(!restored.enable_anonymous_stats && !restored.crowd_metrics_enabled);
        let from_partial: AppSettings = serde_json::from_str("{}").expect("空对象应能解析");
        assert!(
            from_partial.crowd_metrics_enabled,
            "settings.json 缺这个键时必须读成 true"
        );

        // 关键不变式：点过「暂不参与」的用户存的是显式 false，翻默认值绝不能
        // 把他们静默重新拉进上传 —— 那等于推翻用户已做的明确选择。
        let opted_out: AppSettings = serde_json::from_str(
            r#"{"crowdMetricsEnabled":false,"crowdMetricsNoticeConfirmed":true}"#,
        )
        .expect("显式拒绝应能解析");
        assert!(!opted_out.crowd_metrics_enabled);
        assert_eq!(opted_out.crowd_metrics_notice_confirmed, Some(true));
    }

    #[test]
    fn visible_apps_old_settings_default_claude_desktop_visible() {
        let visible: VisibleApps = serde_json::from_value(serde_json::json!({
            "claude": true,
            "codex": true,
            "gemini": true,
            "opencode": true,
            "openclaw": true,
            "hermes": true
        }))
        .expect("visible apps");

        assert!(visible.is_visible(&AppType::ClaudeDesktop));
    }

    #[test]
    fn visible_apps_accepts_claude_desktop_aliases() {
        let visible: VisibleApps = serde_json::from_value(serde_json::json!({
            "claude": true,
            "claudeDesktop": false,
            "codex": true,
            "gemini": true,
            "opencode": true,
            "openclaw": true,
            "hermes": true
        }))
        .expect("visible apps");

        assert!(!visible.is_visible(&AppType::ClaudeDesktop));
    }

    #[test]
    fn frontend_settings_materialize_backend_visible_app_defaults() {
        let settings = AppSettings {
            visible_apps: None,
            ..Default::default()
        };

        let frontend = prepare_settings_for_frontend(settings);
        let visible = frontend
            .visible_apps
            .expect("frontend settings must include visible apps");

        // 默认可见集 = Claude / Codex / Grok / 生图 四个（见 VisibleApps::default 注释）
        assert!(visible.claude);
        assert!(visible.codex);
        assert!(visible.grokbuild);
        assert!(visible.codex_image);
        assert!(!visible.claude_desktop);
        assert!(!visible.gemini);
        assert!(!visible.opencode);
        assert!(!visible.openclaw);
        assert!(!visible.hermes);
        assert!(!visible.pi);
    }

    /// 「新装用户默认看哪些 app」在 Rust（`VisibleApps::default`）与 TS
    /// （`DEFAULT_VISIBLE_APPS`，settings 未加载瞬间的兜底）各存一份 —— 跨语言
    /// 编译器管不到，分叉只表现为两边短暂闪不同的 tab 集。这道闸把分叉变成
    /// `cargo test` 秒红（CLAUDE.md §三点六；hermes 曾真实分叉过）。
    #[test]
    fn default_visible_apps_match_the_frontend_copy() {
        let ts = include_str!("../../src/config/appConfig.tsx");
        let block_start = ts
            .find("export const DEFAULT_VISIBLE_APPS")
            .expect("appConfig.tsx 里应有 DEFAULT_VISIBLE_APPS");
        let block = &ts[block_start
            ..ts[block_start..]
                .find("\n};")
                .map(|end| block_start + end + 3)
                .unwrap_or(ts.len())];

        let defaults = VisibleApps::default();
        for (key, value) in [
            ("claude", defaults.claude),
            ("claude-desktop", defaults.claude_desktop),
            ("codex", defaults.codex),
            ("codex-image", defaults.codex_image),
            ("gemini", defaults.gemini),
            ("grokbuild", defaults.grokbuild),
            ("opencode", defaults.opencode),
            ("openclaw", defaults.openclaw),
            ("hermes", defaults.hermes),
            ("pi", defaults.pi),
        ] {
            // 键名与 serde 序列化一致；TS 里含连字符的键带引号（"claude-desktop"）
            let expected = if key.contains('-') {
                format!("\"{key}\": {value}")
            } else {
                format!("{key}: {value}")
            };
            assert!(
                block.contains(&expected),
                "DEFAULT_VISIBLE_APPS 的 `{key}` 与 Rust 侧 VisibleApps::default 不一致\n  \
                 Rust 侧: {value}\n  期望 TS 里出现: {expected}"
            );
        }
    }

    #[test]
    fn override_paths_expand_windows_style_tilde_separators() {
        let home = dirs::home_dir().expect("home directory");
        assert_eq!(
            resolve_override_path(r"~\pi\agent"),
            home.join("pi").join("agent")
        );
    }
}
