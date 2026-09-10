//! 使用统计：配置的中转站与官方服务注册域、账号数量和随机安装标识。
//!
//! ⚠️ **准确的词是「假名化」不是「匿名」** —— 见下方那一节（review 纠正的用词）。
//!
//! 存在的理由：维护者需要知道用户实际在用哪几家中转站，据此决定优先适配谁。
//! 而这个事实**目前完全拿不到** —— 站点列表只存在每个用户本机的 SQLite 里。
//!
//! ## 上报什么、不报什么（这条边界是硬的）
//!
//! | 报 | 不报 |
//! |---|---|
//! | 站点 host（归一后：小写、去 `www.`、**去端口**） | 账号、邮箱、昵称 |
//! | 站点个数 | 余额、充值记录、用量 |
//! | app 版本、os（`macos` / `windows` / `linux`） | 密钥（sk / token 任何一种） |
//! | 一个**本模块专属**的随机 id（见下） | 档位、分组、切换记录 |
//!
//! ⚠️ **绝不复用 `creds` 的 `device_id`。** 那个 id 被写进中转站服务器上的 API key 名
//! （`provision::key_name_for` → `LoongPort/<device_id>/<platform>/<group_id>`）⇒
//! 任何看得到它的中转站都能把一条上报**对回一个具体的付费账号**。
//! 本模块自己生成一个只用于统计的随机 id，两者永不交叉。
//!
//! ## 为什么要那个 id（而不是完全无状态）
//!
//! 没有它就无法去重：一个用户开十次 app 会被算成十个用户，「站点个数分布」直接失真。
//! 它是**随机 UUID、不含任何设备指纹**（不取硬件序列号 / MAC / hostname），
//! 换台机器就是另一个 id。
//!
//! ## ⚠️ 准确的词是「假名化」而不是「匿名」（review 纠正）
//!
//! 我原来通篇写「匿名」，那是**过强的表述**。三个事实合起来足以做到
//! singling-out 与跨次关联：
//!
//! 1. **`install_id` 是稳定且持久的** —— 同一个安装的多次上报可以被串起来
//! 2. **稀有 host 的组合本身就是准标识符** —— 一个小众中转站可能全世界只有几个
//!    LoongPort 用户，「那个站 + 另外两个站」的组合很可能唯一
//! 3. **接收端看得到来源 IP**（我们控制不了这一层）
//!
//! ⇒ 它不是「无法关联到个人」的匿名数据，而是「不含直接身份字段、但可持续关联」的
//! **假名化**数据。用户看到的文案已按这条改（不再说「任何能认出你的标识」都不报，
//! 而是说清有一个随机安装标识）。
//!
//! ### 缓解措施与它们的代价（本轮没做，记在 TODO）
//!
//! reviewer 给了三条，都要牺牲数据质量，需要维护者按「他到底想答什么问题」权衡：
//!
//! - **稀有 host 归入 `other`**：需要一份「已知站点白名单」，而那正是我们想发现的东西
//!   （鸡生蛋）—— 除非只统计已知的几家
//! - **每个 host 单独发 `HMAC(local_secret, host)`**：去重键不可跨 host 关联，
//!   但那样**服务端看不到站点名**，只能数「有多少个不同的站」，答不了「用户在用哪几家」
//! - **站点数粗粒度分桶**（1 / 2-3 / 4+）：便宜且有效，不影响主要问题
//!
//! **接收端侧的义务同样重要**：不记 IP、设保留期。2026-09-09 接收端落地
//! （metrics Worker 的 `/v1/ping`）时一并兑现：IP 只以 SHA-256 进限流表
//! （保留 2 天），安装行 180 天未见活动即删 —— 见 `crowd-metrics/README`
//! 的隐私边界节。客户端这边做再多匿名化，服务端记了 IP 就全白费。
//!
//! ## Explicit sharing choice
//!
//! Fresh installations keep sharing off until onboarding completion or a settings
//! change. Existing settings retain their previous preferences. The persisted
//! switch is the sending gate; opening or dismissing onboarding never changes it.
//!
//! ## 失败静默，绝不影响任何用户流程
//!
//! 上报是**我们的需求**，不是用户要的功能。所以：不 await 在任何交互路径上、
//! 失败只记 log、不重试、不排队补发。拿不到这次就算了 —— 为它拖慢或打断
//! 用户的操作是本末倒置。

use serde::Serialize;

use crate::error::AppError;

/// 上报端点：metrics Worker（与实测共建同一个 Worker / D1，2026-09-09 切生产）。
///
/// `/v1/ping` 是**写入型**端点：只落 D1、无公开读出口，永不进任何公开快照。
/// 载荷校验、限流与保留期见 `crowd-metrics/src/ping.ts` 与该目录 README。
const ENDPOINT: &str = "https://metrics.loongport.dev/v1/ping";

/// 占位域名的标记（`.invalid` 是 RFC 2606 保留 TLD）。端点已切生产，
/// [`is_configured`] 恒为 true —— 保留这层判断是因为上报任务与首启告知
/// 弹窗共用同一个「配好了没」判据，将来若有意回退占位不需要改两处。
const UNCONFIGURED_MARKER: &str = ".invalid";

/// 端点配好了没。没配就整条链路 no-op。
pub fn is_configured() -> bool {
    !ENDPOINT.contains(UNCONFIGURED_MARKER)
}

/// 一次上报的载荷。**字段就这几个，加字段前先回到模块文档那张表。**
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// 本模块专属的随机安装 id（**不是** `creds` 的 `device_id`，见模块文档）。
    pub install_id: String,
    /// app 版本，用于分辨「老版本用户在用什么」。
    pub app_version: String,
    /// `macos` / `windows` / `linux`。**不带版本号** —— 精确的 OS 版本会缩小匿名集合。
    pub os: String,
    /// 用户添加的站点注册域（apex），**已归一、已排序、已去重**。
    pub site_hosts: Vec<String>,
    /// **服务账号行数**（包含官方服务）。保留旧字段名以兼容接收端。
    ///
    /// ⚠️ 名字曾叫 `relay_count` 且被弹窗文案说成「站点个数」——
    /// **那是不准的**（review 抓出）：同一个 host 挂三个账号时它是 3 而 `site_hosts`
    /// 是 1。它额外暴露了「这个用户在同一家开了几个号」这个事实。
    ///
    /// 保留它是因为「多账号使用率」本身有产品价值，但**名字与文案都要说实话**。
    pub relay_account_count: usize,
}

/// One origin per configured service account. No account identity or credentials
/// leave this projection; the receiver keeps its existing payload field names.
pub(crate) fn configured_service_origins(
    conn: &rusqlite::Connection,
) -> Result<Vec<String>, AppError> {
    let mut origins: Vec<String> = crate::relay::creds::list(conn)?
        .into_iter()
        .map(|row| row.site_origin)
        .collect();
    origins.extend(
        crate::vendor::creds::list(conn)?
            .into_iter()
            .filter_map(|row| {
                crate::vendor::Vendor::from_id(&row.vendor_id)
                    .map(|vendor| crate::vendor::builtin_login_url(vendor).to_string())
            }),
    );
    Ok(origins)
}

/// 由站点 origin 列表构造载荷。
///
/// `origins` 来自 configured_service_origins（含重复：同服务多账号）。
/// 归一走 [`super::identity::site_domain`]（注册域）—— 与 aff / 实测上传同一套身份，
/// 不然同一个站在两边会算成不同的东西；端口被一并去掉也有隐私理由：
/// 一个非标准端口（`:8443`）本身就是**近乎唯一的指纹**，会把匿名集合缩到很小。
pub fn build_report(install_id: String, app_version: String, origins: &[String]) -> Report {
    let relay_account_count = origins.len();

    let mut site_hosts: Vec<String> = origins
        .iter()
        .map(|o| super::identity::site_domain(o))
        .collect();
    // 排序 + 去重：**排序是为了不泄漏添加顺序**（那是行为信息，而且能当指纹用），
    // 去重是因为 host 集合答的是「用了哪几家」。
    site_hosts.sort_unstable();
    site_hosts.dedup();

    Report {
        install_id,
        app_version,
        os: current_os().to_string(),
        site_hosts,
        relay_account_count,
    }
}

/// 只报三大类，不带版本号 —— 精确 OS 版本会把匿名集合缩小。
fn current_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

/// 发一次上报。
///
/// **调用方不要 await 它在任何交互路径上**（见模块文档最后一节）。
/// 返回 `Err` 只用于日志，调用方应当忽略。
pub async fn send(report: &Report) -> Result<(), AppError> {
    if !is_configured() {
        // 端点还没配。**这不是错误** —— 静默返回，一个字节都不发。
        return Ok(());
    }

    let client = reqwest::Client::builder()
        // 超时短：它是后台动作，卡住毫无价值。
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Config(format!("构造统计客户端失败: {e}")))?;

    let resp = client
        .post(ENDPOINT)
        .json(report)
        .send()
        .await
        .map_err(|e| AppError::Config(format!("上报失败: {e}")))?;

    if !resp.status().is_success() {
        return Err(AppError::Config(format!("上报被拒: {}", resp.status())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_projection_counts_vendor_accounts_once_regardless_of_plans() {
        let db = crate::database::Database::memory().unwrap();
        let conn = db.conn.lock().unwrap();
        crate::relay::creds::save_site(
            &conn,
            "https://panel.example",
            "Example",
            "https://api.panel.example",
        )
        .unwrap();
        for (vendor, account) in [
            ("deepseek", "example-a"),
            ("deepseek", "example-b"),
            ("opencode", "example-c"),
            ("bigmodel", "example-d"),
        ] {
            conn.execute("INSERT INTO loongport_vendor (vendor_id, account_id, auth_token, account_label) VALUES (?1, ?2, 'secret-example', 'private-example')", rusqlite::params![vendor, account]).unwrap();
        }
        let origins = configured_service_origins(&conn).unwrap();
        let report = build_report("example-install".into(), "1.0".into(), &origins);
        assert_eq!(report.relay_account_count, 5);
        assert_eq!(
            report.site_hosts,
            vec![
                "bigmodel.cn",
                "deepseek.com",
                "opencode.ai",
                "panel.example"
            ]
        );
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("secret-example"));
        assert!(!json.contains("private-example"));
        assert!(!json.contains("example-a"));
    }

    #[test]
    fn endpoint_is_configured_to_production() {
        // ⭐ 2026-09-09 切生产：端点指向 metrics Worker 的 /v1/ping。原占位
        // 断言按它自己注释的指引翻转；「接收端还没建」那条技术债已同步销账。
        assert!(
            is_configured(),
            "端点又指回占位了？首启告知弹窗会跟着一起消失 —— 若是有意回退，\
             请把这条断言翻回去并在 TODO 重新立账"
        );
        assert!(
            !ENDPOINT.contains(UNCONFIGURED_MARKER),
            "生产端点不该含占位标记"
        );
    }

    #[test]
    fn report_never_carries_credentials_or_account_fields() {
        // ⭐ 本模块最重要的闸：**载荷里不能出现任何可认人/可认账号的东西**。
        //
        // 断言序列化后的 JSON 键，而不是「读一遍结构体觉得没问题」——
        // 将来有人给 `Report` 加字段时，这条会当场红。
        let json = serde_json::to_value(build_report(
            "install-abc".into(),
            "1.2.3".into(),
            &["https://panel.example".to_string()],
        ))
        .unwrap();

        // 排序后比较：这条闸管的是**键的集合**，不是声明顺序 ——
        // 调整字段顺序不该让它红（那是无害重构），加字段才该让它红。
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "appVersion",
                "installId",
                "os",
                "relayAccountCount",
                "siteHosts"
            ],
            "载荷的字段集合变了 —— 加字段前请回到模块文档那张「报/不报」的表"
        );

        // 反面：这些**账号身份/凭据**相关的词一个都不该出现在序列化结果里（含值）。
        //
        // ⚠️ 判据是「泄漏身份或凭据的东西」，不是「含某个英文单词」——
        // `relayAccountCount` 是个**计数**（不含任何身份），它合法。
        // 原来禁词表里有裸 `"account"`，把这个合法字段名也拦了 ⇒ 判据太粗。
        // 现在禁的是那些只可能来自身份/凭据的形态。
        let text = json.to_string();
        for forbidden in [
            "token",
            "sk-",
            "email",
            "@", // 邮箱的形状，比 "email" 这个词更难绕过
            "balance",
            "password",
            "refresh",
            "apikey",
            "api_key",
            "accountid", // 账号**标识**不许有；accountCount（计数）合法
            "account_id",
            "username",
            "nickname",
        ] {
            assert!(
                !text.to_ascii_lowercase().contains(forbidden),
                "载荷里出现了 {forbidden}：{text}"
            );
        }
    }

    #[test]
    fn hosts_are_normalized_the_same_way_as_the_aff_table() {
        // 两边用同一套身份归一，否则同一个站在「有没有邀请码」与「上报」里会算成两个东西。
        let r = build_report(
            "i".into(),
            "v".into(),
            &[
                "https://Panel.Example".to_string(),
                "https://www.panel.example:8443".to_string(),
                "https://api.panel.example".to_string(),
            ],
        );
        assert_eq!(
            r.site_hosts,
            vec!["panel.example"],
            "大小写 / www. / 端口 / api. 子域都该归一成同一个注册域"
        );
    }

    #[test]
    fn hosts_are_sorted_so_the_order_the_user_added_them_is_not_leaked() {
        // 添加顺序是行为信息，而且顺序本身能当指纹用（N 个站有 N! 种排列）。
        let r = build_report(
            "i".into(),
            "v".into(),
            &[
                "https://zzz.com".to_string(),
                "https://aaa.com".to_string(),
                "https://mmm.com".to_string(),
            ],
        );
        assert_eq!(r.site_hosts, vec!["aaa.com", "mmm.com", "zzz.com"]);
    }

    #[test]
    fn relay_count_counts_rows_while_hosts_are_deduped() {
        // 同一个站挂两个账号：**个数是 2**（用户视角「我加了两个」），
        // 而 host 集合是 1（「我用了一家」）。两个数答的是不同问题，别合并。
        let r = build_report(
            "i".into(),
            "v".into(),
            &[
                "https://panel.example".to_string(),
                "https://panel.example".to_string(),
                "https://other.example".to_string(),
            ],
        );
        assert_eq!(r.relay_account_count, 3, "个数是行数");
        assert_eq!(r.site_hosts.len(), 2, "host 集合去重");
    }

    #[test]
    fn a_user_with_no_sites_still_produces_a_valid_report() {
        // 装了但还没加站的用户也是有效样本（「多少人装了却没用起来」是个真问题）。
        let r = build_report("i".into(), "v".into(), &[]);
        assert_eq!(r.relay_account_count, 0);
        assert!(r.site_hosts.is_empty());
    }

    #[test]
    fn os_carries_no_version_number() {
        // 精确 OS 版本会把匿名集合缩小（「macOS 15.3.1 + 这三个站」可能就唯一了）。
        let os = current_os();
        assert!(
            !os.chars().any(|c| c.is_ascii_digit()),
            "os 不该带版本号: {os}"
        );
        assert!(["macos", "windows", "linux", "other"].contains(&os));
    }
}
