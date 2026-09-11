//! 请求体本地 token 计数（crowd 对账的采集原语）。
//!
//! 用途：与上游响应 `usage.input_tokens` 对账，众测「同站同模型的计数口径
//! 比值是否跳变」（站点换模型家族时，远端口径变、本地口径不变 ⇒ 比值跳）。
//! **判据不在客户端**：这里只产出事实（一个 u64），聚合与判定在服务端
//! （宁缺毋认：样本不足就不出结论，见 crowd 模块文档）。
//!
//! 口径取舍：**对整个请求体 JSON 的紧凑序列化计数**，不解析 messages 结构。
//! 判据只依赖「同一把尺子量所有请求」的口径恒定，不依赖与上游计费口径
//! 对齐——JSON 结构开销（键名、转义）在同 app 同模型下是常数偏移，不影响
//! 比值稳定性判据；解析 messages 反而引入各家族格式分支。

use std::sync::OnceLock;

/// o200k 分词器进程级单例：首次构造要加载 BPE 表（几十毫秒），
/// 之后每次调用纯 CPU（百 KB 文本毫秒级），代理热路径可承受。
/// 存 `Option`：初始化失败时固化「停用」，失败行返回 0（未采语义）。
static TOKENIZER: OnceLock<Option<tiktoken_rs::CoreBPE>> = OnceLock::new();

fn tokenizer() -> Option<&'static tiktoken_rs::CoreBPE> {
    TOKENIZER
        .get_or_init(|| {
            tiktoken_rs::o200k_base()
                .map_err(|e| {
                    log::warn!("[crowd-tokens] o200k 分词器初始化失败，本地计数停用: {e}");
                    e
                })
                .ok()
        })
        .as_ref()
}

/// 数请求体的本地 token 数。任何失败（序列化/分词器不可用/文本非法）返回 0
/// —— 0 语义即「未采」，桶 SQL 的配对条件以 `> 0` 收口，失败行天然出局，
/// 不会以假数据参与对账。
pub fn count_request_tokens(body: &serde_json::Value) -> u64 {
    let Some(bpe) = tokenizer() else {
        return 0;
    };
    // `&Value` 的序列化对合法 JSON 不失败；失败返回 0（未采语义）。
    let Ok(compact) = serde_json::to_string(body) else {
        return 0;
    };
    bpe.encode_ordinary(&compact).len() as u64
}

#[cfg(test)]
mod tests {
    use super::count_request_tokens;
    use serde_json::json;

    #[test]
    fn counts_grow_with_input() {
        let small = count_request_tokens(&json!({"model": "gpt-6", "input": "hi"}));
        let large = count_request_tokens(&json!({
            "model": "gpt-6",
            "input": "这是一段长得多的请求内容，token 数必须显著大于短请求。".repeat(50)
        }));
        assert!(small > 0, "合法请求必须产出正计数");
        assert!(large > small * 10, "长文本计数应显著大于短文本");
    }

    #[test]
    fn same_body_is_deterministic() {
        // 口径恒定是对账判据的前提：同一 body 两次计数必须一致。
        let body = json!({"model": "claude-5", "messages": [{"role": "user", "content": "ping"}]});
        assert_eq!(count_request_tokens(&body), count_request_tokens(&body));
    }
}
