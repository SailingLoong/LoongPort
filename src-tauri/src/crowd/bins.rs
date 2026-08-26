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
        exprs.push(format!("SUM(CASE WHEN {cond} THEN 1 ELSE 0 END)"));
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

    /// ⭐ 跨语言一致性闸：Rust 侧边界必须与 Worker（crowd-metrics/src/bins.ts）
    /// 完全一致 —— 服务端按位置求和，边界分叉 = 数据垃圾。
    #[test]
    fn ttft_edges_match_the_worker_typescript_constant() {
        let ts = include_str!("../../../crowd-metrics/src/bins.ts");

        let start = ts
            .find("TTFT_BIN_EDGES_MS")
            .expect("bins.ts 里应有 TTFT_BIN_EDGES_MS 常量");
        // 用 "= [" 定位数组字面量 —— 类型标注 `number[]` 里也有方括号，
        // 直接找第一个 '[' 会把类型标注误当数组体。
        let assign_offset = ts[start..]
            .find("= [")
            .expect("TTFT_BIN_EDGES_MS 应有数组字面量初始化");
        let array_start = start + assign_offset + 2; // "= [" 的 '[' 本身
        let bracket_end = ts[array_start..]
            .find("];")
            .expect("TTFT_BIN_EDGES_MS 数组应闭合");
        let body = &ts[array_start + 1..array_start + bracket_end];

        let ts_edges: Vec<i64> = body
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<i64>()
                    .unwrap_or_else(|_| panic!("bins.ts 里出现非整数边界: {s}"))
            })
            .collect();

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
}
