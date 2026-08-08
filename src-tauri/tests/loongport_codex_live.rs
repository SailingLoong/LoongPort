//! LoongPort 切换分组后落到 `~/.codex/` 的东西，必须是 codex 真能用的形态。
//!
//! 这个文件守的是整条链路里最容易静默走错的一段：**config.toml 的内容对不对**。
//! 单元测试只验字符串包含关系，这里验的是「写文件」这个动作的真实产物 —— 包括
//! `auth.json` 有没有被动过（那是 ChatGPT 桌面版的登录凭据）。
//!
//! ## 为什么这段值得一个集成测试
//!
//! `codex doctor` 实测过三组对照，只有一个组合是错的，而它恰好是「照抄上游预设」的结果：
//!
//! | config.toml | reachability mode | 实际打到哪 |
//! |---|---|---|
//! | `requires_openai_auth = true` + bearer token | **ChatGPT auth** | chatgpt.com（403，1 fail） |
//! | 无 `requires_openai_auth` + bearer token | provider auth | 中转站 `/v1`（200，0 fail） |
//! | `requires_openai_auth = true` + auth.json 有 key | API key auth | 中转站 `/v1`（200，0 fail） |
//!
//! LoongPort 走第二行。第一行是「沿用上游模板 + 开 preserve 开关」会得到的东西 —— 它跑不通，
//! 但错误现场在 codex 那边（credentials incomplete），从 LoongPort 这边完全看不出来。

use cc_switch_lib::{
    get_codex_auth_path, get_codex_config_path, write_codex_live_atomic, AppType, Provider,
    ProviderMeta, ProviderService,
};

#[path = "support.rs"]
mod support;
use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};

/// 复刻 `relay::provision::codex_config_toml` 的产物形态。
///
/// 有意不直接调那个函数：这里要断言的是「落到磁盘上的 config.toml 长什么样」，
/// 复刻一份输入能让测试在有人改了生成器时**照样跑**，然后由断言指出行为变了。
fn loongport_provider_settings(api_key: &str) -> serde_json::Value {
    serde_json::json!({
        "auth": { "OPENAI_API_KEY": api_key },
        "config": "model_provider = \"custom\"\n\
                   model = \"gpt-5.5\"\n\
                   model_reasoning_effort = \"high\"\n\
                   disable_response_storage = true\n\
                   \n\
                   [model_providers.custom]\n\
                   name = \"BestApi · Pro\"\n\
                   base_url = \"https://bestapi.store/v1\"\n\
                   wire_api = \"responses\"",
    })
}

#[test]
fn generated_config_is_a_shape_codex_accepts() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();

    let settings = loongport_provider_settings("sk-loongport-test");
    let config_text = settings["config"].as_str().expect("config 应是字符串");

    // 1) 必须是合法 TOML —— 语法错的话 codex 启动时才报，而那时用户只看到「用不了」。
    let parsed: toml::Value = config_text.parse().expect("生成的 config.toml 必须可解析");

    // 2) model_provider 必须是 custom：它是会话历史的桶标识，也是 bearer token 能落进
    //    provider 作用域的前提（`openai` 在 codex 的保留 id 里）。
    assert_eq!(
        parsed["model_provider"].as_str(),
        Some("custom"),
        "model_provider 必须是 custom"
    );
    assert!(
        parsed["model_providers"].get("custom").is_some(),
        "必须有 [model_providers.custom] 表"
    );

    // 3) 绝不能声明 requires_openai_auth。见文件头那张表：它 + 不写 auth.json 是唯一
    //    跑不通的组合（codex 会去打 chatgpt.com 而不是中转站）。
    assert!(
        parsed["model_providers"]["custom"]
            .get("requires_openai_auth")
            .is_none(),
        "声明 requires_openai_auth 会让 codex 走 ChatGPT auth 模式去打 chatgpt.com"
    );

    // 4) 两个会让请求直接被服务端拒掉的键。
    assert_eq!(
        parsed["disable_response_storage"].as_bool(),
        Some(true),
        "漏掉它 codex 会发 previous_response_id，sub2api 的 HTTP 路径直接 400"
    );
    assert_eq!(
        parsed["model_providers"]["custom"]["wire_api"].as_str(),
        Some("responses"),
        "sub2api 的 openai 网关原生走 responses"
    );

    // 5) base_url 必须带 /v1：中转站后台的 api_base_url 可能是空串，补 /v1 的责任在客户端。
    let base_url = parsed["model_providers"]["custom"]["base_url"]
        .as_str()
        .expect("base_url 应存在");
    assert!(
        base_url.ends_with("/v1"),
        "base_url 必须以 /v1 结尾，实际: {base_url}"
    );
}

#[test]
fn writing_live_config_only_leaves_chatgpt_login_untouched() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();

    // 先造一份「用户已经用 ChatGPT 账号登录过」的 auth.json —— 这是 ChatGPT 桌面版与命令行
    // codex 共用的那份凭据（它们共用同一个 ~/.codex）。
    let existing_login = serde_json::json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": serde_json::Value::Null,
        "tokens": {
            "access_token": "chatgpt-access",
            "refresh_token": "chatgpt-refresh",
            "account_id": "acct-1",
        },
    });
    write_codex_live_atomic(&existing_login, None).expect("seed 已有的 ChatGPT 登录");

    let auth_path = get_codex_auth_path();
    let before = std::fs::read_to_string(&auth_path).expect("读 seed 后的 auth.json");

    // 现在走 LoongPort 的落盘路径：只写 config.toml。
    let settings = loongport_provider_settings("sk-loongport-test");
    let config_text = settings["config"].as_str().unwrap();
    let live_config =
        cc_switch_lib::prepare_codex_provider_live_config(&settings["auth"], config_text)
            .expect("准备 live config");
    cc_switch_lib::write_codex_live_config_atomic(Some(&live_config)).expect("写 config.toml");

    // auth.json 必须**逐字节没变** —— 变了就意味着用户的 ChatGPT 登录被打掉了，
    // 重开桌面版会要求重新登录。
    let after = std::fs::read_to_string(&auth_path).expect("读写入后的 auth.json");
    assert_eq!(
        before, after,
        "切换分组不得改动 auth.json（那是 ChatGPT 桌面版的登录凭据）"
    );

    // 而 sk 应该落在 config.toml 的 provider 作用域里。
    let written = std::fs::read_to_string(get_codex_config_path()).expect("读 config.toml");
    let parsed: toml::Value = written.parse().expect("写出的 config.toml 必须可解析");
    assert_eq!(
        parsed["model_providers"]["custom"]["experimental_bearer_token"].as_str(),
        Some("sk-loongport-test"),
        "sk 必须落在 [model_providers.custom] 下，而不是顶层"
    );
    // 顶层不该有 token —— 那是 model_provider 撞上 codex 保留 id 时才会发生的事。
    assert!(
        parsed.get("experimental_bearer_token").is_none(),
        "sk 不该落到顶层: {written}"
    );
}

/// 整条落地链路：像 `relay_provision` 那样写一条 provider，然后真的切上去，
/// 断言 `~/.codex/config.toml` 里出现的正是 codex 能用的形态。
///
/// 这是唯一覆盖「provision 写库 → ProviderService::switch → 落盘」全程的测试。前面两条
/// 分别只验「生成的内容对不对」与「auth.json 有没有被动」，都不经过 switch 那条链。
#[test]
fn provisioned_provider_switches_and_lands_correct_config() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();

    let state = create_test_state().expect("create test state");

    // 复刻 relay_provision 写库的那条记录（含它那两个容易漏的字段）。
    let provider_id = "loongport-test0000000001";
    let provider = Provider {
        id: provider_id.to_string(),
        name: "BestApi · Pro 混池".to_string(),
        settings_config: loongport_provider_settings("sk-provisioned"),
        website_url: Some("https://bestapi.store".to_string()),
        // aggregator 而不是 official —— official 会触发一批只对官方订阅成立的逻辑。
        category: Some("aggregator".to_string()),
        created_at: Some(1_800_000_000_000),
        sort_index: Some(0),
        notes: None,
        meta: Some(ProviderMeta {
            // 不写它会落到 ProxyChat profile —— 那是唯一会 spawn codex 子进程的分支。
            api_format: Some("openai_responses".to_string()),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    state
        .db
        .save_provider(AppType::Codex.as_str(), &provider)
        .expect("save provider");

    // 切上去 —— 走的是 cc-switch 既有链路，不是我们自己写文件。
    ProviderService::switch(&state, AppType::Codex, provider_id).expect("switch should succeed");

    // 落盘的 config.toml 必须是 codex 接受的形态。
    let written = std::fs::read_to_string(get_codex_config_path()).expect("读 config.toml");
    let parsed: toml::Value = written.parse().expect("落盘的 config.toml 必须可解析");

    assert_eq!(
        parsed["model_provider"].as_str(),
        Some("custom"),
        "落盘后 model_provider 仍必须是 custom（会话历史的桶标识）"
    );
    assert!(
        parsed["model_providers"]["custom"]
            .get("requires_openai_auth")
            .is_none(),
        "落盘后不得出现 requires_openai_auth，否则 codex 会去打 chatgpt.com: {written}"
    );
    assert_eq!(
        parsed["model_providers"]["custom"]["experimental_bearer_token"].as_str(),
        Some("sk-provisioned"),
        "sk 必须落进 provider 作用域"
    );
    assert_eq!(
        parsed["model_providers"]["custom"]["base_url"].as_str(),
        Some("https://bestapi.store/v1")
    );
    assert_eq!(parsed["disable_response_storage"].as_bool(), Some(true));

    // current 也要真的指向它 —— 否则 UI 上显示切了、codex 用的还是别的。
    assert_eq!(
        ProviderService::current(&state, AppType::Codex).expect("read current"),
        provider_id
    );
}

/// ⭐ **刷新当前档位的密钥后，落地文件必须跟着变** —— 只写 DB 是不够的。
///
/// ## 这条测试守的缺陷（review 抓出）
///
/// `relay_provision` 与 `relay_reset_tier_config` 原来都只 `db.save_provider`。
/// 而 CLI 读的是 `~/.codex/config.toml`，不是我们的 DB ⇒ 服务端那把 sk 被撤销、
/// provision 重建了一把之后：**界面提示刷新成功、库里确实是新密钥，codex 仍拿旧的去请求**。
///
/// 更糟的是用户没有自救手段：这个档位已经是当前项（`isCurrent` 为 true），
/// 前端 `if (tier.isCurrent) return;` 会让「再点它一次」什么也不做。
///
/// 所以判据必须落在**磁盘内容**上，不能只断言 DB —— 断言 DB 的测试对这个缺陷是绿的。
#[test]
fn refreshing_the_current_tiers_key_updates_the_live_config() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();

    let state = create_test_state().expect("create test state");
    let provider_id = "loongport-test0000000002";

    let tier = |api_key: &str| Provider {
        id: provider_id.to_string(),
        name: "BestApi · Pro 混池".to_string(),
        settings_config: loongport_provider_settings(api_key),
        website_url: Some("https://bestapi.store".to_string()),
        category: Some("aggregator".to_string()),
        created_at: Some(1_800_000_000_000),
        sort_index: Some(0),
        notes: None,
        meta: Some(ProviderMeta {
            api_format: Some("openai_responses".to_string()),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };

    // 先把它切成当前项 —— 这是缺陷成立的前提（非当前项本来就不该碰 live 文件）。
    state
        .db
        .save_provider(AppType::Codex.as_str(), &tier("sk-old"))
        .expect("save provider");
    ProviderService::switch(&state, AppType::Codex, provider_id).expect("switch");
    assert!(
        std::fs::read_to_string(get_codex_config_path())
            .expect("读 config.toml")
            .contains("sk-old"),
        "前提没成立：切换后 live 里本该是旧 sk"
    );

    // 模拟 provision 重建密钥那一步：换 sk 写库。**只写 DB，正是缺陷现场。**
    state
        .db
        .save_provider(AppType::Codex.as_str(), &tier("sk-rebuilt"))
        .expect("save rebuilt provider");

    // ⚠️ 这里调的是**服务层**那一步，而命令层（`do_provision` / `reset_tier_config_impl`）
    // 有没有真的调它，由 `commands::relay` 里那条
    // `refresh_live_for_current_tiers_is_wired_into_both_commands` 钉住 ——
    // 那两个函数都吃 `&tauri::AppHandle`（集成测试里也造不出来），所以命令层那一步
    // 只能靠「源码里那两处调用还在吗」来守。两条测试合起来才覆盖完整链路。
    //
    // （第二路 review 实测过：只有这条测试时，把命令层那两段注释掉全绿。）
    ProviderService::sync_current_provider_for_app(&state, AppType::Codex)
        .expect("同步当前项的落地配置不该失败");

    let written = std::fs::read_to_string(get_codex_config_path()).expect("读 config.toml");
    let parsed: toml::Value = written.parse().expect("落盘的 config.toml 必须可解析");
    assert_eq!(
        parsed["model_providers"]["custom"]["experimental_bearer_token"].as_str(),
        Some("sk-rebuilt"),
        "⭐ live 文件里必须是**新** sk —— 只写 DB 的话 codex 会一直拿旧密钥打 401，\
         而用户点不动那个档位（UI 认为它已是当前项）。实际落盘内容：{written}"
    );
    assert!(
        !written.contains("sk-old"),
        "旧 sk 必须彻底消失，不能与新的并存：{written}"
    );
}
