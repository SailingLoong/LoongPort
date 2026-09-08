import { invoke } from "@tauri-apps/api/core";

/** 全局重置预告（社区数据源 codex-reset.com 的公开 feed）。所有字段可缺省。 */
export interface CodexResetFeed {
  /** 上次已验证重置（epoch 秒）+ 原帖链接 */
  lastVerifiedAt: number | null;
  lastVerifiedUrl: string | null;
  /** 最新一条重置公告（epoch 秒 + 原帖链接 + 原文摘要） */
  announcementAt: number | null;
  announcementUrl: string | null;
  announcementSummary: string | null;
  /** 公告文本窄解析出的预计落地时间（epoch 秒）；解析不出为 null */
  landingAt: number | null;
  fetchedAt: number;
}

export const codexResetApi = {
  getFeed: (): Promise<CodexResetFeed> => invoke("get_codex_reset_feed"),
};
