/**
 * 删站点确认框该用哪一句文案。
 *
 * ## 为什么是个独立模块
 *
 * 判据本身是一条领域规则（「这一行到底登录过没有」），而它的两个字段的语义差别
 * 很容易判错（见下）。写在 `RelaySection` 的 JSX 里，测它就得渲染整个面板；
 * 抽出来之后一个纯函数就能钉住三种处境。形状照 `lowBalance.ts`（同目录先例）。
 */

/** 判据只吃这两个字段 —— 不吃整个 `RelayRow`，那会让它依赖一堆无关的东西。 */
interface SessionState {
  loggedIn: boolean;
  sessionExpired: boolean;
}

/**
 * 这一行**登录过**吗（而不是「此刻登录着」）。
 *
 * ## 为什么不能只判 `loggedIn`
 *
 * `loggedIn` 来自 `token_looks_valid`，说的是「此刻凭据还能用」。凭据过期的行
 * `loggedIn === false`，但库里**真有 token 与 account_id** —— 删它确实会删掉
 * 登录态。只判 `loggedIn` 会把它归到「从没登录」那句，于是反向说错。
 *
 * `sessionExpired` 的定义（`creds.rs` 的 `session_expired`）本身就含
 * `account_id.is_some()`，即「登录过且凭据已不可用」。所以两者取或正好是「登录过」。
 */
function hasEverLoggedIn(state: SessionState): boolean {
  return state.loggedIn || state.sessionExpired;
}

/**
 * 删站点确认框的正文 key。
 *
 * 两句话的差别不只是措辞：从没登录的行**没有登录态、也没有余额**，
 * 而原来那句无条件文案把这两样都说了 —— 用户删一个只探测过的占位行时，
 * 三句话里有两句不属实（用户实测指出）。
 */
export function removeConfirmMessageKey(state: SessionState): string {
  return hasEverLoggedIn(state)
    ? "loongport.row.removeConfirmMessage"
    : "loongport.row.removeConfirmMessageNeverLoggedIn";
}
