import { invoke } from "@tauri-apps/api/core";

/** 远端公告（数据来自签名远端配置，见 `src-tauri/src/commands/announcements.rs`）。 */
export interface Announcement {
  id: string;
  /** 展示类型；当前只有 "dialog"。 */
  type: string;
  title: string;
  /** 正文，支持换行。 */
  body: string;
}

export const announcementsApi = {
  /** 待展示的公告（已确认的在后端过滤掉）。空数组 = 今天没有公告。 */
  getPending: (): Promise<Announcement[]> =>
    invoke("get_pending_announcements"),
  /** 确认公告：任何关闭方式（点确认、Esc、点遮罩）都应调用。 */
  acknowledge: (id: string): Promise<void> =>
    invoke("acknowledge_announcement", { id }),
};
