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

use futures::{
    future::{BoxFuture, Shared},
    FutureExt,
};
use std::sync::{Arc, OnceLock};

type SampleResult = Result<u64, Arc<tokio::task::JoinError>>;

/// Request-owned sampling: no tokenizer work until a usage consumer resolves it.
/// Clones share one blocking job and its result, including a failed job. Dropping
/// an unpolled sample releases its input; a started blocking job finishes normally.
#[derive(Clone)]
pub(crate) struct DeferredLocalTokens(Shared<BoxFuture<'static, SampleResult>>);

impl DeferredLocalTokens {
    pub(crate) fn new(body: &serde_json::Value) -> Self {
        // Keep the original JSON, before model mapping or protocol transforms.
        let body = body.clone();
        Self::from_count(move || count_request_tokens(&body))
    }

    fn from_count(count: impl FnOnce() -> u64 + Send + 'static) -> Self {
        Self(
            async move { tokio::task::spawn_blocking(count).await.map_err(Arc::new) }
                .boxed()
                .shared(),
        )
    }

    /// Already measured at a separate admission boundary (e.g. bounded native).
    pub(crate) fn ready(count: u64) -> Self {
        Self(futures::future::ready(Ok(count)).boxed().shared())
    }

    pub(crate) async fn resolve(&self) -> SampleResult {
        self.0.clone().await
    }

    pub(crate) async fn resolve_for_usage(&self) -> u64 {
        match self.resolve().await {
            Ok(count) => count,
            Err(error) => {
                // Do not discard upstream usage when optional local sampling fails.
                // Never format the panic payload: it could contain request content.
                log::warn!("[crowd-tokens] local sampling unavailable (cancelled={}, panicked={}); recording unsampled usage", error.is_cancelled(), error.is_panic());
                0 // Existing persisted convention: not sampled, NOT a measured zero.
            }
        }
    }
}

/// o200k 分词器进程级单例：初始化及编码是 CPU 工作；请求日志通过延迟采样
/// 在线程池解析，不让冷启动或特殊长文本阻塞代理运行时。
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
    #[cfg(test)]
    test_observation::record();
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

// Scoped to the current test thread; never installed in a production build.
#[cfg(test)]
pub(crate) mod test_observation {
    std::thread_local! {
        static CALLS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }
    pub(crate) struct CountScope;
    impl CountScope {
        pub(crate) fn start() -> Self {
            CALLS.with(|calls| {
                assert!(calls.replace(Some(0)).is_none());
            });
            Self
        }
        pub(crate) fn calls(&self) -> usize {
            CALLS.with(|calls| calls.get().expect("active count observation"))
        }
    }
    impl Drop for CountScope {
        fn drop(&mut self) {
            CALLS.with(|calls| calls.set(None));
        }
    }
    pub(super) fn record() {
        CALLS.with(|calls| {
            if let Some(count) = calls.get() {
                calls.set(Some(count + 1));
            }
        });
    }
}

#[cfg(test)]
#[path = "deferred_local_tokens_sampling_tests.rs"]
mod deferred_sampling_tests;
