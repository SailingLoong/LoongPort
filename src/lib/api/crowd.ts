import { invoke } from "@tauri-apps/api/core";

/**
 * 站点实测共建的公共快照（与后端 `crowd::snapshot` 的 DTO 同形）。
 *
 * `sites` 的键是归一化站点 host —— 与广场行的 `siteHost` 同一套归一，
 * 前端直接按 host join。k-匿名的口径在服务端：没过门槛的站/窗整个缺席。
 */
export interface CrowdWindowStats {
  samples: number;
  /** 独立来源数（≈ 贡献用户数）。 */
  sources: number;
  ttftP50Ms: number | null;
  ttftP95Ms: number | null;
  errRate: number | null;
  cacheHitRate: number | null;
  /** 花费参考值（$/百万 token，模型混合会拉偏，展示必须带「参考」语义）。 */
  costUsdPerMTok: number | null;
}

export interface CrowdHourSlot {
  p50Ms: number | null;
  samples: number;
}

export interface CrowdSiteStats {
  w24: CrowdWindowStats | null;
  w7: CrowdWindowStats | null;
  hours: CrowdHourSlot[];
}

export interface CrowdSnapshot {
  version: number;
  generatedAt: number;
  sites: Record<string, CrowdSiteStats>;
}

export const crowdApi = {
  /**
   * 读快照。**对等门禁在命令层**：共建关闭时后端返回 `null`（连拉取都不发生），
   * 前端拿 `null` 渲染锁定态 —— 别在前端再养一份「是否参与」的判断。
   */
  getSnapshot: () => invoke<CrowdSnapshot | null>("crowd_get_snapshot"),
};
