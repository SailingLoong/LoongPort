import type { RelayRow } from "@/lib/api/relay";

/** 判据只吃后端给出的状态，不在前端重新组合登录字段。 */
interface SessionState {
  status: RelayRow["status"];
}

/**
 * 删站点确认框的正文 key。
 *
 * 从没登录过的行没有登录态与余额，其他状态则按已配置账号处理。
 */
export function removeConfirmMessageKey(state: SessionState): string {
  return state.status !== "notLoggedIn"
    ? "loongport.row.removeConfirmMessage"
    : "loongport.row.removeConfirmMessageNeverLoggedIn";
}
