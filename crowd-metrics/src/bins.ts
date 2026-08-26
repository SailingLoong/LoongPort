/**
 * TTFT（首字耗时）直方图分桶边界，单位毫秒。
 *
 * ⚠️ 跨语言共享常量：客户端（src-tauri/src/crowd/bins.rs）按同一组边界分桶，
 * 服务端按位置求和 —— **两边边界不一致时求和结果是垃圾**。
 * Rust 侧有一条一致性闸测试解析本文件比对（先例：maintenance 模块对
 * constants.ts 的跨语言检查），改这里必须同步改那边。
 *
 * 桶 i（0 基）覆盖 [lo_i, hi_i)：
 *   lo_0 = 0；lo_i = EDGES[i-1]；hi_i = EDGES[i]；
 *   最后一个桶是溢出桶 [9600, ∞)。
 * 分位数值在桶内做线性插值（直方图求分位的标准近似）。
 */
export const TTFT_BIN_EDGES_MS: readonly number[] = [
  200, 400, 600, 800, 1200, 1600, 2400, 3200, 4800, 6400, 9600,
];

/** 桶数 = 边界数 + 1（含溢出桶）。 */
export const TTFT_BIN_COUNT = TTFT_BIN_EDGES_MS.length + 1;

/** 溢出桶的上界外推值（[9600, ∞) 无法插值，按 1.5×末边外推）。 */
const OVERFLOW_HI_MS = TTFT_BIN_EDGES_MS[TTFT_BIN_EDGES_MS.length - 1] * 1.5;

/** 桶 i 的 [lo, hi) 边界。 */
export function binBounds(i: number): { lo: number; hi: number } {
  const lo = i === 0 ? 0 : TTFT_BIN_EDGES_MS[i - 1];
  const hi =
    i < TTFT_BIN_EDGES_MS.length
      ? TTFT_BIN_EDGES_MS[i]
      : OVERFLOW_HI_MS;
  return { lo, hi };
}

/** 桶 i 的代表值（中点），用于按源求均值做极端源裁剪。 */
export function binMidpoint(i: number): number {
  const { lo, hi } = binBounds(i);
  return (lo + hi) / 2;
}

/**
 * 从直方图计数求分位数（0 ≤ q ≤ 1）。空直方图返回 null。
 *
 * rank = q * (total - 1)：落到哪个桶就在该桶 [lo, hi) 内按占比线性插值。
 * 这是聚合展示用的近似，不是精确分位 —— 足够「这个站快不快」的用途。
 */
export function quantileFromBins(
  bins: readonly number[],
  q: number,
): number | null {
  const total = bins.reduce((a, b) => a + b, 0);
  if (total === 0) return null;

  const rank = q * (total - 1);
  let cumulative = 0;
  for (let i = 0; i < bins.length; i++) {
    const count = bins[i];
    if (count === 0) continue;
    if (rank < cumulative + count) {
      const { lo, hi } = binBounds(i);
      const offset = (rank - cumulative) / count;
      return lo + offset * (hi - lo);
    }
    cumulative += count;
  }
  // 数学上到不了这里（rank ≤ total-1 落在最后一个非空桶）；兜底返回末桶上界。
  return OVERFLOW_HI_MS;
}
