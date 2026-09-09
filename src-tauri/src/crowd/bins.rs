//! TTFT 直方图分桶边界 —— 与 Worker（`crowd-metrics/src/bins.ts`）共享的常量。
//!
//! 服务端按**位置**求和各来源的桶计数，两边边界不一致时求和结果是垃圾，
//! 所以这里有一条解析 TS 文件比对的闸测试（先例：maintenance 对 constants.ts
//! 的跨语言检查）。改任何一边，闸会红，必须两边一起改。

/// 桶上边界（ms）。桶 i 覆盖 `[lo_i, hi_i)`：`lo_0 = 0`，`lo_i = EDGES[i-1]`，
/// `hi_i = EDGES[i]`；最后一个桶是 `[9600, ∞)` 溢出桶。
pub const TTFT_BIN_EDGES_MS: &[i64] =
    &[200, 400, 600, 800, 1200, 1600, 2400, 3200, 4800, 6400, 9600];

/// 桶数 = 边界数 + 1（含溢出桶）。上传载荷的 `ttftBins` 长度必须等于它。
pub const TTFT_BIN_COUNT: usize = TTFT_BIN_EDGES_MS.len() + 1;

/// 生成 SQL 里的逐桶计数表达式（逗号分隔），分桶条件由同一份边界生成 ——
/// SQL 与 Rust 常量天然同源，不存在「SQL 里手写一份边界」的分叉。
///
/// 资格限定 `data_source = 'proxy'`：直方图只收本地代理亲历计时的行
/// （session 回填行现在 `first_token_ms` 恒缺省、天然不进桶；资格条件把口径
/// 钉死 —— 将来 session 侧若有延迟数据，先过口径审视再放开，别静默混桶）。
pub(crate) fn ttft_bin_sum_exprs(alias: &str) -> String {
    let mut exprs = Vec::with_capacity(TTFT_BIN_COUNT);
    for i in 0..TTFT_BIN_COUNT {
        let lo = if i == 0 { 0 } else { TTFT_BIN_EDGES_MS[i - 1] };
        let cond = if i < TTFT_BIN_EDGES_MS.len() {
            format!(
                "{alias}.first_token_ms >= {lo} AND {alias}.first_token_ms < {}",
                TTFT_BIN_EDGES_MS[i]
            )
        } else {
            format!("{alias}.first_token_ms >= {lo}")
        };
        exprs.push(format!(
            "SUM(CASE WHEN {alias}.data_source = 'proxy' AND {cond} THEN 1 ELSE 0 END)"
        ));
    }
    exprs.join(", ")
}

/// 输出速度（tokens/秒）分桶上边界 —— 与 Worker 共享的第二组跨语言常量
/// （P4 模型维度）。行值 = output_tokens / max((latency - first_token)/1s, 0.1s)，
/// 只统计 output_tokens > 0 的行（0 输出的行没有速度语义）。
pub const TPS_BIN_EDGES: &[i64] = &[5, 10, 20, 40, 60, 80, 120, 160, 240, 320, 480];

/// TPS 桶数 = 边界数 + 1（含溢出桶）。上传载荷的 `tpsBins` 长度必须等于它。
pub const TPS_BIN_COUNT: usize = TPS_BIN_EDGES.len() + 1;

/// 逐桶计数表达式（与 `ttft_bin_sum_exprs` 同构）。行速度表达式在 SQL 里
/// 内联生成（SQLite 无变量复用，重复求值无碍聚合正确性）。
///
/// 资格限定 `data_source = 'proxy'`：桶自 2026-09 起收全部用量行（含 session
/// 回填），而 session 行 `latency_ms` 恒 0 —— 0/100ms 兜底会把任何输出算成
/// 10 倍 tok/s 的病态高速，灌进溢出桶。速度只在转发路径被真实计时。
pub(crate) fn tps_bin_sum_exprs(alias: &str) -> String {
    let row_tps = format!(
        "CAST({alias}.output_tokens AS REAL) * 1000.0 / MAX({alias}.latency_ms - COALESCE({alias}.first_token_ms, 0), 100)"
    );
    let mut exprs = Vec::with_capacity(TPS_BIN_COUNT);
    for i in 0..TPS_BIN_COUNT {
        let lo = if i == 0 { 0 } else { TPS_BIN_EDGES[i - 1] };
        let cond = if i < TPS_BIN_EDGES.len() {
            format!("{row_tps} >= {lo} AND {row_tps} < {}", TPS_BIN_EDGES[i])
        } else {
            format!("{row_tps} >= {lo}")
        };
        exprs.push(format!(
            "SUM(CASE WHEN {alias}.data_source = 'proxy' AND {alias}.output_tokens > 0 AND ({cond}) THEN 1 ELSE 0 END)"
        ));
    }
    exprs.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 值落进哪个桶（0 基）。运行期分桶在 SQL 里（`ttft_bin_sum_exprs` 生成的
    /// 条件与这里同源），这个函数是给测试钉语义的规格镜像。
    fn bin_index(ms: i64) -> usize {
        TTFT_BIN_EDGES_MS.iter().filter(|edge| **edge <= ms).count()
    }

    #[test]
    fn bin_index_follows_the_edges() {
        assert_eq!(bin_index(0), 0);
        assert_eq!(bin_index(199), 0);
        assert_eq!(bin_index(200), 1);
        assert_eq!(bin_index(399), 1);
        assert_eq!(bin_index(400), 2);
        assert_eq!(bin_index(9599), TTFT_BIN_COUNT - 2);
        assert_eq!(bin_index(9600), TTFT_BIN_COUNT - 1);
        assert_eq!(bin_index(60_000), TTFT_BIN_COUNT - 1);
    }

    #[test]
    fn sql_bin_exprs_count_matches_bin_count_and_cover_all_values() {
        let exprs = ttft_bin_sum_exprs("l");
        let sum_count = exprs.matches("SUM(CASE WHEN").count();
        assert_eq!(sum_count, TTFT_BIN_COUNT);
        // 首桶下界是 0（全值覆盖），溢出桶只有下界。
        assert!(exprs.contains("l.first_token_ms >= 0 AND l.first_token_ms < 200"));
        assert!(exprs.contains("l.first_token_ms >= 6400 AND l.first_token_ms < 9600"));
        assert!(exprs.contains("l.first_token_ms >= 9600"));
        assert_eq!(
            exprs.matches("<").count(),
            TTFT_BIN_EDGES_MS.len(),
            "有上界的桶数应等于边界数（其余是溢出桶）"
        );
    }

    /// 从 bins.ts 里按常量名提取数组字面量（用 "= [" 定位 —— 类型标注
    /// `number[]` 里也有方括号，直接找第一个 '[' 会把类型标注误当数组体）。
    fn ts_edges(ts: &str, name: &str) -> Vec<i64> {
        let start = ts
            .find(name)
            .unwrap_or_else(|| panic!("bins.ts 里应有 {name} 常量"));
        let assign_offset = ts[start..]
            .find("= [")
            .unwrap_or_else(|| panic!("{name} 应有数组字面量初始化"));
        let array_start = start + assign_offset + 2; // "= [" 的 '[' 本身
        let bracket_end = ts[array_start..]
            .find("];")
            .unwrap_or_else(|| panic!("{name} 数组应闭合"));
        ts[array_start + 1..array_start + bracket_end]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<i64>()
                    .unwrap_or_else(|_| panic!("bins.ts 里出现非整数边界: {s}"))
            })
            .collect()
    }

    /// ⭐ 跨语言一致性闸：Rust 侧边界必须与 Worker（crowd-metrics/src/bins.ts）
    /// 完全一致 —— 服务端按位置求和，边界分叉 = 数据垃圾。
    #[test]
    fn ttft_edges_match_the_worker_typescript_constant() {
        let ts = include_str!("../../../crowd-metrics/src/bins.ts");

        let ts_edges = ts_edges(ts, "TTFT_BIN_EDGES_MS");
        assert_eq!(
            ts_edges, TTFT_BIN_EDGES_MS,
            "Rust 与 Worker 的 TTFT 桶边界不一致 —— 两边必须一起改"
        );

        // 桶数公式也必须同构（两侧都是 边界数 + 1）。
        assert!(
            ts.contains("TTFT_BIN_COUNT = TTFT_BIN_EDGES_MS.length + 1"),
            "bins.ts 的 TTFT_BIN_COUNT 公式变了 —— 检查两侧桶数定义是否仍同构"
        );
    }

    /// ⭐ TPS 边界的同款跨语言闸（P4）。
    #[test]
    fn tps_edges_match_the_worker_typescript_constant() {
        let ts = include_str!("../../../crowd-metrics/src/bins.ts");

        let ts_edges = ts_edges(ts, "TPS_BIN_EDGES");
        assert_eq!(
            ts_edges, TPS_BIN_EDGES,
            "Rust 与 Worker 的 TPS 桶边界不一致 —— 两边必须一起改"
        );
        assert!(
            ts.contains("TPS_BIN_COUNT = TPS_BIN_EDGES.length + 1"),
            "bins.ts 的 TPS_BIN_COUNT 公式变了 —— 检查两侧桶数定义是否仍同构"
        );
    }
}
