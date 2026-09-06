/**
 * 字节数的展示格式化（`1.2 KB` / `3.4 MB`）。原先三处各写一份（备份列表、
 * 工作区内存面板、生图画廊），KB 精度已经分叉 —— 收成一份，美元/整数同理走
 * `components/usage/format` 的 fmtUsd/fmtInt，别再手拼单位串。
 */
export function fmtBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
